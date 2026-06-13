//! Recording session lifecycle: toggle handling and the streaming worker
//! that turns detected phrases into delivered text.

use std::time::{Duration, Instant};

use murmur_core::output::OutputMode;
use murmur_core::voice_commands::{self, VoiceCommand};
use tauri::{Emitter, Manager};

use crate::audio_worker::{AudioResult, StartParams, panic_message};
use crate::state::{
    AppState, emit_hotkey_error, emit_recording_state, emit_transcription_diagnostic,
};
use crate::transcribe::transcribe_chunk;

/// Handle a recording toggle from any input source (hotkey, UI, double-tap,
/// click-to-stop), debounced so simultaneous sources fire once.
pub(crate) fn handle_toggle(app: &tauri::AppHandle) {
    let state = app.state::<AppState>();

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
        *recording = false;
        drop(recording);
        stop_session(app, &state);
    } else {
        drop(recording);
        start_session(app, &state);
    }
}

/// Push-to-talk: start recording if idle. Idempotent — a held key that
/// auto-repeats won't restart an active session.
pub(crate) fn begin_recording(app: &tauri::AppHandle) {
    let state = app.state::<AppState>();
    let already = *state.recording.lock().unwrap_or_else(|e| e.into_inner());
    if already {
        return;
    }
    start_session(app, &state);
}

/// Push-to-talk: stop recording if active. Idempotent.
pub(crate) fn end_recording(app: &tauri::AppHandle) {
    let state = app.state::<AppState>();
    let mut recording = state.recording.lock().unwrap_or_else(|e| e.into_inner());
    if !*recording {
        return;
    }
    *recording = false;
    drop(recording);
    stop_session(app, &state);
}

fn stop_session(app: &tauri::AppHandle, state: &AppState) {
    tracing::info!("Toggle: manual stop");
    // Reflect stopped state immediately; the streaming worker finishes
    // cleanup when it receives StreamingDone.
    emit_recording_state(app, false, false);

    let stop_result = match state.audio.get() {
        Some(audio) => audio.request_stop(),
        None => Err("Audio worker not initialized".to_string()),
    };
    if let Err(e) = stop_result {
        tracing::error!("Failed to send stop command: {}", e);
        emit_hotkey_error(app, &format!("Failed to stop recording: {}", e));
    }
}

fn start_session(app: &tauri::AppHandle, state: &AppState) {
    if !state
        .engine_loaded
        .load(std::sync::atomic::Ordering::Acquire)
    {
        emit_hotkey_error(app, "Model still loading — please wait");
        return;
    }

    *state.recording.lock().unwrap_or_else(|e| e.into_inner()) = true;

    #[cfg(windows)]
    crate::focus::save_output_target_window(state);

    tracing::info!("Toggle: start streaming");

    // Queue StartStreaming synchronously, before spawning the worker thread:
    // the command channel is FIFO, so a later stop toggle's Stop can never
    // overtake the start and leave the mic running while the UI shows idle.
    let params = {
        let settings = state.settings.lock().unwrap_or_else(|e| e.into_inner());
        StartParams {
            audio_device: settings.audio_device.clone(),
            rms_threshold: settings.silence_rms_threshold,
            vad_threshold: settings.vad_threshold,
            phrase_pause: Duration::from_secs_f32(settings.phrase_pause_secs),
            session_timeout: Duration::from_secs_f32(settings.session_timeout_secs),
        }
    };
    let send_result = match state.audio.get() {
        Some(audio) => audio.send_start(params),
        None => Err("Audio worker not initialized".to_string()),
    };
    if let Err(e) = send_result {
        tracing::error!("Failed to queue start command: {}", e);
        *state.recording.lock().unwrap_or_else(|e| e.into_inner()) = false;
        emit_recording_state(app, false, false);
        emit_hotkey_error(app, &format!("Failed to start recording: {}", e));
        return;
    }

    emit_recording_state(app, true, false);
    spawn_streaming_worker(app.clone());
}

fn spawn_streaming_worker(app: tauri::AppHandle) {
    std::thread::spawn(move || {
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            streaming_worker(&app);
        }));
        if let Err(panic_info) = outcome {
            let msg = panic_message(panic_info, "unknown panic in streaming worker");
            tracing::error!("Streaming worker panicked: {}", msg);
            if let Some(state) = app.try_state::<AppState>() {
                *state.recording.lock().unwrap_or_else(|e| e.into_inner()) = false;
            }
            emit_recording_state(&app, false, false);
            emit_hotkey_error(&app, &format!("Recording crashed: {}", msg));
        }
    });
}

/// Per-session bookkeeping used to pick the right end-of-session message.
#[derive(Default)]
struct SessionStats {
    had_transcription: bool,
    saw_signal: bool,
    had_phrase_audio: bool,
    saw_no_signal: bool,
}

/// Background thread: receive phrases from the audio worker, transcribe
/// each, and deliver the text to the focused application.
fn streaming_worker(app: &tauri::AppHandle) {
    let state = app.state::<AppState>();
    let output_mode = state
        .settings
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .output_mode;

    #[cfg(windows)]
    let previous_hwnd = *state
        .previous_foreground
        .lock()
        .unwrap_or_else(|e| e.into_inner());

    // Fresh decoder context: the first phrase of this session must not be
    // biased by the last phrase of the previous one.
    state
        .session_prev_text
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clear();
    *state
        .last_delivered_len
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = 0;

    let Some(audio) = state.audio.get() else {
        tracing::error!("Audio worker not initialized in streaming_worker");
        *state.recording.lock().unwrap_or_else(|e| e.into_inner()) = false;
        emit_recording_state(app, false, false);
        return;
    };
    if let Err(e) = audio.await_started() {
        tracing::error!("Failed to start streaming: {}", e);
        *state.recording.lock().unwrap_or_else(|e| e.into_inner()) = false;
        emit_recording_state(app, false, false);
        emit_hotkey_error(app, &format!("Failed to start recording: {}", e));
        return;
    }

    let mut stats = SessionStats::default();
    loop {
        match audio.recv_result() {
            Ok(AudioResult::PhraseReady(buffer)) => {
                stats.had_phrase_audio = true;
                handle_phrase(
                    app,
                    &state,
                    &buffer,
                    output_mode,
                    #[cfg(windows)]
                    previous_hwnd,
                    &mut stats,
                );
            }
            Ok(AudioResult::StreamingDone) => {
                tracing::info!("Streaming session ended");
                break;
            }
            Ok(AudioResult::AudioLevel(rms)) => {
                let _ = app.emit("audio-level", rms);
            }
            Ok(AudioResult::SignalDetected) => {
                stats.saw_signal = true;
                let _ = app.emit("audio-signal-detected", serde_json::json!({}));
            }
            Ok(AudioResult::NoSignal(message)) => {
                stats.saw_no_signal = true;
                emit_transcription_diagnostic(app, "rejected", "no_signal", None, None, None);
                crate::state::emit_transcription_error(app, &message);
            }
            Ok(AudioResult::Started) => {}
            Ok(AudioResult::StartFailed(e)) => {
                tracing::error!("Unexpected StartFailed during streaming: {}", e);
                break;
            }
            Err(e) => {
                tracing::error!("Streaming recv error: {}", e);
                emit_hotkey_error(app, &format!("Streaming error: {}", e));
                break;
            }
        }
    }

    finish_streaming(app, &state, &stats);
}

/// Transcribe one phrase and deliver the result. Phrases that arrive after
/// a manual stop are still delivered — the speech already happened.
fn handle_phrase(
    app: &tauri::AppHandle,
    state: &AppState,
    buffer: &murmur_core::audio::AudioBuffer,
    output_mode: OutputMode,
    #[cfg(windows)] previous_hwnd: usize,
    stats: &mut SessionStats,
) {
    tracing::info!(
        "Phrase ready: {} samples ({:.1}s)",
        buffer.samples.len(),
        buffer.samples.len() as f32 / 16_000.0
    );

    let still_recording = *state.recording.lock().unwrap_or_else(|e| e.into_inner());
    emit_recording_state(app, still_recording, true);

    if let Some((text, processing_time_ms)) = transcribe_chunk(app, buffer) {
        stats.had_transcription = true;
        match voice_commands::parse(&text) {
            VoiceCommand::Text => deliver_text(
                app,
                state,
                &text,
                output_mode,
                processing_time_ms,
                #[cfg(windows)]
                previous_hwnd,
            ),
            command => execute_command(app, state, command),
        }
    }

    let still_recording = *state.recording.lock().unwrap_or_else(|e| e.into_inner());
    if still_recording {
        emit_recording_state(app, true, false);
    }
}

/// Deliver a normal text phrase to the focused window and record how many
/// characters landed so "scratch that" can undo exactly this phrase.
fn deliver_text(
    app: &tauri::AppHandle,
    state: &AppState,
    text: &str,
    output_mode: OutputMode,
    processing_time_ms: u64,
    #[cfg(windows)] previous_hwnd: usize,
) {
    deliver_output(
        app,
        state,
        text,
        output_mode,
        #[cfg(windows)]
        previous_hwnd,
    );

    // Focused modes append a trailing space (dispatch_output); clipboard-only
    // doesn't type, so there is nothing to scratch.
    let delivered = if matches!(output_mode, OutputMode::Clipboard | OutputMode::Stdout) {
        0
    } else {
        text.trim().chars().count() + 1
    };
    *state
        .last_delivered_len
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = delivered;

    let _ = app.emit(
        "streaming-phrase",
        serde_json::json!({ "text": text, "processing_time_ms": processing_time_ms }),
    );
}

/// Run a spoken editing command (new line, new paragraph, scratch that).
fn execute_command(app: &tauri::AppHandle, state: &AppState, command: VoiceCommand) {
    use murmur_core::output::keyboard;

    let result = match command {
        VoiceCommand::NewLine => keyboard::press_enter(1),
        VoiceCommand::NewParagraph => keyboard::press_enter(2),
        VoiceCommand::ScratchThat => {
            let count = *state
                .last_delivered_len
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            // A correction invalidates the decoder context — start fresh so
            // the model doesn't keep biasing toward deleted words.
            state
                .session_prev_text
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clear();
            keyboard::press_backspace(count)
        }
        VoiceCommand::Text => return,
    };
    if let Err(e) = result {
        tracing::error!("Voice command failed: {}", e);
        emit_hotkey_error(app, &format!("Voice command failed: {}", e));
    }

    // After any command, there is no longer a coherent "last phrase" to
    // scratch (a second "scratch that" should not run).
    *state
        .last_delivered_len
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = 0;

    let label = match command {
        VoiceCommand::NewLine => "new line",
        VoiceCommand::NewParagraph => "new paragraph",
        VoiceCommand::ScratchThat => "scratch that",
        VoiceCommand::Text => "",
    };
    tracing::info!("Executed voice command: {}", label);
    let _ = app.emit("voice-command", serde_json::json!({ "command": label }));
}

fn deliver_output(
    app: &tauri::AppHandle,
    state: &AppState,
    text: &str,
    output_mode: OutputMode,
    #[cfg(windows)] previous_hwnd: usize,
) {
    #[cfg(not(windows))]
    let _ = state;
    #[cfg(windows)]
    let last_external_hwnd = *state
        .last_external_foreground
        .lock()
        .unwrap_or_else(|e| e.into_inner());

    // enigo/clipboard interact with OS APIs that can panic — contain it.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        crate::focus::output_text(
            text,
            output_mode,
            #[cfg(windows)]
            previous_hwnd,
            #[cfg(windows)]
            last_external_hwnd,
        )
    }));
    match result {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            tracing::error!("Failed to output text: {}", e);
            emit_hotkey_error(app, &format!("Failed to output text: {}", e));
        }
        Err(panic_info) => {
            let msg = panic_message(panic_info, "unknown panic in output_text");
            tracing::error!("output_text panicked: {}", msg);
            emit_hotkey_error(app, &format!("Output crashed: {}", msg));
        }
    }
}

fn finish_streaming(app: &tauri::AppHandle, state: &AppState, stats: &SessionStats) {
    if !stats.had_transcription && !stats.saw_no_signal {
        let msg = if stats.saw_signal || stats.had_phrase_audio {
            "Speech was detected, but transcription failed. Try speaking a bit slower/closer to the mic, or switch to a larger model."
        } else {
            "No speech detected — check your microphone input"
        };
        emit_hotkey_error(app, msg);
    }

    *state.recording.lock().unwrap_or_else(|e| e.into_inner()) = false;
    emit_recording_state(app, false, false);
    let _ = app.emit("streaming-done", serde_json::json!({}));
}
