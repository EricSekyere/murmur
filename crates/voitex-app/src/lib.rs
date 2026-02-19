use std::sync::Mutex;

use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager, State,
};
#[cfg(feature = "full")]
use tauri::Emitter;
use voitex_core::config::Settings;
#[cfg(feature = "full")]
use voitex_core::stt::models::ModelManager;

// --- Audio worker (runs AudioCapture on a dedicated thread) ---

#[cfg(feature = "full")]
mod audio_worker {
    use std::sync::mpsc;
    use voitex_core::audio::{capture::AudioCapture, AudioBuffer};

    pub enum Cmd {
        Start,
        Stop,
    }

    pub struct Handle {
        cmd_tx: mpsc::Sender<Cmd>,
        result_rx: Mutex<mpsc::Receiver<Result<AudioBuffer, String>>>,
    }

    use std::sync::Mutex;

    impl Handle {
        pub fn spawn() -> Self {
            let (cmd_tx, cmd_rx) = mpsc::channel::<Cmd>();
            let (result_tx, result_rx) = mpsc::channel::<Result<AudioBuffer, String>>();

            std::thread::spawn(move || {
                let mut capture = match AudioCapture::new() {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::error!("Failed to create AudioCapture: {}", e);
                        return;
                    }
                };

                while let Ok(cmd) = cmd_rx.recv() {
                    match cmd {
                        Cmd::Start => {
                            if let Err(e) = capture.start() {
                                let _ = result_tx.send(Err(e.to_string()));
                            }
                        }
                        Cmd::Stop => {
                            let result = capture.stop().map_err(|e| e.to_string());
                            let _ = result_tx.send(result);
                        }
                    }
                }
            });

            Handle {
                cmd_tx,
                result_rx: Mutex::new(result_rx),
            }
        }

        pub fn start(&self) -> Result<(), String> {
            self.cmd_tx.send(Cmd::Start).map_err(|e| e.to_string())
        }

        pub fn stop(&self) -> Result<AudioBuffer, String> {
            self.cmd_tx
                .send(Cmd::Stop)
                .map_err(|e| e.to_string())?;
            self.result_rx
                .lock()
                .map_err(|e| e.to_string())?
                .recv()
                .map_err(|e| e.to_string())?
        }
    }
}

/// Shared application state managed by Tauri.
struct AppState {
    #[cfg(feature = "full")]
    audio: audio_worker::Handle,
    #[cfg(feature = "full")]
    engine: voitex_core::stt::engine::SttEngine,
    recording: Mutex<bool>,
    settings: Mutex<Settings>,
}

#[derive(serde::Serialize, Clone)]
struct TranscriptionEvent {
    text: String,
    processing_time_ms: u64,
}

/// Tauri command: get the current app status.
#[tauri::command]
fn get_status(state: State<'_, AppState>) -> serde_json::Value {
    let recording = *state.recording.lock().unwrap_or_else(|e| e.into_inner());
    let settings = state.settings.lock().unwrap_or_else(|e| e.into_inner());

    serde_json::json!({
        "recording": recording,
        "model": settings.model.name(),
        "mode": if recording { "listening" } else { "idle" }
    })
}

/// Tauri command: start listening for voice input.
#[tauri::command]
fn start_listening(state: State<'_, AppState>) -> Result<(), String> {
    #[cfg(feature = "full")]
    {
        let mut recording = state.recording.lock().map_err(|e| e.to_string())?;
        if *recording {
            return Err("Already recording".into());
        }
        state.audio.start()?;
        *recording = true;
        tracing::info!("Audio capture started from Tauri app");
        Ok(())
    }

    #[cfg(not(feature = "full"))]
    {
        let _ = state;
        Err("Built without 'full' feature — audio capture not available".into())
    }
}

/// Tauri command: stop listening, transcribe, and emit result.
#[tauri::command]
fn stop_listening(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<TranscriptionEvent, String> {
    #[cfg(feature = "full")]
    {
        *state.recording.lock().map_err(|e| e.to_string())? = false;
        let audio = state.audio.stop()?;

        if audio.samples.is_empty() {
            return Ok(TranscriptionEvent {
                text: String::new(),
                processing_time_ms: 0,
            });
        }

        let result = state
            .engine
            .transcribe(&audio.samples)
            .map_err(|e| e.to_string())?;

        tracing::info!(
            "Transcribed in {}ms: {}",
            result.processing_time_ms,
            result.text
        );

        let event = TranscriptionEvent {
            text: result.text,
            processing_time_ms: result.processing_time_ms,
        };

        let _ = app.emit("transcription", &event);

        Ok(event)
    }

    #[cfg(not(feature = "full"))]
    {
        let _ = (app, state);
        Err("Built without 'full' feature — transcription not available".into())
    }
}

/// Tauri command: get the current configuration.
#[tauri::command]
fn get_config(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    let settings = state.settings.lock().map_err(|e| e.to_string())?;
    serde_json::to_value(&*settings).map_err(|e| e.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let config_path = Settings::default_path().expect("Failed to determine config path");
    let settings = Settings::load(&config_path).expect("Failed to load settings");

    #[cfg(feature = "full")]
    let app_state = {
        let model = settings.model;
        let model_mgr = ModelManager::new(
            ModelManager::default_dir().expect("Failed to determine models directory"),
        );

        let model_path = model_mgr.model_path(model);
        if !model_mgr.is_downloaded(model) {
            tracing::warn!(
                "Model {} not downloaded. Run `voitex models --download {}` first.",
                model.name(),
                model.name()
            );
        }

        let engine = voitex_core::stt::engine::SttEngine::new(
            model_path.to_str().expect("Invalid model path"),
            0,
        )
        .expect("Failed to initialize STT engine");

        let audio = audio_worker::Handle::spawn();

        AppState {
            audio,
            engine,
            recording: Mutex::new(false),
            settings: Mutex::new(settings),
        }
    };

    #[cfg(not(feature = "full"))]
    let app_state = AppState {
        recording: Mutex::new(false),
        settings: Mutex::new(settings),
    };

    tauri::Builder::default()
        .manage(app_state)
        .invoke_handler(tauri::generate_handler![
            get_status,
            start_listening,
            stop_listening,
            get_config,
        ])
        .setup(|app| {
            let show_i = MenuItem::with_id(app, "show", "Show", true, None::<&str>)?;
            let quit_i = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show_i, &quit_i])?;

            let _tray = TrayIconBuilder::new()
                .icon(app.default_window_icon().cloned().unwrap())
                .menu(&menu)
                .show_menu_on_left_click(false)
                .tooltip("Voitex - Voice to Text")
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "quit" => {
                        app.exit(0);
                    }
                    "show" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.unminimize();
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.unminimize();
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                })
                .build(app)?;

            tracing::info!("Voitex app started");
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Voitex");
}
