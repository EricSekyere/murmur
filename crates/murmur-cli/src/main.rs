mod mcp;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use cpal::SampleFormat;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use murmur_core::config::Settings;
use murmur_core::output::OutputMode;
use murmur_core::stt::models::{Backend, ModelManager, SttModel};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Parser)]
#[command(name = "murmur")]
#[command(about = "Voice-to-text for developers", long_about = None)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start listening for voice input (push-to-talk via global hotkey).
    Listen {
        /// Output to stdout instead of simulating keystrokes.
        #[arg(long)]
        stdout: bool,

        /// Copy transcription to clipboard.
        #[arg(long)]
        clipboard: bool,

        /// STT model to use (e.g. whisper-small-en, parakeet-tdt-06b-v2).
        #[arg(long, short)]
        model: Option<String>,
    },

    /// Manage configuration.
    Config {
        /// Show the current configuration.
        #[arg(long)]
        show: bool,

        /// Reset configuration to defaults.
        #[arg(long)]
        reset: bool,

        /// Set the global hotkey (e.g., "Ctrl+Shift+Space").
        #[arg(long)]
        hotkey: Option<String>,
    },

    /// Manage STT models.
    Models {
        /// List available and downloaded models.
        #[arg(long)]
        list: bool,

        /// Download a model (e.g. whisper-small-en, parakeet-tdt-06b-v2).
        #[arg(long)]
        download: Option<String>,
    },

    /// Test microphone capture without transcription or UI.
    AudioTest {
        /// Audio input device name. Omit to use system default.
        #[arg(long)]
        device: Option<String>,

        /// Test every available input device.
        #[arg(long)]
        all: bool,

        /// Capture duration in seconds.
        #[arg(long, default_value_t = 3)]
        seconds: u64,
    },

    /// Index a project and print the ranked codebase vocabulary it would inject.
    Index {
        /// Project root to scan.
        path: PathBuf,

        /// Maximum number of symbols to print.
        #[arg(long, default_value_t = 64)]
        max: usize,
    },

    /// Run a stdio MCP server exposing transcription history to Claude/Cursor.
    Mcp,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Logs go to stderr so they never corrupt stdout, which the `mcp`
    // subcommand uses as the MCP JSON-RPC channel.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    murmur_core::config::Settings::migrate_from_voitex();
    let cli = Cli::parse();

    match cli.command {
        Commands::Listen {
            stdout,
            clipboard,
            model,
        } => cmd_listen(stdout, clipboard, model).await?,
        Commands::Config {
            show,
            reset,
            hotkey,
        } => cmd_config(show, reset, hotkey)?,
        Commands::Models { list, download } => cmd_models(list, download).await?,
        Commands::AudioTest {
            device,
            all,
            seconds,
        } => cmd_audio_test(device, all, seconds)?,
        Commands::Index { path, max } => cmd_index(path, max)?,
        Commands::Mcp => mcp::run().await?,
    }

    Ok(())
}

/// Print the ranked codebase vocabulary for a project. Audio-free, so it works
/// without the STT features and is the cheap way to eyeball the ranking.
fn cmd_index(path: PathBuf, max: usize) -> Result<()> {
    use murmur_core::indexer::{IndexConfig, index_project_ranked};

    let cfg = IndexConfig {
        max_symbols: max,
        ..IndexConfig::default()
    };
    let ranked = index_project_ranked(&path, &cfg)
        .with_context(|| format!("Failed to index {}", path.display()))?;

    println!("{} symbols from {}", ranked.len(), path.display());
    println!("{:>8}  {:>5}  symbol", "score", "freq");
    for s in &ranked {
        println!("{:>8.2}  {:>5}  {}", s.score, s.freq, s.text);
    }
    Ok(())
}

fn cmd_audio_test(device_name: Option<String>, all: bool, seconds: u64) -> Result<()> {
    let host = cpal::default_host();
    let default_name = host
        .default_input_device()
        .and_then(|device| device.name().ok())
        .unwrap_or_else(|| "<none>".to_string());

    let devices = host
        .input_devices()
        .context("Failed to enumerate input devices")?
        .collect::<Vec<_>>();

    println!("Default input device: {}", default_name);
    println!("Available input devices:");
    for device in &devices {
        let name = device.name().unwrap_or_else(|_| "<unnamed>".to_string());
        let marker = if name == default_name {
            " (default)"
        } else {
            ""
        };
        println!("  - {}{}", name, marker);
    }

    let mut targets = Vec::new();
    if all {
        targets = devices;
    } else if let Some(requested) = device_name {
        let requested = requested.trim();
        let device = devices
            .into_iter()
            .find(|device| device.name().map(|name| name == requested).unwrap_or(false))
            .ok_or_else(|| anyhow::anyhow!("Input device not found: {}", requested))?;
        targets.push(device);
    } else {
        targets.push(
            host.default_input_device()
                .ok_or_else(|| anyhow::anyhow!("No default input device available"))?,
        );
    }

    for device in targets {
        test_audio_device(&device, seconds)?;
    }

    Ok(())
}

fn test_audio_device(device: &cpal::Device, seconds: u64) -> Result<()> {
    let name = device.name().unwrap_or_else(|_| "<unnamed>".to_string());
    let config = device
        .default_input_config()
        .with_context(|| format!("Failed to read default input config for {}", name))?;

    println!();
    println!("Testing: {}", name);
    println!(
        "  Config: {}Hz, {} channel(s), {:?}",
        config.sample_rate().0,
        config.channels(),
        config.sample_format()
    );
    println!("  Speak now for {} second(s)...", seconds);

    let samples = Arc::new(Mutex::new(Vec::<f32>::new()));
    let stream_config = config.config();
    let sample_format = config.sample_format();
    let stream = build_test_stream(device, &stream_config, sample_format, Arc::clone(&samples))?;

    stream.play()?;
    std::thread::sleep(Duration::from_secs(seconds));
    drop(stream);

    let samples = samples.lock().map_err(|e| anyhow::anyhow!("{}", e))?;
    let peak = samples.iter().map(|s| s.abs()).fold(0.0_f32, f32::max);
    let rms = if samples.is_empty() {
        0.0
    } else {
        (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
    };

    println!("  Captured samples: {}", samples.len());
    println!("  Peak: {:.6}", peak);
    println!("  RMS:  {:.6}", rms);

    if samples.is_empty() {
        println!("  Result: FAIL - stream delivered no samples");
    } else if peak < 0.001 || rms < 0.0001 {
        println!("  Result: FAIL - samples are near digital silence");
    } else {
        println!("  Result: OK - microphone signal detected");
    }

    Ok(())
}

fn build_test_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    sample_format: SampleFormat,
    samples: Arc<Mutex<Vec<f32>>>,
) -> Result<cpal::Stream> {
    let err_fn = |err| eprintln!("  Stream error: {}", err);

    let stream = match sample_format {
        SampleFormat::F32 => device.build_input_stream(
            config,
            move |data: &[f32], _| {
                if let Ok(mut out) = samples.lock() {
                    out.extend_from_slice(data);
                }
            },
            err_fn,
            None,
        )?,
        SampleFormat::I16 => device.build_input_stream(
            config,
            move |data: &[i16], _| {
                if let Ok(mut out) = samples.lock() {
                    out.extend(data.iter().map(|&s| s as f32 / i16::MAX as f32));
                }
            },
            err_fn,
            None,
        )?,
        SampleFormat::U16 => device.build_input_stream(
            config,
            move |data: &[u16], _| {
                if let Ok(mut out) = samples.lock() {
                    out.extend(
                        data.iter()
                            .map(|&s| (s as f32 / u16::MAX as f32) * 2.0 - 1.0),
                    );
                }
            },
            err_fn,
            None,
        )?,
        format => anyhow::bail!("Unsupported audio test sample format: {:?}", format),
    };

    Ok(stream)
}

async fn cmd_listen(stdout: bool, clipboard: bool, model_name: Option<String>) -> Result<()> {
    let config_path = Settings::default_path()?;
    let settings = Settings::load(&config_path)?;

    let output_mode = if stdout {
        OutputMode::Stdout
    } else if clipboard {
        OutputMode::Clipboard
    } else {
        settings.output_mode
    };

    let model = match model_name {
        Some(name) => SttModel::from_name(&name).ok_or_else(|| {
            let available: Vec<&str> = SttModel::all().iter().map(|m| m.id()).collect();
            anyhow::anyhow!(
                "Unknown model '{}'. Available: {}",
                name,
                available.join(", ")
            )
        })?,
        None => settings.model,
    };

    let model_mgr = ModelManager::new(ModelManager::default_dir()?);
    if !model_mgr.is_downloaded(model) {
        tracing::info!("Model {} not found, downloading...", model.name());
        model_mgr.download(model).await?;
    }

    let model_path = model_mgr.model_path(model);

    let mut stt_engine = match model.backend() {
        Backend::Whisper => murmur_core::stt::engine::SttEngine::new_whisper(
            model_path.to_str().context("Invalid model path")?,
            0,
        )?,
        Backend::Parakeet => murmur_core::stt::engine::SttEngine::new_parakeet(
            model_path.to_str().context("Invalid model path")?,
        )?,
    };

    let mut capture = murmur_core::audio::capture::AudioCapture::new()?;

    let hotkey_mgr = murmur_core::hotkey::HotkeyManager::new(&settings.hotkey)?;
    let hotkey_rx = hotkey_mgr.events();

    println!(
        "Murmur listening (hotkey: {}, model: {}, output: {:?})",
        settings.hotkey,
        model.name(),
        output_mode
    );
    println!(
        "Press {} to start recording, release to transcribe. Ctrl+C to quit.",
        settings.hotkey
    );

    loop {
        match hotkey_rx.recv() {
            Ok(murmur_core::hotkey::HotkeyEvent::Pressed) => {
                tracing::info!("Hotkey pressed — recording...");
                println!("Recording...");
                capture.start(settings.audio_device.as_deref(), settings.echo_cancellation)?;
            }
            Ok(murmur_core::hotkey::HotkeyEvent::Released) => {
                tracing::info!("Hotkey released — transcribing...");
                let audio = capture.stop()?;

                if audio.samples.is_empty() {
                    tracing::debug!("No audio captured, skipping");
                    continue;
                }

                let result = stt_engine.transcribe(&audio.samples)?;

                if result.text.is_empty() {
                    tracing::debug!("Empty transcription, skipping");
                    continue;
                }

                tracing::info!(
                    "Transcribed in {}ms: {}",
                    result.processing_time_ms,
                    result.text
                );

                output_text(&result.text, output_mode)?;
            }
            Err(_) => {
                tracing::info!("Hotkey channel closed, exiting");
                break;
            }
        }
    }

    Ok(())
}

fn output_text(text: &str, mode: OutputMode) -> Result<()> {
    murmur_core::output::dispatch_output(text, mode)
}

fn cmd_config(show: bool, reset: bool, hotkey: Option<String>) -> Result<()> {
    if show {
        let path = Settings::default_path()?;
        let settings = Settings::load(&path)?;
        println!("{}", toml::to_string_pretty(&settings)?);
    } else if reset {
        let path = Settings::default_path()?;
        let settings = Settings::default();
        settings.save(&path)?;
        println!("Configuration reset to defaults.");
    } else if let Some(hotkey) = hotkey {
        let path = Settings::default_path()?;
        let mut settings = Settings::load(&path)?;
        settings.hotkey = hotkey;
        settings.save(&path)?;
        println!("Hotkey updated.");
    } else {
        println!("Use --show, --reset, or --hotkey to manage config.");
    }
    Ok(())
}

async fn cmd_models(list: bool, download: Option<String>) -> Result<()> {
    if list {
        let dir = ModelManager::default_dir()?;
        let manager = ModelManager::new(dir);
        let downloaded = manager.list_downloaded();

        for model in SttModel::all() {
            let status = if downloaded.contains(model) {
                "downloaded"
            } else {
                "not downloaded"
            };
            println!(
                "  {} [{}] ({} MB) [{}] - {}",
                model.name(),
                model.backend(),
                model.size_mb(),
                status,
                model.description()
            );
        }
    } else if let Some(model_name) = download {
        let model = SttModel::from_name(&model_name).ok_or_else(|| {
            let available: Vec<&str> = SttModel::all().iter().map(|m| m.id()).collect();
            anyhow::anyhow!(
                "Unknown model '{}'. Available: {}",
                model_name,
                available.join(", ")
            )
        })?;
        let dir = ModelManager::default_dir()?;
        let manager = ModelManager::new(dir);

        if manager.is_downloaded(model) {
            println!(
                "{} is already downloaded at {}",
                model.name(),
                manager.model_path(model).display()
            );
        } else {
            let path = manager.download(model).await?;
            println!("Downloaded {} to {}", model.name(), path.display());
        }
    } else {
        println!("Use --list or --download to manage models.");
    }
    Ok(())
}
