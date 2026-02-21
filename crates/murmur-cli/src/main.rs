use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use murmur_core::config::Settings;
use murmur_core::output::OutputMode;
use murmur_core::stt::models::{Backend, ModelManager, SttModel};

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
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
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
    }

    Ok(())
}

async fn cmd_listen(stdout: bool, clipboard: bool, model_name: Option<String>) -> Result<()> {
    let config_path = Settings::default_path()?;
    let settings = Settings::load(&config_path)?;

    // Determine output mode
    let output_mode = if stdout {
        OutputMode::Stdout
    } else if clipboard {
        OutputMode::Clipboard
    } else {
        settings.output_mode
    };

    // Determine which model to use
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

    // Ensure model is downloaded
    let model_mgr = ModelManager::new(ModelManager::default_dir()?);
    if !model_mgr.is_downloaded(model) {
        tracing::info!("Model {} not found, downloading...", model.name());
        model_mgr.download(model).await?;
    }

    let model_path = model_mgr.model_path(model);

    // Initialize STT engine based on backend
    let mut stt_engine = match model.backend() {
        Backend::Whisper => murmur_core::stt::engine::SttEngine::new_whisper(
            model_path.to_str().context("Invalid model path")?,
            0,
        )?,
        Backend::Parakeet => murmur_core::stt::engine::SttEngine::new_parakeet(
            model_path.to_str().context("Invalid model path")?,
        )?,
    };

    // Initialize audio capture
    let mut capture = murmur_core::audio::capture::AudioCapture::new()?;

    // Register global hotkey
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

    // Main event loop
    loop {
        match hotkey_rx.recv() {
            Ok(murmur_core::hotkey::HotkeyEvent::Pressed) => {
                tracing::info!("Hotkey pressed — recording...");
                println!("Recording...");
                capture.start()?;
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
    match mode {
        OutputMode::Stdout => {
            murmur_core::output::stdout::StdoutOutput::new().write(text)?;
        }
        OutputMode::Clipboard => {
            murmur_core::output::clipboard::ClipboardOutput::new()?.copy(text)?;
            println!("Copied to clipboard: {}", text);
        }
        OutputMode::Keyboard => {
            murmur_core::output::keyboard::KeyboardOutput::new()?.type_text(text)?;
        }
    }
    Ok(())
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
