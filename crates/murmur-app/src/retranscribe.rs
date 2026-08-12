//! Re-transcribe a retained utterance against the currently configured model.
//!
//! This path runs transcription and text post-processing only. It must never
//! touch the outside world the way live dictation does: no typing or paste
//! delivery, no junction repair, no voice-command parsing, no snippet,
//! clipboard-placeholder, or emoji expansion, no auto-submit, and no session
//! context update. The result becomes a new history entry and lands on the
//! clipboard; the original entry and its retained audio stay untouched.

use std::sync::atomic::{AtomicBool, Ordering};

use tauri::Manager;

use crate::state::AppState;

/// Single-flight claim on the one allowed re-transcription, following the
/// `model_setup::SetupClaim` pattern. Releases on drop, so an error or panic
/// cannot wedge the feature shut.
struct RetranscribeClaim<'a>(&'a AtomicBool);

impl<'a> RetranscribeClaim<'a> {
    fn acquire(flag: &'a AtomicBool) -> Option<Self> {
        (!flag.swap(true, Ordering::AcqRel)).then_some(Self(flag))
    }
}

impl Drop for RetranscribeClaim<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

/// Why a request is refused before any work starts. Live dictation always
/// wins: both paths contend for the single engine mutex, and stalling live
/// speech to fix an old transcript is the wrong priority.
fn refusal_reason(
    dictation_active: bool,
    meeting_active: bool,
    already_running: bool,
) -> Option<&'static str> {
    if dictation_active {
        return Some("Dictation is running. Stop it first, then re-transcribe.");
    }
    if meeting_active {
        return Some("A meeting is recording. Stop it first, then re-transcribe.");
    }
    if already_running {
        return Some("A re-transcription is already running.");
    }
    None
}

/// The complete set of side effects a successful re-transcription may have.
/// Deliberately closed: typed/pasted delivery, junction repair, voice-command
/// execution, auto-submit, and session-context updates have no variant here,
/// so the executor (which matches this enum exhaustively) cannot regress into
/// touching the user's focused window.
#[derive(Debug, PartialEq)]
enum RetranscribeAction {
    AddHistoryEntry { text: String, app: Option<String> },
    CopyToClipboard { text: String },
}

fn actions_for(text: &str, source_app: Option<String>) -> Vec<RetranscribeAction> {
    vec![
        RetranscribeAction::AddHistoryEntry {
            text: text.to_string(),
            app: source_app,
        },
        RetranscribeAction::CopyToClipboard {
            text: text.to_string(),
        },
    ]
}

/// Blocking body of the `retranscribe_entry` command; runs on a blocking
/// thread because inference holds the engine mutex for its full duration.
pub(crate) fn run(app: &tauri::AppHandle, entry_id: &str) -> Result<String, String> {
    let state = app.state::<AppState>();

    let dictation_active = *state.recording.lock().unwrap_or_else(|e| e.into_inner());
    let meeting_active = state.meeting_active.load(Ordering::Acquire);
    let claim = RetranscribeClaim::acquire(&state.retranscribing);
    if let Some(reason) = refusal_reason(dictation_active, meeting_active, claim.is_none()) {
        return Err(reason.to_string());
    }
    // Held for the rest of the run; released on every return path by Drop.
    let _claim = claim;

    // Idle-unloaded engine: recover exactly the way session start does — kick
    // the one-shot reload and report the same "still loading" message.
    if !state
        .engine_loaded
        .load(std::sync::atomic::Ordering::Acquire)
    {
        if state
            .idle_unloaded
            .swap(false, std::sync::atomic::Ordering::AcqRel)
        {
            let model = state
                .settings
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .model;
            crate::model_setup::spawn_download_and_init(
                app.clone(),
                std::sync::Arc::clone(&state.engine),
                model,
            );
        }
        return Err("Model still loading, please wait".to_string());
    }

    let buffer = {
        let retained = state
            .retained_audio
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let Some(utterance) = retained.get(entry_id) else {
            return Err("The audio for this entry is no longer available.".to_string());
        };
        murmur_core::audio::AudioBuffer {
            samples: utterance.samples.clone(),
            sample_rate: utterance.sample_rate,
        }
    };
    let source_app = state
        .history
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .entry_by_id(entry_id)
        .and_then(|entry| entry.app.clone());

    tracing::info!(entry_id, "re-transcribing retained utterance");
    let Some((text, _processing_ms)) = crate::transcribe::transcribe_detached(app, &buffer) else {
        return Err(
            "Re-transcription did not produce a usable transcript. The original entry is unchanged."
                .to_string(),
        );
    };

    for action in actions_for(&text, source_app) {
        match action {
            RetranscribeAction::AddHistoryEntry { text, app: source } => {
                let mut history = state.history.lock().unwrap_or_else(|e| e.into_inner());
                // The new entry gets its own id; the retained buffer stays
                // keyed to the original entry so the user can retry again.
                let _new_id = history.add(&text, source);
                if let Err(e) = history.save(&state.history_path) {
                    tracing::warn!("Failed to save history after re-transcription: {}", e);
                }
            }
            RetranscribeAction::CopyToClipboard { text } => {
                let copied = murmur_core::output::clipboard::ClipboardOutput::new()
                    .and_then(|mut clipboard| clipboard.copy(&text));
                if let Err(e) = copied {
                    tracing::warn!("Failed to copy re-transcription to clipboard: {}", e);
                }
            }
        }
    }
    crate::tray::update_menu(app);
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refusal_prefers_live_dictation() {
        assert!(refusal_reason(true, false, false).is_some());
        assert!(refusal_reason(false, true, false).is_some());
        assert!(refusal_reason(false, false, true).is_some());
        // Dictation outranks the single-flight message when both apply.
        assert_eq!(
            refusal_reason(true, false, true),
            Some("Dictation is running. Stop it first, then re-transcribe.")
        );
        assert!(refusal_reason(false, false, false).is_none());
    }

    #[test]
    fn single_flight_claim_is_exclusive_and_releases_on_drop() {
        let flag = AtomicBool::new(false);
        let first = RetranscribeClaim::acquire(&flag);
        assert!(first.is_some());
        assert!(RetranscribeClaim::acquire(&flag).is_none());
        drop(first);
        assert!(RetranscribeClaim::acquire(&flag).is_some());
    }

    /// The boundary most likely to regress silently: a re-transcription must
    /// never deliver output. Its full effect set is a history entry and a
    /// clipboard copy; anything else showing up here is a bug.
    #[test]
    fn retranscription_produces_no_delivered_output() {
        let actions = actions_for("hello world", Some("code.exe".to_string()));
        assert_eq!(
            actions,
            vec![
                RetranscribeAction::AddHistoryEntry {
                    text: "hello world".to_string(),
                    app: Some("code.exe".to_string()),
                },
                RetranscribeAction::CopyToClipboard {
                    text: "hello world".to_string(),
                },
            ]
        );
    }
}
