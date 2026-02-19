use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use voitex_core::config::Settings;
use voitex_core::output::OutputMode;
use voitex_core::stt::models::{ModelManager, WhisperModel};

#[derive(Parser)]
#[command(name = "voitex")]
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

        /// Whisper model to use (base-en, small-en, medium-en, large-v3-turbo).
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

    /// Manage Whisper models.
    Models {
        /// List available and downloaded models.
        #[arg(long)]
        list: bool,

        /// Download a model (base-en, small-en, medium-en, large-v3-turbo).
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
        settings.output_mode.into()
    };

    // Determine which model to use
    let model = match model_name {
        Some(name) => WhisperModel::from_name(&name).ok_or_else(|| {
            anyhow::anyhow!(
                "Unknown model '{}'. Available: base-en, small-en, medium-en, large-v3-turbo",
                name
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

    // Initialize STT engine
    let stt_engine = voitex_core::stt::engine::SttEngine::new(
        model_path.to_str().context("Invalid model path")?,
        0, // auto-detect threads
    )?;

    // Initialize audio capture
    let mut capture = voitex_core::audio::capture::AudioCapture::new()?;

    // Register global hotkey
    let hotkey_mgr = voitex_core::hotkey::HotkeyManager::new(&settings.hotkey)?;
    let hotkey_rx = hotkey_mgr.events();

    println!(
        "Voitex listening (hotkey: {}, model: {}, output: {:?})",
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
            Ok(voitex_core::hotkey::HotkeyEvent::Pressed) => {
                tracing::info!("Hotkey pressed — recording...");
                println!("Recording...");
                capture.start()?;
            }
            Ok(voitex_core::hotkey::HotkeyEvent::Released) => {
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
            voitex_core::output::stdout::StdoutOutput::new().write(text)?;
        }
        OutputMode::Clipboard => {
            voitex_core::output::clipboard::ClipboardOutput::new()?.copy(text)?;
            println!("Copied to clipboard: {}", text);
        }
        OutputMode::Keyboard => {
            voitex_core::output::keyboard::KeyboardOutput::new()?.type_text(text)?;
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

        for model in WhisperModel::all() {
            let status = if downloaded.contains(model) {
                "downloaded"
            } else {
                "not downloaded"
            };
            println!(
                "  {} ({} MB) [{}]",
                model.name(),
                model.size_mb(),
                status
            );
        }
    } else if let Some(model_name) = download {
        let model = WhisperModel::from_name(&model_name).ok_or_else(|| {
            anyhow::anyhow!(
                "Unknown model '{}'. Available: base-en, small-en, medium-en, large-v3-turbo",
                model_name
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
