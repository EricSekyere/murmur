use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Context;
use murmur_core::config::Settings;
use murmur_core::stt::engine::SttEngine;
use murmur_core::stt::models::{Backend, ModelManager, SttModel};
use murmur_core::stt::postprocess::PostProcessor;
use murmur_core::stt::runtime;
use tauri::Emitter;
use tauri::{
    Manager, State,
    image::Image,
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

// --- Audio worker (runs AudioCapture on a dedicated thread with silence monitoring) ---

mod audio_worker {
    use murmur_core::audio::AudioBuffer;
    use murmur_core::audio::capture::AudioCapture;
    use murmur_core::audio::silence::{PhraseDetector, PhraseState, compute_rms, downmix_to_mono};
    use std::sync::Mutex;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    enum Cmd {
        StartStreaming {
            rms_threshold: f32,
            phrase_pause: Duration,
            session_timeout: Duration,
        },
        Stop,
    }

    pub enum AudioResult {
        Started,
        StartFailed(String),
        PhraseReady(AudioBuffer),
        StreamingDone,
    }

    pub struct Handle {
        cmd_tx: mpsc::Sender<Cmd>,
        result_rx: Mutex<mpsc::Receiver<AudioResult>>,
    }

    impl Handle {
        pub fn spawn() -> Self {
            let (cmd_tx, cmd_rx) = mpsc::channel::<Cmd>();
            let (result_tx, result_rx) = mpsc::channel::<AudioResult>();

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
                        Cmd::StartStreaming {
                            rms_threshold,
                            phrase_pause,
                            session_timeout,
                        } => {
                            if let Err(e) = capture.start() {
                                let _ = result_tx.send(AudioResult::StartFailed(e.to_string()));
                                continue;
                            }
                            let _ = result_tx.send(AudioResult::Started);

                            let live_buf = capture.live_buffer();
                            let native_rate = capture.native_rate();
                            let native_channels = capture.native_channels();
                            let mut analyzed_up_to = 0usize;
                            let mut phrase_start_idx = 0usize;

                            // ── Calibration phase: measure ambient noise ──
                            let calibration_duration = Duration::from_millis(500);
                            let calibration_start = Instant::now();
                            let mut ambient_rms_samples = Vec::new();

                            tracing::info!(
                                "Calibrating noise floor ({} ch, {}Hz)...",
                                native_channels,
                                native_rate
                            );
                            while calibration_start.elapsed() < calibration_duration {
                                std::thread::sleep(Duration::from_millis(50));
                                let buf = live_buf.lock().unwrap_or_else(|e| e.into_inner());
                                if buf.len() > analyzed_up_to {
                                    let mono =
                                        downmix_to_mono(&buf[analyzed_up_to..], native_channels);
                                    let chunk_rms = compute_rms(&mono);
                                    if chunk_rms > 0.0 {
                                        ambient_rms_samples.push(chunk_rms);
                                    }
                                    analyzed_up_to = buf.len();
                                }
                            }

                            let ambient_rms = if ambient_rms_samples.is_empty() {
                                0.0
                            } else {
                                ambient_rms_samples.iter().sum::<f32>()
                                    / ambient_rms_samples.len() as f32
                            };

                            // If config threshold > 0, use it directly; otherwise auto-calibrate.
                            // Multiplier of 1.5x above ambient keeps sensitivity high for
                            // normal speech while filtering out background noise.
                            // Cap at 0.015 so even in noisy rooms, regular speech is detected.
                            let calibrated_threshold = if rms_threshold > 0.0 {
                                rms_threshold
                            } else {
                                (ambient_rms * 1.5).clamp(0.0001, 0.015)
                            };
                            tracing::info!(
                                "Calibrated: ambient RMS = {:.6}, threshold = {:.6} (config = {:.6}, mode = {})",
                                ambient_rms,
                                calibrated_threshold,
                                rms_threshold,
                                if rms_threshold > 0.0 {
                                    "manual"
                                } else {
                                    "auto"
                                }
                            );

                            let mut detector = PhraseDetector::new(
                                calibrated_threshold,
                                phrase_pause,
                                session_timeout,
                            );

                            loop {
                                // Check for manual Stop
                                if let Ok(Cmd::Stop) = cmd_rx.try_recv() {
                                    // Drain any remaining audio as a final phrase
                                    let remaining = {
                                        let buf =
                                            live_buf.lock().unwrap_or_else(|e| e.into_inner());
                                        if buf.len() > phrase_start_idx {
                                            Some(buf[phrase_start_idx..].to_vec())
                                        } else {
                                            None
                                        }
                                    };
                                    if let Some(raw) = remaining {
                                        let audio = AudioBuffer::from_raw(
                                            &raw,
                                            native_rate,
                                            native_channels,
                                        );
                                        if !audio.samples.is_empty() {
                                            let _ = result_tx.send(AudioResult::PhraseReady(audio));
                                        }
                                    }
                                    let _ = capture.stop();
                                    let _ = result_tx.send(AudioResult::StreamingDone);
                                    break;
                                }

                                // Read new samples, downmix to mono, feed to phrase detector
                                let state = {
                                    let buf = live_buf.lock().unwrap_or_else(|e| e.into_inner());
                                    if buf.len() > analyzed_up_to {
                                        let mono = downmix_to_mono(
                                            &buf[analyzed_up_to..],
                                            native_channels,
                                        );
                                        let st = detector.feed(&mono);
                                        analyzed_up_to = buf.len();
                                        st
                                    } else {
                                        detector.state()
                                    }
                                };

                                match state {
                                    PhraseState::PhraseComplete => {
                                        // Drain the phrase audio
                                        let raw = {
                                            let buf =
                                                live_buf.lock().unwrap_or_else(|e| e.into_inner());
                                            buf[phrase_start_idx..analyzed_up_to].to_vec()
                                        };
                                        phrase_start_idx = analyzed_up_to;

                                        let audio = AudioBuffer::from_raw(
                                            &raw,
                                            native_rate,
                                            native_channels,
                                        );
                                        if !audio.samples.is_empty() {
                                            let _ = result_tx.send(AudioResult::PhraseReady(audio));
                                        }

                                        detector.reset_phrase();
                                    }
                                    PhraseState::SessionTimeout => {
                                        // Drain any remaining audio
                                        let remaining = {
                                            let buf =
                                                live_buf.lock().unwrap_or_else(|e| e.into_inner());
                                            if buf.len() > phrase_start_idx {
                                                Some(buf[phrase_start_idx..].to_vec())
                                            } else {
                                                None
                                            }
                                        };
                                        if let Some(raw) = remaining {
                                            let audio = AudioBuffer::from_raw(
                                                &raw,
                                                native_rate,
                                                native_channels,
                                            );
                                            if !audio.samples.is_empty() {
                                                let _ =
                                                    result_tx.send(AudioResult::PhraseReady(audio));
                                            }
                                        }
                                        tracing::info!("Streaming session timeout — no speech");
                                        let _ = capture.stop();
                                        let _ = result_tx.send(AudioResult::StreamingDone);
                                        break;
                                    }
                                    _ => {}
                                }

                                std::thread::sleep(Duration::from_millis(50));
                            }
                        }

                        Cmd::Stop => {
                            tracing::debug!("Stop received outside monitoring loop, ignoring");
                        }
                    }
                }
            });

            Handle {
                cmd_tx,
                result_rx: Mutex::new(result_rx),
            }
        }

        /// Start streaming mode. Blocks until audio capture is ready.
        pub fn start_streaming(
            &self,
            rms_threshold: f32,
            phrase_pause: Duration,
            session_timeout: Duration,
        ) -> Result<(), String> {
            self.cmd_tx
                .send(Cmd::StartStreaming {
                    rms_threshold,
                    phrase_pause,
                    session_timeout,
                })
                .map_err(|e| format!("Audio worker channel closed: {}", e))?;

            let rx = self.result_rx.lock().unwrap_or_else(|e| e.into_inner());
            match rx.recv_timeout(Duration::from_secs(5)) {
                Ok(AudioResult::Started) => Ok(()),
                Ok(AudioResult::StartFailed(e)) => Err(e),
                Ok(_) => Err("Unexpected response from audio worker".to_string()),
                Err(e) => Err(format!("Audio worker response timeout: {}", e)),
            }
        }

        /// Request the worker to stop recording. Non-blocking.
        pub fn request_stop(&self) -> Result<(), String> {
            self.cmd_tx
                .send(Cmd::Stop)
                .map_err(|e| format!("Audio worker channel closed: {}", e))
        }

        /// Blocking receive for the next streaming result.
        pub fn recv_result(&self) -> Result<AudioResult, String> {
            let rx = self.result_rx.lock().unwrap_or_else(|e| e.into_inner());
            rx.recv_timeout(Duration::from_secs(120))
                .map_err(|e| format!("Audio worker recv timeout: {}", e))
        }
    }
}

/// Shared application state managed by Tauri.
struct AppState {
    audio: audio_worker::Handle,
    engine: Arc<Mutex<Option<SttEngine>>>,
    recording: Mutex<bool>,
    settings: Mutex<Settings>,
    last_toggle: Mutex<Instant>,
}

#[derive(serde::Serialize, Clone)]
struct RecordingStateEvent {
    recording: bool,
    processing: bool,
}

#[derive(serde::Serialize, Clone)]
struct ModelDownloadProgress {
    percent: u8,
    message: String,
    done: bool,
    error: Option<String>,
}

#[derive(serde::Serialize, Clone)]
struct ModelInfo {
    id: String,
    name: String,
    backend: String,
    size_mb: u32,
    description: String,
    downloaded: bool,
    active: bool,
}

#[derive(serde::Serialize, Clone)]
struct ModelChangedEvent {
    model_id: String,
    model_name: String,
    ready: bool,
}

/// Emit a `recording-state` event to all windows (main + widget).
fn emit_recording_state(app: &tauri::AppHandle, recording: bool, processing: bool) {
    let _ = app.emit(
        "recording-state",
        RecordingStateEvent {
            recording,
            processing,
        },
    );
}

// ─── Tauri Commands ──────────────────────────────────────────────────────────

/// Tauri command: get the current app status.
#[tauri::command]
fn get_status(state: State<'_, AppState>) -> serde_json::Value {
    let recording = *state.recording.lock().unwrap_or_else(|e| e.into_inner());

    // Read settings and drop the lock before acquiring engine lock
    // to avoid ABBA deadlock with transcribe_chunk (which locks engine first).
    let (model_name, model_id, hotkey, output_mode, developer_mode) = {
        let settings = state.settings.lock().unwrap_or_else(|e| e.into_inner());
        (
            settings.model.name().to_string(),
            settings.model.id().to_string(),
            settings.hotkey.clone(),
            settings.output_mode,
            settings.developer_mode,
        )
    };

    let model_ready = state
        .engine
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .is_some();

    serde_json::json!({
        "recording": recording,
        "model": model_name,
        "model_id": model_id,
        "model_ready": model_ready,
        "mode": if recording { "listening" } else { "idle" },
        "hotkey": hotkey,
        "output_mode": output_mode,
        "developer_mode": developer_mode,
    })
}

/// Tauri command: toggle recording on/off. Used by widget, main window, and hotkey.
#[tauri::command]
fn toggle_recording(app: tauri::AppHandle) -> Result<(), String> {
    handle_toggle(&app);
    Ok(())
}

/// Tauri command: get the current configuration.
#[tauri::command]
fn get_config(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    let settings = state.settings.lock().unwrap_or_else(|e| e.into_inner());
    serde_json::to_value(&*settings).map_err(|e| e.to_string())
}

/// Tauri command: manually trigger model download (fallback if auto-download failed).
#[tauri::command]
async fn download_model(app: tauri::AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    {
        let guard = state.engine.lock().unwrap_or_else(|e| e.into_inner());
        if guard.is_some() {
            return Ok(());
        }
    }

    let model = state
        .settings
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .model;
    let engine_ref = Arc::clone(&state.engine);

    spawn_download_and_init(app, engine_ref, model);
    Ok(())
}

/// Tauri command: list all available models with their status.
#[tauri::command]
fn list_models(state: State<'_, AppState>) -> Result<Vec<ModelInfo>, String> {
    let model_mgr = ModelManager::new(ModelManager::default_dir().map_err(|e| e.to_string())?);

    let settings = state.settings.lock().unwrap_or_else(|e| e.into_inner());
    let active_model = settings.model;

    let models: Vec<ModelInfo> = SttModel::all()
        .iter()
        .map(|model| ModelInfo {
            id: model.id().to_string(),
            name: model.name().to_string(),
            backend: model.backend().to_string(),
            size_mb: model.size_mb(),
            description: model.description().to_string(),
            downloaded: model_mgr.is_downloaded(*model),
            active: *model == active_model,
        })
        .collect();

    Ok(models)
}

/// Tauri command: change the active STT model.
#[tauri::command]
async fn change_model(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    model_id: String,
) -> Result<(), String> {
    let model =
        SttModel::from_name(&model_id).ok_or_else(|| format!("Unknown model '{}'", model_id))?;

    // Check if already active
    {
        let settings = state.settings.lock().unwrap_or_else(|e| e.into_inner());
        if settings.model == model {
            let engine_guard = state.engine.lock().unwrap_or_else(|e| e.into_inner());
            if engine_guard.is_some() {
                return Ok(());
            }
        }
    }

    // Clear current engine
    {
        let mut guard = state.engine.lock().unwrap_or_else(|e| e.into_inner());
        *guard = None;
    }

    // Emit loading state
    let _ = app.emit(
        "model-changed",
        ModelChangedEvent {
            model_id: model.id().to_string(),
            model_name: model.name().to_string(),
            ready: false,
        },
    );

    // Update settings
    {
        let mut settings = state.settings.lock().unwrap_or_else(|e| e.into_inner());
        settings.model = model;
        if let Ok(path) = Settings::default_path()
            && let Err(e) = settings.save(&path)
        {
            tracing::error!("Failed to save settings: {}", e);
        }
    }

    let engine_ref = Arc::clone(&state.engine);
    spawn_download_and_init(app, engine_ref, model);

    Ok(())
}

/// Tauri command: get developer mode state.
#[tauri::command]
fn get_developer_mode(state: State<'_, AppState>) -> bool {
    state
        .settings
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .developer_mode
}

/// Tauri command: set developer mode on/off and persist to config.
#[tauri::command]
fn set_developer_mode(state: State<'_, AppState>, enabled: bool) -> Result<(), String> {
    let mut settings = state.settings.lock().unwrap_or_else(|e| e.into_inner());
    settings.developer_mode = enabled;
    if let Ok(path) = Settings::default_path() {
        settings.save(&path).map_err(|e| e.to_string())?;
    }
    tracing::info!("Developer mode {}", if enabled { "enabled" } else { "disabled" });
    Ok(())
}

// ─── Model Download & Init ───────────────────────────────────────────────────

/// Spawn a background task that downloads the model, inits the engine, and emits progress events.
fn spawn_download_and_init(
    app: tauri::AppHandle,
    engine: Arc<Mutex<Option<SttEngine>>>,
    model: SttModel,
) {
    let app_handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let result = download_and_init_model(&app_handle, &engine, model).await;
        if let Err(e) = result {
            tracing::error!("Model download/init failed: {}", e);
            let _ = app_handle.emit(
                "model-download-progress",
                ModelDownloadProgress {
                    percent: 0,
                    message: format!("Download failed: {}", e),
                    done: false,
                    error: Some(e.to_string()),
                },
            );
        }
    });
}

/// Download the model (with progress events) and initialize the STT engine.
async fn download_and_init_model(
    app: &tauri::AppHandle,
    engine: &Arc<Mutex<Option<SttEngine>>>,
    model: SttModel,
) -> anyhow::Result<()> {
    let model_mgr = ModelManager::new(
        ModelManager::default_dir().context("Failed to determine models directory")?,
    );

    // For ONNX Runtime-based backends, ensure the runtime DLL is available
    if model.backend() == Backend::Parakeet && !runtime::is_downloaded() {
        let _ = app.emit(
            "model-download-progress",
            ModelDownloadProgress {
                percent: 0,
                message: "Downloading ONNX Runtime...".to_string(),
                done: false,
                error: None,
            },
        );

        let app_ref = app.clone();
        runtime::download_with_progress(|downloaded, total| {
            let percent = total
                .map(|t| {
                    if t > 0 {
                        ((downloaded * 100) / t).min(100) as u8
                    } else {
                        0
                    }
                })
                .unwrap_or(0);

            let _ = app_ref.emit(
                "model-download-progress",
                ModelDownloadProgress {
                    percent,
                    message: format!("Downloading ONNX Runtime... {}%", percent),
                    done: false,
                    error: None,
                },
            );
        })
        .await
        .context("ONNX Runtime download failed")?;
    }

    // Download model files if not already present
    if !model_mgr.is_downloaded(model) {
        let _ = app.emit(
            "model-download-progress",
            ModelDownloadProgress {
                percent: 0,
                message: format!("Downloading {}...", model.name()),
                done: false,
                error: None,
            },
        );

        let app_ref = app.clone();
        model_mgr
            .download_with_progress(model, move |downloaded, total| {
                let percent = total
                    .map(|t| {
                        if t > 0 {
                            ((downloaded * 100) / t).min(100) as u8
                        } else {
                            0
                        }
                    })
                    .unwrap_or(0);

                let _ = app_ref.emit(
                    "model-download-progress",
                    ModelDownloadProgress {
                        percent,
                        message: format!("Downloading {}... {}%", model.name(), percent),
                        done: false,
                        error: None,
                    },
                );
            })
            .await
            .context("Model download failed")?;
    }

    let _ = app.emit(
        "model-download-progress",
        ModelDownloadProgress {
            percent: 100,
            message: "Loading model...".to_string(),
            done: false,
            error: None,
        },
    );

    let model_path = model_mgr.model_path(model);
    let path_str = model_path
        .to_str()
        .context("Invalid model path (non-UTF-8)")?
        .to_string();

    let backend = model.backend();

    // SttEngine init is CPU-intensive; run on a blocking thread
    let stt = tokio::task::spawn_blocking(move || match backend {
        Backend::Whisper => SttEngine::new_whisper(&path_str, 0),
        Backend::Parakeet => SttEngine::new_parakeet(&path_str),
    })
    .await
    .context("Engine init task panicked")?
    .context("Failed to initialize STT engine")?;

    {
        let mut guard = engine.lock().unwrap_or_else(|e| e.into_inner());
        *guard = Some(stt);
    }

    let _ = app.emit(
        "model-download-progress",
        ModelDownloadProgress {
            percent: 100,
            message: "Model ready".to_string(),
            done: true,
            error: None,
        },
    );

    let _ = app.emit(
        "model-changed",
        ModelChangedEvent {
            model_id: model.id().to_string(),
            model_name: model.name().to_string(),
            ready: true,
        },
    );

    tracing::info!("Model {} downloaded and engine initialized", model.name());
    Ok(())
}

// ─── Output ──────────────────────────────────────────────────────────────────

/// Output transcribed text according to the configured output mode.
///
/// - `Keyboard`: simulates keystrokes via enigo to type into the focused app.
/// - `Clipboard`: copies text to the system clipboard (user pastes manually).
/// - `Stdout`: no-op in the desktop app (text is emitted via events to the UI).
fn output_text(text: &str, mode: murmur_core::output::OutputMode) -> anyhow::Result<()> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(());
    }

    match mode {
        murmur_core::output::OutputMode::Keyboard => {
            let mut kb = murmur_core::output::keyboard::KeyboardOutput::new()
                .context("Failed to create keyboard output")?;
            let with_space = format!("{} ", trimmed);
            kb.type_text(&with_space)
                .context("Failed to type text via keyboard")?;
        }
        murmur_core::output::OutputMode::Clipboard => {
            let mut cb = murmur_core::output::clipboard::ClipboardOutput::new()
                .context("Failed to open clipboard")?;
            cb.copy(trimmed).context("Failed to copy text to clipboard")?;
        }
        murmur_core::output::OutputMode::Stdout => {
            // In the desktop app, stdout output is a no-op.
            // Text is delivered to the UI via the streaming-phrase event.
        }
    }

    Ok(())
}

/// Transcribe an audio buffer and return the text, or None if empty/error.
fn transcribe_chunk(
    app: &tauri::AppHandle,
    audio: &murmur_core::audio::AudioBuffer,
) -> Option<(String, u64)> {
    if audio.samples.is_empty() {
        return None;
    }

    let state = app.state::<AppState>();

    let developer_mode = state
        .settings
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .developer_mode;

    let mut engine_guard = state.engine.lock().unwrap_or_else(|e| e.into_inner());
    let engine = engine_guard.as_mut()?;

    match engine.transcribe(&audio.samples) {
        Ok(result) if !result.text.is_empty() => {
            let text = if developer_mode {
                let processed = PostProcessor::process(&result.text);
                tracing::info!(
                    "Phrase transcribed in {}ms (dev mode): raw={:?} processed={:?}",
                    result.processing_time_ms,
                    result.text,
                    processed
                );
                processed
            } else {
                tracing::info!(
                    "Phrase transcribed in {}ms: {}",
                    result.processing_time_ms,
                    result.text
                );
                result.text
            };
            if text.is_empty() {
                None
            } else {
                Some((text, result.processing_time_ms))
            }
        }
        Ok(_) => None,
        Err(e) => {
            tracing::error!("Phrase transcription failed: {}", e);
            None
        }
    }
}

// ─── Toggle Recording Logic ──────────────────────────────────────────────────

/// Handle a recording toggle (start or stop). Called from hotkey, Tauri command, and mouse-click listener.
/// Includes debounce to prevent double-toggles when multiple input sources fire for the same action.
fn handle_toggle(app: &tauri::AppHandle) {
    let state = app.state::<AppState>();

    // Debounce: ignore toggles within 500ms of the last one
    {
        let mut last = state.last_toggle.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        if now.duration_since(*last) < Duration::from_millis(500) {
            return;
        }
        *last = now;
    }

    let mut recording = state.recording.lock().unwrap_or_else(|e| e.into_inner());

    if *recording {
        // ── Manual stop ──────────────────────────────────────────────────
        *recording = false;
        drop(recording);

        tracing::info!("Toggle: manual stop");

        if let Err(e) = state.audio.request_stop() {
            tracing::error!("Failed to send stop command: {}", e);
            emit_recording_state(app, false, false);
            let _ = app.emit(
                "hotkey-error",
                serde_json::json!({ "error": format!("Failed to stop recording: {}", e) }),
            );
        }
        // The streaming_worker thread will handle cleanup via StreamingDone
    } else {
        // ── Start streaming ──────────────────────────────────────────────
        // Guard: engine not loaded
        {
            let engine = state.engine.lock().unwrap_or_else(|e| e.into_inner());
            if engine.is_none() {
                drop(recording);
                tracing::debug!("Toggle: engine not loaded yet");
                let _ = app.emit(
                    "hotkey-error",
                    serde_json::json!({ "error": "Model not loaded yet" }),
                );
                return;
            }
        }

        // Set recording immediately to prevent double-toggle
        *recording = true;
        drop(recording);

        tracing::info!("Toggle: start streaming");
        emit_recording_state(app, true, false);

        let app_handle = app.clone();
        std::thread::spawn(move || {
            streaming_worker(&app_handle);
        });
    }
}

/// Background thread: streaming mode — detect phrases, transcribe each, type into focused field.
fn streaming_worker(app: &tauri::AppHandle) {
    let state = app.state::<AppState>();

    let (rms_threshold, phrase_pause, session_timeout, output_mode) = {
        let settings = state.settings.lock().unwrap_or_else(|e| e.into_inner());
        (
            settings.silence_rms_threshold,
            Duration::from_secs_f32(settings.phrase_pause_secs),
            Duration::from_secs_f32(settings.session_timeout_secs),
            settings.output_mode,
        )
    };

    // Start streaming (blocks until audio capture is ready)
    if let Err(e) = state
        .audio
        .start_streaming(rms_threshold, phrase_pause, session_timeout)
    {
        tracing::error!("Failed to start streaming: {}", e);
        *state.recording.lock().unwrap_or_else(|e| e.into_inner()) = false;
        emit_recording_state(app, false, false);
        let _ = app.emit(
            "hotkey-error",
            serde_json::json!({ "error": format!("Failed to start recording: {}", e) }),
        );
        return;
    }

    // Loop: receive PhraseReady / StreamingDone
    loop {
        match state.audio.recv_result() {
            Ok(audio_worker::AudioResult::PhraseReady(audio)) => {
                tracing::info!(
                    "Phrase ready: {} samples ({:.1}s of audio)",
                    audio.samples.len(),
                    audio.samples.len() as f32 / 16000.0
                );

                // Brief "processing" flash while transcribing
                emit_recording_state(app, true, true);

                let result = transcribe_chunk(app, &audio);
                tracing::info!("Transcription result: {:?}", result.as_ref().map(|(t, ms)| (t.as_str(), ms)));

                if let Some((ref text, processing_time_ms)) = result {
                    if let Err(e) = output_text(text, output_mode) {
                        tracing::error!("Failed to output text: {}", e);
                        let _ = app.emit(
                            "hotkey-error",
                            serde_json::json!({ "error": format!("Failed to output text: {}", e) }),
                        );
                    }

                    let _ = app.emit("streaming-phrase", serde_json::json!({ "text": text, "processing_time_ms": processing_time_ms }));
                }

                // Back to "listening"
                let still_recording = *state.recording.lock().unwrap_or_else(|e| e.into_inner());
                if still_recording {
                    emit_recording_state(app, true, false);
                }
            }
            Ok(audio_worker::AudioResult::StreamingDone) => {
                tracing::info!("Streaming session ended");
                break;
            }
            Ok(audio_worker::AudioResult::Started) => {
                // Shouldn't happen in this loop, but harmless
            }
            Ok(audio_worker::AudioResult::StartFailed(e)) => {
                tracing::error!("Unexpected StartFailed during streaming: {}", e);
                break;
            }
            Err(e) => {
                tracing::error!("Streaming recv error: {}", e);
                let _ = app.emit(
                    "hotkey-error",
                    serde_json::json!({ "error": format!("Streaming error: {}", e) }),
                );
                break;
            }
        }
    }

    // Cleanup
    *state.recording.lock().unwrap_or_else(|e| e.into_inner()) = false;
    emit_recording_state(app, false, false);
    let _ = app.emit("streaming-done", serde_json::json!({}));
}

// ─── Hotkey Handler ──────────────────────────────────────────────────────────

/// Handle a global hotkey event. Toggle mode: press to start/stop, release ignored.
fn handle_hotkey_event(app: &tauri::AppHandle, shortcut_state: ShortcutState) {
    match shortcut_state {
        ShortcutState::Pressed => handle_toggle(app),
        ShortcutState::Released => {} // Toggle mode — release is ignored
    }
}

// ─── Tray & App Setup ────────────────────────────────────────────────────────

/// 1x1 transparent PNG used as fallback tray icon.
fn fallback_icon() -> Image<'static> {
    Image::new_owned(vec![0, 0, 0, 0], 1, 1)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    Settings::migrate_from_voitex();
    let config_path = Settings::default_path().context("Failed to determine config path")?;
    let settings = Settings::load(&config_path).context("Failed to load settings")?;

    let model = settings.model;
    let model_mgr = ModelManager::new(
        ModelManager::default_dir().context("Failed to determine models directory")?,
    );

    let model_already_downloaded = model_mgr.is_downloaded(model);

    // For Parakeet, also need the ONNX Runtime DLL
    let ort_ready = model.backend() != Backend::Parakeet || runtime::is_downloaded();

    // If model exists (and runtime DLL for Parakeet), init engine immediately
    let engine = if model_already_downloaded && ort_ready {
        let model_path = model_mgr.model_path(model);
        let path_str = model_path
            .to_str()
            .context("Invalid model path (non-UTF-8)")?;

        let result = match model.backend() {
            Backend::Whisper => SttEngine::new_whisper(path_str, 0),
            Backend::Parakeet => SttEngine::new_parakeet(path_str),
        };

        match result {
            Ok(e) => Some(e),
            Err(e) => {
                tracing::error!("Failed to initialize STT engine: {}", e);
                None
            }
        }
    } else {
        tracing::info!(
            "Model {} not ready, will auto-download on startup",
            model.name()
        );
        None
    };

    // Need to auto-download if model or runtime is missing
    let need_auto_download = !model_already_downloaded || !ort_ready;

    let engine = Arc::new(Mutex::new(engine));
    let audio = audio_worker::Handle::spawn();

    let hotkey_str = settings.hotkey.clone();

    let app_state = AppState {
        audio,
        engine: Arc::clone(&engine),
        recording: Mutex::new(false),
        settings: Mutex::new(settings),
        last_toggle: Mutex::new(Instant::now() - Duration::from_secs(10)),
    };

    tauri::Builder::default()
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(move |app, _shortcut, event| {
                    handle_hotkey_event(app, event.state);
                })
                .build(),
        )
        .manage(app_state)
        .on_window_event(|window, event| {
            // Hide the main window on close instead of destroying it,
            // so it can be re-shown from the tray icon.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event
                && window.label() == "main"
            {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_status,
            toggle_recording,
            get_config,
            download_model,
            list_models,
            change_model,
            get_developer_mode,
            set_developer_mode,
        ])
        .setup(move |app| {
            let show_i = MenuItem::with_id(app, "show", "Show", true, None::<&str>)?;
            let quit_i = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show_i, &quit_i])?;

            let icon = app.default_window_icon().cloned().unwrap_or_else(|| {
                tracing::warn!("No default window icon found, using fallback");
                fallback_icon()
            });

            let _tray = TrayIconBuilder::new()
                .icon(icon)
                .menu(&menu)
                .show_menu_on_left_click(false)
                .tooltip("Murmur - Voice to Text")
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

            // Register global hotkey (toggle mode)
            let shortcut: tauri_plugin_global_shortcut::Shortcut = hotkey_str
                .parse()
                .map_err(|e| anyhow::anyhow!("Failed to parse hotkey '{}': {:?}", hotkey_str, e))?;

            // Unregister first in case a previous instance left it registered
            // (e.g. after a force-kill that skipped cleanup)
            let _ = app.global_shortcut().unregister(shortcut);

            app.global_shortcut().register(shortcut).map_err(|e| {
                anyhow::anyhow!("Failed to register hotkey '{}': {:?}", hotkey_str, e)
            })?;

            tracing::info!("Registered global hotkey: {}", hotkey_str);

            // Auto-download model (and ORT runtime if needed) if not ready
            if need_auto_download {
                let engine_ref = Arc::clone(&engine);
                spawn_download_and_init(app.handle().clone(), engine_ref, model);
            }

            // Global mouse-click listener: any click stops an active recording session.
            let app_for_mouse = app.handle().clone();
            std::thread::spawn(move || {
                if let Err(e) = rdev::listen(move |event| {
                    if let rdev::EventType::ButtonPress(
                        rdev::Button::Left | rdev::Button::Right | rdev::Button::Middle,
                    ) = event.event_type
                    {
                        let state = app_for_mouse.state::<AppState>();
                        let is_recording = state
                            .recording
                            .try_lock()
                            .map(|g| *g)
                            .unwrap_or(false);
                        if is_recording {
                            handle_toggle(&app_for_mouse);
                        }
                    }
                }) {
                    tracing::error!("Global mouse listener failed: {:?}", e);
                }
            });

            tracing::info!("Murmur app started");
            Ok(())
        })
        .run(tauri::generate_context!())
        .context("error while running Murmur")?;

    Ok(())
}
