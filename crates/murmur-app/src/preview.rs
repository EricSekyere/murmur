//! Live preview: a background worker that transcribes in-progress phrase
//! snapshots for interim on-screen display. It never delivers text and skips
//! the audio quality gates the final-phrase path applies, but still drops
//! obvious hallucination fillers so the caption doesn't flash text the final
//! path would reject. Its job is to show words as they are spoken, then get
//! out of the way of the real transcription.

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
/// `caption_target` carries the target window (and focused-input rect) when the
/// caption should roam to the active window; `None` keeps the caption under the
/// pill.
pub(crate) fn spawn(
    app: tauri::AppHandle,
    caption_target: Option<crate::caption::CaptionAnchor>,
) -> (Sender<AudioBuffer>, JoinHandle<()>) {
    let (tx, rx) = std::sync::mpsc::channel::<AudioBuffer>();
    let handle = std::thread::spawn(move || run(&app, rx, caption_target));
    (tx, handle)
}

fn run(
    app: &tauri::AppHandle,
    rx: Receiver<AudioBuffer>,
    caption_target: Option<crate::caption::CaptionAnchor>,
) {
    let state = app.state::<AppState>();
    while let Ok(mut latest) = rx.recv() {
        // Collapse the backlog: only the most recent snapshot reflects what
        // the user has said so far, so older ones are dead weight.
        while let Ok(newer) = rx.try_recv() {
            latest = newer;
        }
        if let Some(text) = transcribe_preview(&state, &latest) {
            let _ = app.emit("streaming-partial", serde_json::json!({ "text": text }));
            if let Some(anchor) = &caption_target {
                crate::caption::show(app, anchor, &text);
            }
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
    let (language, translate, profile) = {
        let settings = state.settings.lock().unwrap_or_else(|e| e.into_inner());
        (
            settings.language.clone(),
            settings.translate_to_english,
            settings.transcription_profile,
        )
    };
    let non_english = crate::transcribe::is_non_english_language(&language);
    let mut guard = state.engine.try_lock().ok()?;
    let engine = guard.as_mut()?;
    // No decoder prompt: a preview must not pollute the running session
    // context that the final-phrase path feeds back as `initial_prompt`.
    engine.set_initial_prompt(None);
    // Mirror the language/translate settings so the preview matches the final.
    engine.set_language(Some(language));
    engine.set_translate(translate);
    let result = engine.transcribe(&audio.samples).ok()?;
    let text = result.text.trim();
    if text.is_empty() || crate::transcribe::is_hallucination_text(text, profile, non_english) {
        return None;
    }
    Some(text.to_string())
}
