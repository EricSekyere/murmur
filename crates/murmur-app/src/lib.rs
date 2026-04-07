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
    use murmur_core::audio::silence::{compute_rms, downmix_to_mono};
    use murmur_core::dictation::{DictationConfig, DictationEvent, DictationSession};
    use std::sync::Mutex;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};
    use tauri::Emitter;

    enum Cmd {
        StartStreaming {
            audio_device: Option<String>,
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
        /// Periodic RMS level update (0.0 - 1.0 range, sent every ~200ms).
        AudioLevel(f32),
        SignalDetected,
        NoSignal(String),
        StreamingDone,
    }

    pub struct Handle {
        cmd_tx: mpsc::Sender<Cmd>,
        result_rx: Mutex<mpsc::Receiver<AudioResult>>,
    }

    impl Handle {
        pub fn spawn(app_handle: tauri::AppHandle) -> Self {
            let (cmd_tx, cmd_rx) = mpsc::channel::<Cmd>();
            let (result_tx, result_rx) = mpsc::channel::<AudioResult>();

            std::thread::spawn(move || {
                let worker_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let mut capture = match AudioCapture::new() {
                        Ok(c) => c,
                        Err(e) => {
                            tracing::error!("Failed to create AudioCapture: {}", e);
                            let _ = result_tx.send(AudioResult::StartFailed(e.to_string()));
                            return;
                        }
                    };

                    while let Ok(cmd) = cmd_rx.recv() {
                        match cmd {
                            Cmd::StartStreaming {
                                audio_device,
                                rms_threshold,
                                phrase_pause,
                                session_timeout,
                            } => {
                                // Wrap capture.start() in catch_unwind — CPAL's native
                                // audio backend (WASAPI) can panic/crash on some drivers.
                                let start_result =
                                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                        capture.start(audio_device.as_deref())
                                    }));
                                match start_result {
                                    Ok(Err(e)) => {
                                        let _ =
                                            result_tx.send(AudioResult::StartFailed(e.to_string()));
                                        continue;
                                    }
                                    Err(panic_info) => {
                                        let msg =
                                            if let Some(s) = panic_info.downcast_ref::<String>() {
                                                s.clone()
                                            } else {
                                                "native audio panic".to_string()
                                            };
                                        tracing::error!("Audio capture panicked on start: {}", msg);
                                        let _ = result_tx.send(AudioResult::StartFailed(msg));
                                        continue;
                                    }
                                    Ok(Ok(())) => {}
                                }
                                let _ = result_tx.send(AudioResult::Started);

                                let live_buf = capture.live_buffer();
                                let native_rate = capture.native_rate();
                                let native_channels = capture.native_channels();
                                tracing::info!(
                                    "Audio stream started: native_rate={}Hz, native_channels={}",
                                    native_rate,
                                    native_channels,
                                );
                                let mut analyzed_up_to = 0usize;

                                // ── Calibration phase: measure ambient noise ──
                                // Wait for WASAPI/CPAL to actually begin streaming before
                                // measuring. Audio backends often need 100-200ms to start
                                // producing samples after capture.start() returns.
                                std::thread::sleep(Duration::from_millis(200));

                                let calibration_duration = Duration::from_millis(400);
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
                                        let mono = downmix_to_mono(
                                            &buf[analyzed_up_to..],
                                            native_channels,
                                        );
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

                                // Compute mic gain to compensate for quiet mics.
                                // Common causes of low signal: mono mic reported as stereo
                                // (downmix halves amplitude), low Windows mic volume, distant
                                // USB mic. We boost the signal so the PhraseDetector, UI level
                                // indicator, and STT engine all receive usable audio levels.
                                //
                                // Compute mic gain to bring quiet microphones into a usable
                                // range. Many laptop mics (especially when reported as stereo
                                // and downmixed to mono) produce very low ambient RMS like
                                // 0.01, with speech peaks only reaching 0.02-0.05. We need
                                // to boost so the PhraseDetector's threshold can separate
                                // speech from silence.
                                //
                                // Cap at 5x to prevent clipping distortion that causes
                                // Whisper hallucinations. Target effective ambient ≈ 0.02.
                                let mic_gain = if ambient_rms > 0.0001 && ambient_rms < 0.02 {
                                    // Quiet mic: boost so effective ambient ≈ 0.02
                                    (0.02 / ambient_rms).min(5.0)
                                } else if ambient_rms <= 0.0001 {
                                    // Calibration captured near-zero signal. The mic may not
                                    // have started streaming yet or is truly silent. Apply
                                    // moderate gain — real speech will be much louder than
                                    // digital silence.
                                    3.0
                                } else {
                                    1.0
                                };

                                // Compute threshold using the gained (effective) ambient level.
                                // The threshold must be high enough to reject silence/noise
                                // but low enough to detect quiet speech on laptop mics.
                                // Use 1.8x ambient (was 2.5x) so speech just above the noise
                                // floor is still detected, with a lower upper clamp.
                                let effective_ambient = ambient_rms * mic_gain;
                                let calibrated_threshold = if rms_threshold > 0.0 {
                                    rms_threshold
                                } else {
                                    (effective_ambient * 1.8).clamp(0.001, 0.015)
                                };
                                tracing::info!(
                                    "Calibrated: ambient RMS = {:.6}, mic_gain = {:.1}x, effective ambient = {:.6}, threshold = {:.6} (config = {:.6}, mode = {})",
                                    ambient_rms,
                                    mic_gain,
                                    effective_ambient,
                                    calibrated_threshold,
                                    rms_threshold,
                                    if rms_threshold > 0.0 {
                                        "manual"
                                    } else {
                                        "auto"
                                    }
                                );

                                // Do not throw away the entire calibration buffer. On Windows,
                                // users often start speaking immediately after pressing the
                                // hotkey; draining everything here discards the start of the
                                // utterance and can lead to "No speech detected".
                                // Keep up to ~1s of preroll so phrase detection and STT can
                                // recover the beginning of the sentence.
                                {
                                    let mut buf =
                                        live_buf.lock().unwrap_or_else(|e| e.into_inner());
                                    let preroll_samples =
                                        (native_rate as usize) * (native_channels as usize);
                                    if analyzed_up_to > preroll_samples {
                                        let drop_count = analyzed_up_to - preroll_samples;
                                        buf.drain(..drop_count);
                                        analyzed_up_to -= drop_count;
                                    }
                                }

                                let mut session = DictationSession::new(
                                    DictationConfig {
                                        speech_threshold: calibrated_threshold,
                                        silence_hold: phrase_pause,
                                        session_timeout,
                                        ..DictationConfig::default()
                                    },
                                    native_rate,
                                );
                                let mut level_tick: u32 = 0;
                                let mut saw_signal = false;
                                let startup_deadline = Instant::now() + Duration::from_millis(1200);
                                let mut warned_no_samples = false;
                                let silence_deadline = Instant::now() + Duration::from_secs(3);
                                let mut warned_digital_silence = false;

                                loop {
                                    // Check for manual Stop
                                    if let Ok(Cmd::Stop) = cmd_rx.try_recv() {
                                        // Feed any remaining samples through the dictation
                                        // session before finalizing so phrase boundaries stay
                                        // consistent with the live path.
                                        let remaining = {
                                            let mut buf =
                                                live_buf.lock().unwrap_or_else(|e| e.into_inner());
                                            if buf.len() > analyzed_up_to {
                                                let mono = downmix_to_mono(
                                                    &buf[analyzed_up_to..],
                                                    native_channels,
                                                );
                                                buf.clear();
                                                Some(if mic_gain > 1.0 {
                                                    mono.iter()
                                                        .map(|s| (s * mic_gain).clamp(-1.0, 1.0))
                                                        .collect::<Vec<f32>>()
                                                } else {
                                                    mono
                                                })
                                            } else {
                                                buf.clear();
                                                None
                                            }
                                        };

                                        if let Some(mono) = remaining {
                                            for event in session.ingest(&mono) {
                                                if let DictationEvent::PhraseReady(audio) = event
                                                    && !audio.samples.is_empty()
                                                {
                                                    let _ = result_tx
                                                        .send(AudioResult::PhraseReady(audio));
                                                }
                                            }
                                        }

                                        if let Some(audio) = session.finish()
                                            && !audio.samples.is_empty()
                                        {
                                            let _ = result_tx.send(AudioResult::PhraseReady(audio));
                                        }

                                        if let Err(e) = capture.stop() {
                                            tracing::error!(
                                                "Failed to stop audio capture on manual stop: {}",
                                                e
                                            );
                                        }
                                        let _ = result_tx.send(AudioResult::StreamingDone);
                                        break;
                                    }
                                    // Read new samples, downmix to mono, apply mic gain, feed to phrase detector
                                    let (events, chunk_rms, saw_new_samples) = {
                                        let mut buf =
                                            live_buf.lock().unwrap_or_else(|e| e.into_inner());
                                        if buf.len() > analyzed_up_to {
                                            let mono = downmix_to_mono(
                                                &buf[analyzed_up_to..],
                                                native_channels,
                                            );
                                            // Apply mic gain so PhraseDetector and UI see proper levels
                                            let mono = if mic_gain > 1.0 {
                                                mono.iter()
                                                    .map(|s| (s * mic_gain).clamp(-1.0, 1.0))
                                                    .collect::<Vec<f32>>()
                                            } else {
                                                mono
                                            };
                                            let rms = compute_rms(&mono);
                                            analyzed_up_to = buf.len();

                                            // If we are still waiting for speech, discard old audio to prevent memory
                                            // growth and avoid sending minutes of silence to the STT engine.
                                            // Keep ~1 second of pre-roll history.
                                            if rms < calibrated_threshold {
                                                let preroll_samples = (native_rate as usize)
                                                    * (native_channels as usize);
                                                if buf.len() > preroll_samples {
                                                    let drop_count = buf.len() - preroll_samples;
                                                    buf.drain(..drop_count);
                                                    analyzed_up_to -= drop_count;
                                                }
                                            }

                                            (session.ingest(&mono), rms, true)
                                        } else {
                                            (Vec::new(), 0.0, false)
                                        }
                                    };

                                    if saw_new_samples && chunk_rms > 0.002 && !saw_signal {
                                        saw_signal = true;
                                        let _ = result_tx.send(AudioResult::SignalDetected);
                                    }

                                    if !saw_signal
                                        && Instant::now() >= startup_deadline
                                        && !saw_new_samples
                                        && !warned_no_samples
                                    {
                                        warned_no_samples = true;
                                        tracing::warn!(
                                            "Audio stream started but no microphone samples arrived within startup window"
                                        );
                                    }

                                    if !saw_signal
                                        && Instant::now() >= silence_deadline
                                        && saw_new_samples
                                        && !warned_digital_silence
                                    {
                                        warned_digital_silence = true;
                                        let msg = "Microphone stream is delivering digital silence. Check Windows microphone permissions, input device, mute switch, and input volume.".to_string();
                                        tracing::warn!("{}", msg);
                                        let _ = result_tx.send(AudioResult::NoSignal(msg));
                                    }

                                    // Emit audio level every ~100ms (every 2nd tick)
                                    level_tick += 1;
                                    if level_tick.is_multiple_of(2) {
                                        let _ = result_tx.send(AudioResult::AudioLevel(chunk_rms));
                                    }

                                    for event in events {
                                        match event {
                                            DictationEvent::Level(_) => {}
                                            DictationEvent::ActivityDetected => {
                                                if !saw_signal {
                                                    saw_signal = true;
                                                    let _ =
                                                        result_tx.send(AudioResult::SignalDetected);
                                                }
                                            }
                                            DictationEvent::PhraseReady(audio) => {
                                                if !audio.samples.is_empty() {
                                                    let _ = result_tx
                                                        .send(AudioResult::PhraseReady(audio));
                                                }
                                            }
                                            DictationEvent::SessionTimeout => {
                                                tracing::info!(
                                                    "Streaming session timeout — no speech"
                                                );
                                                if let Err(e) = capture.stop() {
                                                    tracing::error!(
                                                        "Failed to stop audio capture on session timeout: {}",
                                                        e
                                                    );
                                                }
                                                let _ = result_tx.send(AudioResult::StreamingDone);
                                                break;
                                            }
                                        }
                                    }

                                    std::thread::sleep(Duration::from_millis(50));
                                }
                            }

                            Cmd::Stop => {
                                tracing::debug!("Stop received outside monitoring loop, ignoring");
                            }
                        }
                    }
                }));

                if let Err(panic_info) = worker_result {
                    let msg = if let Some(s) = panic_info.downcast_ref::<String>() {
                        s.clone()
                    } else if let Some(s) = panic_info.downcast_ref::<&str>() {
                        (*s).to_string()
                    } else {
                        "unknown panic in audio worker thread".to_string()
                    };
                    tracing::error!("Audio worker thread panicked: {}", msg);
                    let _ = result_tx.send(AudioResult::StartFailed(format!(
                        "Audio worker crash: {}",
                        msg
                    )));

                    // Since the audio worker is critical and shouldn't crash,
                    // emit an error that can be shown in the UI.
                    let _ = app_handle.emit(
                        "hotkey-error",
                        serde_json::json!({ "error": format!("Audio driver crashed: {}", msg) }),
                    );
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
            audio_device: Option<String>,
            rms_threshold: f32,
            phrase_pause: Duration,
            session_timeout: Duration,
        ) -> Result<(), String> {
            // Drain stale results from the previous session so they do not race
            // with the next StartStreaming handshake.
            if let Ok(rx) = self.result_rx.lock() {
                while rx.try_recv().is_ok() {}
            }

            self.cmd_tx
                .send(Cmd::StartStreaming {
                    audio_device,
                    rms_threshold,
                    phrase_pause,
                    session_timeout,
                })
                .map_err(|e| format!("Audio worker channel closed: {}", e))?;

            let rx = self.result_rx.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                match rx.recv_timeout(Duration::from_secs(5)) {
                    Ok(AudioResult::Started) => return Ok(()),
                    Ok(AudioResult::StartFailed(e)) => return Err(e),
                    Ok(AudioResult::PhraseReady(_))
                    | Ok(AudioResult::AudioLevel(_))
                    | Ok(AudioResult::SignalDetected)
                    | Ok(AudioResult::NoSignal(_))
                    | Ok(AudioResult::StreamingDone) => {
                        tracing::debug!(
                            "Ignoring stale audio worker result during start handshake"
                        );
                    }
                    Err(e) => return Err(format!("Audio worker response timeout: {}", e)),
                }
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
    audio: std::sync::OnceLock<audio_worker::Handle>,
    engine: Arc<Mutex<Option<SttEngine>>>,
    /// Lock-free flag: true once the STT engine has been initialized.
    /// Avoids blocking the UI thread on the engine mutex in handle_toggle.
    engine_loaded: std::sync::atomic::AtomicBool,
    recording: Mutex<bool>,
    settings: Mutex<Settings>,
    last_toggle: Mutex<Instant>,
    /// The foreground window that was active when recording started.
    /// Used to restore focus before outputting text, so keystrokes go
    /// to the user's target app instead of the Murmur window.
    #[cfg(windows)]
    previous_foreground: Mutex<usize>,
    /// Last foreground window that was not owned by this process.
    #[cfg(windows)]
    last_external_foreground: Mutex<usize>,
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
    ram_estimate_mb: u32,
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
    let (
        model_name,
        model_id,
        hotkey,
        audio_device,
        output_mode,
        developer_mode,
        phrase_pause_secs,
        session_timeout_secs,
        click_to_stop,
        show_widget,
    ) = {
        let settings = state.settings.lock().unwrap_or_else(|e| e.into_inner());
        (
            settings.model.name().to_string(),
            settings.model.id().to_string(),
            settings.hotkey.clone(),
            settings.audio_device.clone(),
            settings.output_mode,
            settings.developer_mode,
            settings.phrase_pause_secs,
            settings.session_timeout_secs,
            settings.click_to_stop,
            settings.show_widget,
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
        "audio_device": audio_device,
        "output_mode": output_mode,
        "developer_mode": developer_mode,
        "phrase_pause_secs": phrase_pause_secs,
        "session_timeout_secs": session_timeout_secs,
        "click_to_stop": click_to_stop,
        "show_widget": show_widget,
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
            ram_estimate_mb: model.ram_estimate_mb(),
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

    // Stop recording first if active — changing models while recording would
    // silently drop all phrases since the engine becomes None.
    {
        let is_recording = *state.recording.lock().unwrap_or_else(|e| e.into_inner());
        if is_recording {
            tracing::info!("Stopping recording before model change");
            handle_toggle(&app);
        }
    }

    // Clear current engine
    {
        let mut guard = state.engine.lock().unwrap_or_else(|e| e.into_inner());
        *guard = None;
    }
    state
        .engine_loaded
        .store(false, std::sync::atomic::Ordering::Release);

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
    tracing::info!(
        "Developer mode {}",
        if enabled { "enabled" } else { "disabled" }
    );
    Ok(())
}

/// Tauri command: update one or more settings fields and persist to config.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
fn update_settings(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    hotkey: Option<String>,
    audio_device: Option<String>,
    output_mode: Option<String>,
    phrase_pause_secs: Option<f32>,
    session_timeout_secs: Option<f32>,
    click_to_stop: Option<bool>,
    show_widget: Option<bool>,
) -> Result<(), String> {
    let mut settings = state.settings.lock().unwrap_or_else(|e| e.into_inner());

    // Hotkey: validate, update, re-register
    if let Some(ref new_hotkey) = hotkey {
        let trimmed = new_hotkey.trim().to_string();
        if trimmed.is_empty() {
            return Err("Hotkey cannot be empty".to_string());
        }
        // Validate it parses before accepting
        let new_shortcut: tauri_plugin_global_shortcut::Shortcut = trimmed
            .parse()
            .map_err(|e| format!("Invalid hotkey '{}': {:?}", trimmed, e))?;

        let old_shortcut = settings
            .hotkey
            .parse::<tauri_plugin_global_shortcut::Shortcut>()
            .ok();

        // If old and new are different shortcuts, unregister old first to free the binding.
        // If they're the same, unregister + re-register to refresh.
        if let Some(ref old) = old_shortcut {
            let _ = app.global_shortcut().unregister(*old);
        }

        if let Err(e) = app.global_shortcut().register(new_shortcut) {
            // Registration failed — try to restore the old hotkey so the user
            // isn't left with no working hotkey.
            if let Some(ref old) = old_shortcut {
                let _ = app.global_shortcut().register(*old);
            }
            return Err(format!("Failed to register hotkey '{}': {:?}", trimmed, e));
        }

        settings.hotkey = trimmed;
        tracing::info!("Hotkey updated to: {}", settings.hotkey);
    }

    if let Some(ref mode_str) = output_mode {
        let mode = match mode_str.as_str() {
            "auto" => murmur_core::output::OutputMode::Auto,
            "clipboard" => murmur_core::output::OutputMode::Clipboard,
            "keyboard" => murmur_core::output::OutputMode::Keyboard,
            "clipboard_paste" => murmur_core::output::OutputMode::ClipboardPaste,
            // Keep backward compatibility: map stdout → auto for desktop app.
            "stdout" => murmur_core::output::OutputMode::Auto,
            _ => return Err(format!("Unknown output mode: {}", mode_str)),
        };
        settings.output_mode = mode;
        tracing::info!("Output mode updated to: {:?}", mode);
    }

    if let Some(device_name) = audio_device {
        let trimmed = device_name.trim();
        settings.audio_device = if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        };
        tracing::info!(
            "Audio device updated to: {}",
            settings
                .audio_device
                .as_deref()
                .unwrap_or("<system default>")
        );
    }

    if let Some(pp) = phrase_pause_secs {
        if !(0.3..=10.0).contains(&pp) {
            return Err(format!(
                "phrase_pause_secs must be between 0.3 and 10.0, got {}",
                pp
            ));
        }
        settings.phrase_pause_secs = pp;
    }

    if let Some(st) = session_timeout_secs {
        if !(0.0..=300.0).contains(&st) {
            return Err(format!(
                "session_timeout_secs must be between 0 and 300, got {}",
                st
            ));
        }
        settings.session_timeout_secs = st;
    }

    if let Some(cts) = click_to_stop {
        settings.click_to_stop = cts;
    }

    if let Some(sw) = show_widget {
        settings.show_widget = sw;
        // Show/hide widget window immediately
        if let Some(widget) = app.get_webview_window("widget") {
            if sw {
                let _ = widget.show();
            } else {
                let _ = widget.hide();
            }
        }
    }

    if let Ok(path) = Settings::default_path() {
        settings.save(&path).map_err(|e| e.to_string())?;
    }

    Ok(())
}

/// Tauri command: list available audio input devices.
#[tauri::command]
fn list_audio_devices() -> Result<Vec<String>, String> {
    use cpal::traits::{DeviceTrait, HostTrait};
    let host = cpal::default_host();
    let mut names = Vec::new();

    let default_name = host
        .default_input_device()
        .and_then(|d| d.name().ok())
        .unwrap_or_default();

    // Put default device first
    if !default_name.is_empty() {
        names.push(default_name.clone());
    }

    if let Ok(devices) = host.input_devices() {
        for device in devices {
            if let Ok(name) = device.name()
                && name != default_name
            {
                names.push(name);
            }
        }
    }

    Ok(names)
}

/// Tauri command: toggle widget window visibility.
#[tauri::command]
fn set_widget_visible(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    visible: bool,
) -> Result<(), String> {
    if let Some(widget) = app.get_webview_window("widget") {
        if visible {
            let _ = widget.show();
        } else {
            let _ = widget.hide();
        }
    }
    let mut settings = state.settings.lock().unwrap_or_else(|e| e.into_inner());
    settings.show_widget = visible;
    if let Ok(path) = Settings::default_path() {
        let _ = settings.save(&path);
    }
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
    let model_for_hint = model;
    let stt = tokio::task::spawn_blocking(move || {
        let mut engine = match backend {
            Backend::Whisper => SttEngine::new_whisper(&path_str, 0),
            Backend::Parakeet => SttEngine::new_parakeet(&path_str),
        }?;
        // Set model hint so the engine can tune parameters per model size
        // (e.g., temperature fallback for larger models).
        engine.set_model(model_for_hint);
        Ok::<SttEngine, anyhow::Error>(engine)
    })
    .await
    .context("Engine init task panicked")?
    .context("Failed to initialize STT engine")?;

    {
        let mut guard = engine.lock().unwrap_or_else(|e| e.into_inner());
        *guard = Some(stt);
    }

    // Set the lock-free engine_loaded flag so handle_toggle never blocks
    if let Some(app_state) = app.try_state::<AppState>() {
        app_state
            .engine_loaded
            .store(true, std::sync::atomic::Ordering::Release);
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
/// Restores focus to the window that was active when recording started,
/// then delegates to `murmur_core::output::dispatch_output` which handles the
/// full fallback chain: Auto → keyboard → clipboard+paste → clipboard.
fn output_text(
    text: &str,
    mode: murmur_core::output::OutputMode,
    #[cfg(windows)] previous_hwnd: usize,
) -> anyhow::Result<()> {
    #[cfg(windows)]
    restore_foreground_window(previous_hwnd);

    murmur_core::output::dispatch_output(text, mode)
}

/// Restore focus to the window the user was working in before recording.
#[cfg(windows)]
fn restore_foreground_window(hwnd: usize) {
    if hwnd == 0 {
        return;
    }

    unsafe extern "system" {
        fn SetForegroundWindow(hwnd: usize) -> i32;
        fn GetForegroundWindow() -> usize;
    }

    let current = unsafe { GetForegroundWindow() };
    if current == hwnd {
        return; // Already focused
    }

    tracing::info!(
        "Restoring focus: current=0x{:x} -> target=0x{:x}",
        current,
        hwnd
    );

    let result = unsafe { SetForegroundWindow(hwnd) };
    if result == 0 {
        tracing::warn!("SetForegroundWindow failed for hwnd=0x{:x}", hwnd);
    }

    // Brief pause for the focus change to take effect
    std::thread::sleep(std::time::Duration::from_millis(50));
}

#[cfg(windows)]
fn current_process_id() -> u32 {
    std::process::id()
}

#[cfg(windows)]
fn foreground_window() -> usize {
    unsafe extern "system" {
        fn GetForegroundWindow() -> usize;
    }

    unsafe { GetForegroundWindow() }
}

#[cfg(windows)]
fn window_process_id(hwnd: usize) -> Option<u32> {
    if hwnd == 0 {
        return None;
    }

    unsafe extern "system" {
        fn GetWindowThreadProcessId(hwnd: usize, lpdw_process_id: *mut u32) -> u32;
    }

    let mut process_id = 0u32;
    unsafe {
        GetWindowThreadProcessId(hwnd, &mut process_id);
    }

    (process_id != 0).then_some(process_id)
}

#[cfg(windows)]
fn is_own_window(hwnd: usize) -> bool {
    window_process_id(hwnd) == Some(current_process_id())
}

#[cfg(windows)]
fn save_output_target_window(state: &AppState) {
    let foreground = foreground_window();
    let target = if foreground != 0 && !is_own_window(foreground) {
        *state
            .last_external_foreground
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = foreground;
        foreground
    } else {
        *state
            .last_external_foreground
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    };

    *state
        .previous_foreground
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = target;

    tracing::info!(
        "Saved output target window: foreground=0x{:x}, target=0x{:x}, foreground_is_own={}",
        foreground,
        target,
        foreground != 0 && is_own_window(foreground)
    );
}

#[cfg(windows)]
fn spawn_foreground_tracker(app: tauri::AppHandle) {
    std::thread::spawn(move || {
        loop {
            let hwnd = foreground_window();
            if hwnd != 0
                && !is_own_window(hwnd)
                && let Some(state) = app.try_state::<AppState>()
            {
                *state
                    .last_external_foreground
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()) = hwnd;
            }

            std::thread::sleep(Duration::from_millis(150));
        }
    });
}

/// Normalize very quiet audio so the STT engine can process it effectively.
///
/// If peak amplitude is below the threshold, scales samples so peak reaches
/// the target level. Caps the gain factor to avoid amplifying noise.
fn normalize_peak(samples: &[f32]) -> Vec<f32> {
    let peak = samples.iter().map(|s| s.abs()).fold(0.0_f32, f32::max);

    if peak < 0.1 && peak > 0.0 {
        // Cap gain at 5x to avoid amplifying noise floor excessively.
        let scale = (0.5 / peak).min(5.0);
        tracing::debug!(
            "Normalizing quiet audio: peak {:.4} -> {:.4} (scale {:.2}x)",
            peak,
            peak * scale,
            scale
        );
        samples
            .iter()
            .map(|s| (s * scale).clamp(-1.0, 1.0))
            .collect()
    } else {
        samples.to_vec()
    }
}

/// Trim leading and trailing silence from 16kHz mono audio.
///
/// Removes silent frames from both ends so the STT engine doesn't waste
/// capacity on dead air (which causes hallucinations, especially in larger models).
/// Keeps a small pre-roll buffer (~50ms) before the first speech frame.
fn trim_silence(samples: &[f32]) -> &[f32] {
    const FRAME_SIZE: usize = 512; // ~32ms at 16kHz
    const TRIM_THRESHOLD: f32 = 0.005;
    const PREROLL_FRAMES: usize = 2; // ~64ms of context before speech

    if samples.len() < FRAME_SIZE {
        return samples;
    }

    let frames: Vec<f32> = samples
        .chunks(FRAME_SIZE)
        .map(|chunk| {
            let sum_sq: f32 = chunk.iter().map(|&s| s * s).sum();
            (sum_sq / chunk.len() as f32).sqrt()
        })
        .collect();

    // Find first frame with speech
    let first_speech = frames
        .iter()
        .position(|&rms| rms >= TRIM_THRESHOLD)
        .unwrap_or(0);

    // Find last frame with speech
    let last_speech = frames
        .iter()
        .rposition(|&rms| rms >= TRIM_THRESHOLD)
        .unwrap_or(frames.len().saturating_sub(1));

    // Keep a small pre-roll before first speech
    let start = first_speech.saturating_sub(PREROLL_FRAMES) * FRAME_SIZE;
    // Keep the frame after last speech (partial frame at end)
    let end = ((last_speech + 1) * FRAME_SIZE).min(samples.len());

    if start >= end {
        return &samples[..0];
    }

    &samples[start..end]
}

/// Transcribe an audio buffer and return the text, or None if empty/error.
///
/// All rejection/error paths log at `error` or `warn` level and emit a
/// `transcription-error` event to the frontend so failures are visible.
fn transcribe_chunk(
    app: &tauri::AppHandle,
    audio: &murmur_core::audio::AudioBuffer,
) -> Option<(String, u64)> {
    tracing::info!(
        "transcribe_chunk: {} samples ({:.2}s)",
        audio.samples.len(),
        audio.samples.len() as f32 / 16000.0
    );

    if audio.samples.is_empty() {
        let msg = "Empty audio buffer — nothing to transcribe";
        tracing::error!("{}", msg);
        emit_transcription_error(app, msg);
        return None;
    }

    // Skip very short chunks that produce garbage/hallucinations
    const MIN_AUDIO_SECS: f32 = 0.15;
    // Cap audio length to keep inference latency bounded
    const MAX_AUDIO_SAMPLES: usize = 20 * 16_000;
    let samples = if audio.samples.len() > MAX_AUDIO_SAMPLES {
        tracing::warn!(
            "Truncating audio from {:.1}s to 20s (large chunk)",
            audio.samples.len() as f32 / 16000.0
        );
        &audio.samples[audio.samples.len() - MAX_AUDIO_SAMPLES..]
    } else {
        &audio.samples
    };

    let raw_duration_secs = samples.len() as f32 / 16000.0;

    if raw_duration_secs < MIN_AUDIO_SECS {
        let msg = format!(
            "Audio too short ({:.2}s < {:.1}s minimum) — skipping",
            raw_duration_secs, MIN_AUDIO_SECS
        );
        tracing::warn!("{}", msg);
        emit_transcription_error(app, &msg);
        return None;
    }

    // Preprocessing pipeline:
    // 1. Trim leading/trailing silence to reduce hallucinations
    // 2. Normalize quiet audio so the STT engine can process it
    let trimmed = trim_silence(samples);
    let trimmed_duration = trimmed.len() as f32 / 16000.0;

    if trimmed_duration < MIN_AUDIO_SECS {
        let msg = format!(
            "Audio too short after silence trim ({:.2}s < {:.1}s, raw was {:.2}s) — skipping",
            trimmed_duration, MIN_AUDIO_SECS, raw_duration_secs,
        );
        tracing::warn!("{}", msg);
        emit_transcription_error(app, &msg);
        return None;
    }

    let processed = normalize_peak(trimmed);
    let duration_secs = processed.len() as f32 / 16000.0;
    tracing::info!(
        "Audio preprocessing: {:.2}s raw -> {:.2}s trimmed -> {:.2}s normalized ({} samples)",
        raw_duration_secs,
        trimmed_duration,
        duration_secs,
        processed.len()
    );

    // Compute audio stats for diagnostics
    let peak = processed.iter().map(|s| s.abs()).fold(0.0_f32, f32::max);
    let rms = {
        let sum_sq: f32 = processed.iter().map(|s| s * s).sum();
        (sum_sq / processed.len() as f32).sqrt()
    };
    tracing::info!(
        "Audio stats: peak={:.4}, rms={:.4}, duration={:.2}s",
        peak,
        rms,
        duration_secs
    );

    // Skip near-silent audio that would make Whisper take 30-40+ seconds
    // trying to decode noise, producing hallucinations. A real speaking
    // voice should have peak > 0.05 even on a quiet laptop mic.
    const MIN_PEAK: f32 = 0.015;
    const MIN_RMS: f32 = 0.0015;
    if peak < MIN_PEAK || rms < MIN_RMS {
        let msg = format!(
            "Audio too quiet (peak={:.4} < {}, rms={:.4} < {}) — mic may not be picking up speech. \
             Try increasing your microphone volume in Windows Sound Settings.",
            peak, MIN_PEAK, rms, MIN_RMS,
        );
        tracing::warn!("{}", msg);
        emit_transcription_error(app, &msg);
        return None;
    }

    let state = app.state::<AppState>();

    let developer_mode = state
        .settings
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .developer_mode;

    let mut engine_guard = state.engine.lock().unwrap_or_else(|e| e.into_inner());
    let engine = match engine_guard.as_mut() {
        Some(e) => e,
        None => {
            let msg = "STT engine not initialized — cannot transcribe";
            tracing::error!("{}", msg);
            emit_transcription_error(app, msg);
            return None;
        }
    };

    tracing::info!(
        "Starting transcription of {:.2}s audio (model: {})",
        duration_secs,
        engine.model_path()
    );

    // Known Whisper hallucination phrases produced on silence/noise.
    // Larger models (medium, large-v3-turbo) produce a wider variety.
    const HALLUCINATIONS: &[&str] = &[
        "thank you",
        "thank you for watching",
        "thanks for watching",
        "thanks for listening",
        "please subscribe",
        "like and subscribe",
        "see you next time",
        "see you in the next video",
        "subtitles by",
        "subtitle",
        "share this video",
        "don't forget to subscribe",
        "bye",
        "goodbye",
        "you",
        "the end",
        "so",
    ];

    let transcribe_result = engine.transcribe(&processed);

    match &transcribe_result {
        Ok(r) => tracing::info!(
            "Engine returned: text={:?} ({}ms, {} segments)",
            if r.text.is_empty() {
                "<empty>"
            } else {
                &r.text
            },
            r.processing_time_ms,
            r.segments.len()
        ),
        Err(e) => tracing::error!("Engine transcribe call failed: {:#}", e),
    }

    match transcribe_result {
        Ok(result) if !result.text.is_empty() => {
            let text = if developer_mode {
                let processed = PostProcessor::process(&result.text);
                tracing::info!(
                    "Post-processed ({}ms): raw={:?} -> processed={:?}",
                    result.processing_time_ms,
                    result.text,
                    processed
                );
                processed
            } else {
                tracing::info!(
                    "Transcribed ({}ms): {:?}",
                    result.processing_time_ms,
                    result.text
                );
                result.text
            };
            if text.is_empty() {
                tracing::warn!("Transcription produced empty text after post-processing");
                return None;
            }

            // Filter known Whisper hallucinations
            let normalized = text.trim().trim_end_matches(['.', '!', '?']).to_lowercase();
            if HALLUCINATIONS.contains(&normalized.as_str()) {
                tracing::warn!("Filtered hallucination (exact match): {:?}", text);
                return None;
            }

            // Filter bracketed/asterisk patterns like "*laughs*", "*music*", "[music]"
            let stripped = normalized.trim();
            if (stripped.starts_with('*') && stripped.ends_with('*'))
                || (stripped.starts_with('[') && stripped.ends_with(']'))
            {
                tracing::warn!("Filtered hallucination (bracketed): {:?}", text);
                return None;
            }

            // Filter text that is only punctuation or whitespace
            if stripped
                .chars()
                .all(|c| c.is_ascii_punctuation() || c.is_whitespace())
            {
                tracing::warn!("Filtered hallucination (punctuation only): {:?}", text);
                return None;
            }

            // Filter repeated word patterns (e.g., "the the the", "I I I")
            {
                let words: Vec<&str> = stripped.split_whitespace().collect();
                if words.len() >= 3 && words.iter().all(|w| *w == words[0]) {
                    tracing::warn!("Filtered hallucination (repeated word): {:?}", text);
                    return None;
                }
            }

            // Filter very short single-character outputs (noise artifacts)
            if stripped.len() == 1 && !stripped.chars().next().unwrap().is_alphanumeric() {
                tracing::warn!("Filtered hallucination (single char): {:?}", text);
                return None;
            }

            tracing::info!("Transcription accepted: '{}' ({} chars)", text, text.len());
            Some((text, result.processing_time_ms))
        }
        Ok(result) => {
            let msg = format!(
                "Engine returned empty text ({}ms, {} samples, peak={:.4}, rms={:.4})",
                result.processing_time_ms,
                processed.len(),
                peak,
                rms,
            );
            tracing::error!("{}", msg);
            emit_transcription_error(app, &msg);
            None
        }
        Err(e) => {
            let msg = format!("Transcription engine error: {:#}", e);
            tracing::error!("{}", msg);
            emit_transcription_error(app, &msg);
            None
        }
    }
}

/// Emit a transcription error event to the frontend for visibility.
fn emit_transcription_error(app: &tauri::AppHandle, message: &str) {
    let _ = app.emit(
        "transcription-error",
        serde_json::json!({ "error": message }),
    );
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
        // Reflect stopped state in UI immediately. Cleanup will still happen
        // when streaming_worker receives StreamingDone.
        emit_recording_state(app, false, false);

        if let Err(e) = state.audio.get().expect("audio initialized").request_stop() {
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
        // Guard: engine not loaded (lock-free check — never blocks UI)
        if !state
            .engine_loaded
            .load(std::sync::atomic::Ordering::Acquire)
        {
            drop(recording);
            tracing::debug!("Toggle: engine not loaded yet");
            let _ = app.emit(
                "hotkey-error",
                serde_json::json!({ "error": "Model still loading — please wait" }),
            );
            return;
        }

        // Set recording immediately to prevent double-toggle
        *recording = true;
        drop(recording);

        // Save the current foreground window so we can restore focus
        // before outputting text. Without this, text gets typed into
        // the Murmur window if the user clicks its stop button.
        #[cfg(windows)]
        {
            save_output_target_window(&state);
        }

        tracing::info!("Toggle: start streaming");
        emit_recording_state(app, true, false);

        let app_handle = app.clone();
        std::thread::spawn(move || {
            let worker_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                streaming_worker(&app_handle);
            }));
            if let Err(panic_info) = worker_result {
                let msg = if let Some(s) = panic_info.downcast_ref::<String>() {
                    s.clone()
                } else if let Some(s) = panic_info.downcast_ref::<&str>() {
                    (*s).to_string()
                } else {
                    "unknown panic in streaming worker".to_string()
                };
                tracing::error!("Streaming worker panicked: {}", msg);
                if let Some(state) = app_handle.try_state::<AppState>() {
                    *state.recording.lock().unwrap_or_else(|e| e.into_inner()) = false;
                }
                emit_recording_state(&app_handle, false, false);
                let _ = app_handle.emit(
                    "hotkey-error",
                    serde_json::json!({ "error": format!("Recording crashed: {}", msg) }),
                );
            }
        });
    }
}

/// Background thread: streaming mode — detect phrases, transcribe each, type into focused field.
fn streaming_worker(app: &tauri::AppHandle) {
    let state = app.state::<AppState>();

    let (audio_device, rms_threshold, phrase_pause, session_timeout, output_mode, model_id) = {
        let settings = state.settings.lock().unwrap_or_else(|e| e.into_inner());
        (
            settings.audio_device.clone(),
            settings.silence_rms_threshold,
            Duration::from_secs_f32(settings.phrase_pause_secs),
            Duration::from_secs_f32(settings.session_timeout_secs),
            settings.output_mode,
            settings.model.id().to_string(),
        )
    };

    // Read the saved foreground window handle for focus restoration.
    #[cfg(windows)]
    let previous_hwnd = *state
        .previous_foreground
        .lock()
        .unwrap_or_else(|e| e.into_inner());

    tracing::info!(
        "streaming_worker starting: model={}, rms_threshold={}, phrase_pause={:?}, \
         session_timeout={:?}, output_mode={:?}, audio_device={:?}",
        model_id,
        rms_threshold,
        phrase_pause,
        session_timeout,
        output_mode,
        audio_device,
    );

    // Start streaming (blocks until audio capture is ready)
    if let Err(e) = state
        .audio
        .get()
        .expect("audio initialized")
        .start_streaming(audio_device, rms_threshold, phrase_pause, session_timeout)
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
    let mut had_transcription = false;
    loop {
        match state.audio.get().expect("audio initialized").recv_result() {
            Ok(audio_worker::AudioResult::PhraseReady(audio)) => {
                // We should still process this phrase if recording was JUST stopped.
                // The worker flushes the last chunk on stop.

                tracing::info!(
                    "Phrase ready: {} samples ({:.1}s of audio)",
                    audio.samples.len(),
                    audio.samples.len() as f32 / 16000.0
                );

                // If the user already stopped recording, show processing instead of
                // switching the UI back to a recording animation while the final
                // chunk is being transcribed.
                let still_recording = *state.recording.lock().unwrap_or_else(|e| e.into_inner());
                emit_recording_state(app, still_recording, true);

                let result = transcribe_chunk(app, &audio);
                if result.is_none() {
                    tracing::warn!(
                        "Phrase produced no transcription ({} samples, {:.1}s)",
                        audio.samples.len(),
                        audio.samples.len() as f32 / 16000.0
                    );
                }

                if let Some((ref text, processing_time_ms)) = result {
                    had_transcription = true;
                    // Always output valid transcription regardless of recording state.
                    // The phrase was already detected and transcribed; deliver it.
                    tracing::info!(
                        "Outputting transcription ({} chars) to {:?}",
                        text.len(),
                        output_mode
                    );
                    let output_result =
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            output_text(
                                text,
                                output_mode,
                                #[cfg(windows)]
                                previous_hwnd,
                            )
                        }));
                    match output_result {
                        Ok(Err(e)) => {
                            tracing::error!("Failed to output text: {}", e);
                            let _ = app.emit(
                                "hotkey-error",
                                serde_json::json!({ "error": format!("Failed to output text: {}", e) }),
                            );
                        }
                        Err(panic_info) => {
                            let msg = if let Some(s) = panic_info.downcast_ref::<String>() {
                                s.clone()
                            } else {
                                "unknown panic in output_text".to_string()
                            };
                            tracing::error!("output_text panicked: {}", msg);
                            let _ = app.emit(
                                "hotkey-error",
                                serde_json::json!({ "error": format!("Output crashed: {}", msg) }),
                            );
                        }
                        Ok(Ok(())) => {}
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
            Ok(audio_worker::AudioResult::AudioLevel(rms)) => {
                let _ = app.emit("audio-level", rms);
            }
            Ok(audio_worker::AudioResult::SignalDetected) => {
                let _ = app.emit("audio-signal-detected", serde_json::json!({}));
            }
            Ok(audio_worker::AudioResult::NoSignal(message)) => {
                let _ = app.emit(
                    "transcription-error",
                    serde_json::json!({ "error": message }),
                );
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

    // Notify user if no transcription was produced during the session
    if !had_transcription {
        let _ = app.emit(
            "hotkey-error",
            serde_json::json!({ "error": "No speech detected — check your microphone input" }),
        );
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

fn show_widget_window(app: &tauri::AppHandle) {
    if let Some(widget) = app.get_webview_window("widget") {
        let _ = widget.show();
    }
}

#[cfg(any(windows, target_os = "macos"))]
fn is_platform_double_tap_modifier(key: rdev::Key) -> bool {
    #[cfg(windows)]
    {
        matches!(key, rdev::Key::ControlLeft | rdev::Key::ControlRight)
    }

    #[cfg(target_os = "macos")]
    {
        matches!(key, rdev::Key::MetaLeft | rdev::Key::MetaRight)
    }
}

#[cfg(any(windows, target_os = "macos"))]
fn handle_double_modifier_tap(app: &tauri::AppHandle, last_tap: &mut Option<Instant>) {
    let now = Instant::now();
    let is_double_tap = last_tap
        .map(|last| now.duration_since(last) <= Duration::from_millis(450))
        .unwrap_or(false);

    if is_double_tap {
        *last_tap = None;
        handle_toggle(app);
        show_widget_window(app);
    } else {
        *last_tap = Some(now);
    }
}

fn spawn_global_input_listener(app: tauri::AppHandle) {
    std::thread::spawn(move || {
        tracing::info!("Starting global input listener");
        let mut last_modifier_tap: Option<Instant> = None;

        if let Err(e) = rdev::listen(move |event| {
            let _ =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match event.event_type {
                    #[cfg(any(windows, target_os = "macos"))]
                    rdev::EventType::KeyRelease(key) if is_platform_double_tap_modifier(key) => {
                        handle_double_modifier_tap(&app, &mut last_modifier_tap);
                    }
                    rdev::EventType::ButtonPress(
                        rdev::Button::Left | rdev::Button::Right | rdev::Button::Middle,
                    ) => {
                        let state = app.state::<AppState>();
                        let click_to_stop = state
                            .settings
                            .try_lock()
                            .map(|s| s.click_to_stop)
                            .unwrap_or(false);
                        if !click_to_stop {
                            return;
                        }
                        let is_recording = state.recording.try_lock().map(|g| *g).unwrap_or(false);
                        if is_recording {
                            handle_toggle(&app);
                        }
                    }
                    _ => {}
                }));
        }) {
            tracing::error!("Global input listener failed: {:?}", e);
        }
    });
}

// ─── Tray & App Setup ────────────────────────────────────────────────────────

/// 1x1 transparent PNG used as fallback tray icon.
fn fallback_icon() -> Image<'static> {
    Image::new_owned(vec![0, 0, 0, 0], 1, 1)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() -> anyhow::Result<()> {
    // Set up file-based logging so release builds have visible logs.
    let log_dir = if let Ok(appdata) = std::env::var("APPDATA") {
        std::path::PathBuf::from(appdata).join("murmur")
    } else if let Ok(home) = std::env::var("HOME") {
        std::path::PathBuf::from(home)
            .join(".config")
            .join("murmur")
    } else {
        std::path::PathBuf::from(".")
    };
    let _ = std::fs::create_dir_all(&log_dir);

    let file_appender = tracing_appender::rolling::daily(&log_dir, "app");
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(non_blocking)
        .with_ansi(false)
        .init();

    Settings::migrate_from_voitex();
    let config_path = Settings::default_path().context("Failed to determine config path")?;
    let mut settings = Settings::load(&config_path).context("Failed to load settings")?;
    if settings.output_mode == murmur_core::output::OutputMode::Stdout {
        // Desktop app should not default to display-only mode because users
        // expect text to be delivered to the active app.
        settings.output_mode = murmur_core::output::OutputMode::Keyboard;
        if let Err(e) = settings.save(&config_path) {
            tracing::warn!("Failed to persist output_mode migration from stdout: {}", e);
        }
        tracing::info!("Migrated desktop output mode from stdout to keyboard");
    }

    let model = settings.model;

    // Always initialize engine in background to keep app startup instant.
    // The UI shows a "loading model" banner until the engine is ready.
    let engine: Arc<Mutex<Option<SttEngine>>> = Arc::new(Mutex::new(None));

    let hotkey_str = settings.hotkey.clone();
    let show_widget_on_start = settings.show_widget;

    tauri::Builder::default()
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(move |app, _shortcut, event| {
                    handle_hotkey_event(app, event.state);
                })
                .build(),
        )
        .manage(AppState {
            audio: std::sync::OnceLock::new(),
            engine: Arc::clone(&engine),
            engine_loaded: std::sync::atomic::AtomicBool::new(false),
            recording: Mutex::new(false),
            settings: Mutex::new(settings),
            last_toggle: Mutex::new(Instant::now() - Duration::from_secs(10)),
            #[cfg(windows)]
            previous_foreground: Mutex::new(0),
            #[cfg(windows)]
            last_external_foreground: Mutex::new(0),
        })
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
            update_settings,
            list_audio_devices,
            set_widget_visible,
        ])
        .setup(move |app| {
            // Initialize audio worker once we have the app handle
            let state = app.state::<AppState>();
            let handle = audio_worker::Handle::spawn(app.handle().clone());
            let _ = state.audio.set(handle);

            #[cfg(windows)]
            spawn_foreground_tracker(app.handle().clone());

            let show_i = MenuItem::with_id(app, "show", "Show", true, None::<&str>)?;
            let widget_i =
                MenuItem::with_id(app, "toggle_widget", "Toggle Widget", true, None::<&str>)?;
            let quit_i = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show_i, &widget_i, &quit_i])?;

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
                    "toggle_widget" => {
                        if let Some(widget) = app.get_webview_window("widget") {
                            if widget.is_visible().unwrap_or(false) {
                                let _ = widget.hide();
                            } else {
                                let _ = widget.show();
                            }
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
            // Use unregister_all first to clear any stale registrations from
            // a previous instance (e.g. after a force-kill that skipped cleanup).
            let _ = app.global_shortcut().unregister_all();

            match hotkey_str.parse::<tauri_plugin_global_shortcut::Shortcut>() {
                Ok(shortcut) => match app.global_shortcut().register(shortcut) {
                    Ok(()) => tracing::info!("Registered global hotkey: {}", hotkey_str),
                    Err(e) => tracing::warn!(
                        "Could not register hotkey '{}': {:?} (app will still work via UI)",
                        hotkey_str,
                        e
                    ),
                },
                Err(e) => tracing::warn!("Could not parse hotkey '{}': {:?}", hotkey_str, e),
            }

            // Clear the WebView2 background so the widget is truly transparent.
            if let Some(widget) = app.get_webview_window("widget") {
                let _ = widget.set_background_color(Some(tauri::window::Color(0, 0, 0, 0)));
                if !show_widget_on_start {
                    let _ = widget.hide();
                }
            }

            // Always init engine in background — never block startup
            spawn_download_and_init(app.handle().clone(), Arc::clone(&engine), model);

            // Global input listener:
            // - Windows: double Ctrl toggles recording and shows the pill.
            // - macOS: double Command toggles recording and shows the pill.
            // - If click_to_stop is enabled, mouse clicks stop an active session.
            spawn_global_input_listener(app.handle().clone());

            tracing::info!("Murmur app started");
            Ok(())
        })
        .run(tauri::generate_context!())
        .context("error while running Murmur")?;

    Ok(())
}
