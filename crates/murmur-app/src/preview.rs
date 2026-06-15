//! Live preview: a background worker that transcribes in-progress phrase
//! snapshots for interim on-screen display. It never delivers text and skips
//! the quality gates the final-phrase path applies — its only job is to show
//! words as they are spoken, then get out of the way of the real transcription.

use std::sync::mpsc::{Receiver, Sender};
use std::thread::JoinHandle;

use murmur_core::audio::AudioBuffer;
use tauri::{Emitter, Manager};

use crate::state::AppState;

/// Snapshots shorter than this aren't worth a decode pass (~250ms at 16kHz).
const MIN_PREVIEW_SAMPLES: usize = 16_000 / 4;

/// Spawn the preview worker. It owns the receiving end of a latest-wins
/// channel: the streaming worker forwards phrase snapshots, and this thread
/// transcribes only the newest one, discarding anything that piled up while a
/// decode was running. Drop the returned sender to stop the worker; join the
/// handle to wait for the in-flight decode to finish.
pub(crate) fn spawn(app: tauri::AppHandle) -> (Sender<AudioBuffer>, JoinHandle<()>) {
    let (tx, rx) = std::sync::mpsc::channel::<AudioBuffer>();
    let handle = std::thread::spawn(move || run(&app, rx));
    (tx, handle)
}

fn run(app: &tauri::AppHandle, rx: Receiver<AudioBuffer>) {
    let state = app.state::<AppState>();
    while let Ok(mut latest) = rx.recv() {
        // Collapse the backlog: only the most recent snapshot reflects what
        // the user has said so far, so older ones are dead weight.
        while let Ok(newer) = rx.try_recv() {
            latest = newer;
        }
        if let Some(text) = transcribe_preview(&state, &latest) {
            let _ = app.emit("streaming-partial", serde_json::json!({ "text": text }));
        }
    }
}

/// Transcribe a snapshot for display only. Uses `try_lock` on the engine so a
/// real phrase transcription always wins: if a final is mid-decode, this
/// snapshot is dropped rather than queued behind it, keeping delivered text
/// from ever waiting on a preview.
fn transcribe_preview(state: &AppState, audio: &AudioBuffer) -> Option<String> {
    if audio.samples.len() < MIN_PREVIEW_SAMPLES {
        return None;
    }
    let mut guard = state.engine.try_lock().ok()?;
    let engine = guard.as_mut()?;
    // No decoder prompt: a preview must not pollute the running session
    // context that the final-phrase path feeds back as `initial_prompt`.
    engine.set_initial_prompt(None);
    let result = engine.transcribe(&audio.samples).ok()?;
    let text = result.text.trim();
    (!text.is_empty()).then(|| text.to_string())
}
