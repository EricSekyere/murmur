use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicBool, AtomicU64},
};
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
    /// Monotonic id bumped under the `recording` lock each time a session
    /// starts. A streaming worker captures its id and only mutates the shared
    /// recording/UI state while it still matches — so a worker whose session
    /// has been superseded (rapid stop/start) can't stomp the live one.
    pub session_generation: AtomicU64,
    /// Join handle of the most recent streaming worker. A new worker joins it
    /// before touching the audio result channel, so two workers never consume
    /// that single channel at once (which would split or drop phrases).
    pub streaming_worker: Mutex<Option<std::thread::JoinHandle<()>>>,
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
    /// One-shot startup warning (e.g. hotkey registration failed), shown once
    /// the webview is ready. Cleared on read.
    pub startup_notice: Mutex<Option<String>>,
    /// Display-only mode for the onboarding mic test: phrases are shown but not
    /// typed, run as commands, or saved.
    pub suppress_output: AtomicBool,
    /// Codebase-derived vocabulary from the configured projects, indexed in the
    /// background at startup. Merged with the user's manual vocabulary at
    /// transcription time; empty when the indexer is disabled.
    pub project_vocab: Mutex<Vec<String>>,
    /// Live file watcher that re-indexes when a project's source files change.
    /// `None` when the indexer is disabled or has no roots. Dropping it stops
    /// watching.
    pub codebase_watcher:
        Mutex<Option<notify_debouncer_mini::Debouncer<notify::RecommendedWatcher>>>,
    /// True while a background project index is running, so a burst of file
    /// changes coalesces into one re-scan instead of stacking concurrent scans.
    pub indexing: AtomicBool,
    /// Set when a change arrives mid-scan; the running scan picks it up and
    /// re-runs once instead of being lost.
    pub index_pending: AtomicBool,
    /// True while voice-to-action command mode is active. Toggled by the
    /// command-mode hotkey, a separate activation channel from dictation.
    pub command_mode: AtomicBool,
    /// Command-mode routing/execution context plus the single pending action
    /// awaiting physical confirmation. Async mutex because execution awaits
    /// the tool backend while holding it.
    pub command: tokio::sync::Mutex<crate::command_mode::CommandState>,
    /// Local Help retrieval engine, built in the background at startup once the
    /// embedder model is downloaded. `None` until ready (or if the build fails);
    /// `help_search` then returns no hits rather than erroring.
    #[cfg(feature = "full")]
    pub help: Arc<Mutex<Option<murmur_core::help::HelpEngine>>>,
    /// Local LLM engine for Command Mode selection rewrites, loaded lazily on
    /// first use (the model holds about 1 GB resident) and reused. Std mutex:
    /// only locked inside spawn_blocking, held across an inference.
    #[cfg(feature = "llm")]
    pub llm: Arc<Mutex<Option<murmur_core::llm::LlmEngine>>>,
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

/// One Help search result sent to the frontend: the matched section plus its
/// cosine score. Mirrors `murmur_core::help::HelpHit`.
#[cfg(feature = "full")]
#[derive(serde::Serialize, Clone)]
pub(crate) struct HelpResultDto {
    pub article: String,
    pub heading: String,
    pub body: String,
    pub score: f32,
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
