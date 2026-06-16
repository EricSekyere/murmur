use std::sync::{Arc, Mutex, OnceLock, atomic::AtomicBool};
use std::time::Instant;

use murmur_core::config::Settings;
use murmur_core::stt::engine::SttEngine;
use tauri::Emitter;

use crate::audio_worker;

/// Shared application state managed by Tauri.
pub(crate) struct AppState {
    pub audio: OnceLock<audio_worker::Handle>,
    pub engine: Arc<Mutex<Option<SttEngine>>>,
    /// Lock-free "engine ready" flag so UI paths never block on the engine
    /// mutex, which transcription holds for the duration of an inference.
    pub engine_loaded: AtomicBool,
    pub recording: Mutex<bool>,
    pub settings: Mutex<Settings>,
    pub last_toggle: Mutex<Instant>,
    /// Trailing text of the running session transcript, fed to whisper as
    /// `initial_prompt` so cross-phrase punctuation stays consistent.
    /// Cleared at session start; capped to ~200 chars.
    pub session_prev_text: Mutex<String>,
    /// Character count of the last text phrase delivered to the focused
    /// window, so "scratch that" can backspace exactly that many characters.
    /// Reset to 0 by commands and at session start.
    pub last_delivered_len: Mutex<usize>,
    /// Persistent, searchable transcription history.
    pub history: Mutex<murmur_core::history::History>,
    /// Where `history` is saved on disk.
    pub history_path: std::path::PathBuf,
    /// Developer-mode override for the active session from a matched app
    /// profile. `None` means "use the global setting". Set at session start,
    /// cleared at session end.
    pub session_dev_mode: Mutex<Option<bool>>,
    /// Foreground window at recording start; output falls back to it when
    /// Murmur itself holds focus at delivery time.
    #[cfg(windows)]
    pub previous_foreground: Mutex<usize>,
    /// Last foreground window not owned by this process (live-tracked).
    #[cfg(windows)]
    pub last_external_foreground: Mutex<usize>,
}

#[derive(serde::Serialize, Clone)]
pub(crate) struct RecordingStateEvent {
    pub recording: bool,
    pub processing: bool,
}

#[derive(serde::Serialize, Clone)]
pub(crate) struct ModelDownloadProgress {
    pub percent: u8,
    pub message: String,
    pub done: bool,
    pub error: Option<String>,
}

#[derive(serde::Serialize, Clone)]
pub(crate) struct ModelInfo {
    pub id: String,
    pub name: String,
    pub backend: String,
    pub size_mb: u32,
    pub ram_estimate_mb: u32,
    pub description: String,
    pub downloaded: bool,
    pub active: bool,
}

#[derive(serde::Serialize, Clone)]
pub(crate) struct ModelChangedEvent {
    pub model_id: String,
    pub model_name: String,
    pub ready: bool,
}

/// Emit a `recording-state` event to all windows (main + widget).
pub(crate) fn emit_recording_state(app: &tauri::AppHandle, recording: bool, processing: bool) {
    let _ = app.emit(
        "recording-state",
        RecordingStateEvent {
            recording,
            processing,
        },
    );
}

pub(crate) fn emit_hotkey_error(app: &tauri::AppHandle, message: &str) {
    let _ = app.emit("hotkey-error", serde_json::json!({ "error": message }));
}

pub(crate) fn emit_transcription_error(app: &tauri::AppHandle, message: &str) {
    let _ = app.emit(
        "transcription-error",
        serde_json::json!({ "error": message }),
    );
}

/// Emit diagnostic telemetry for transcription quality debugging.
pub(crate) fn emit_transcription_diagnostic(
    app: &tauri::AppHandle,
    kind: &str,
    reason: &str,
    peak: Option<f32>,
    rms: Option<f32>,
    duration_secs: Option<f32>,
) {
    let _ = app.emit(
        "transcription-diagnostic",
        serde_json::json!({
            "kind": kind,
            "reason": reason,
            "peak": peak,
            "rms": rms,
            "duration_secs": duration_secs,
        }),
    );
}
