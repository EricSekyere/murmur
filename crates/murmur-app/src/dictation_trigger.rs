//! Poller for MCP dictation triggers: consume request files written by the
//! MCP server's `request_dictation` tool and start a recording session, so a
//! coding agent can ask the user a question they answer by voice.

use std::time::Duration;

use murmur_core::dictation_request::{self, DictationRequest};
use tauri::{Emitter, Manager};

use crate::state::AppState;

const POLL_INTERVAL: Duration = Duration::from_millis(400);

/// Whether a consumed trigger should start a session now: the safety setting
/// must allow it, no session may be active (never interrupt live dictation),
/// and the request must be recent (a stale file left by a crashed run must
/// not auto-fire on a later start). Pure so the poll loop stays thin.
fn should_start(req: &DictationRequest, now_ms: u64, enabled: bool, recording: bool) -> bool {
    enabled && !recording && dictation_request::is_fresh(req, now_ms)
}

/// Drop any stale trigger from a previous run, then poll for new ones on a
/// background thread (~400ms cadence; one file existence check when idle).
/// Each trigger is consumed exactly once; `begin_recording` is idempotent, so
/// a race with the hotkey cannot double-start.
pub(crate) fn spawn(app: tauri::AppHandle) {
    let path = match dictation_request::default_path() {
        Ok(path) => path,
        Err(e) => {
            tracing::warn!("Dictation trigger poller disabled, no config dir: {e}");
            return;
        }
    };
    dictation_request::clear(&path);
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(POLL_INTERVAL);
            let Some(req) = dictation_request::take(&path) else {
                continue;
            };
            let Some(state) = app.try_state::<AppState>() else {
                continue;
            };
            let enabled = state
                .settings
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .mcp_dictation_enabled;
            let recording = *state.recording.lock().unwrap_or_else(|e| e.into_inner());
            if !should_start(&req, now_ms(), enabled, recording) {
                tracing::debug!(enabled, recording, "dropped dictation trigger");
                continue;
            }
            tracing::info!("Starting dictation session from MCP trigger");
            // Agent-supplied question (never a transcript): surfaced to the
            // frontend for display alongside the recording pill.
            if let Some(prompt) = req.prompt {
                let _ = app.emit(
                    "dictation-requested",
                    serde_json::json!({ "prompt": prompt }),
                );
            }
            crate::session::begin_recording(&app);
            crate::input::show_widget_window(&app);
        }
    });
}

/// Unix epoch milliseconds, the same clock the trigger was stamped with.
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use murmur_core::dictation_request::MAX_AGE_MS;

    fn req(requested_ms: u64) -> DictationRequest {
        DictationRequest {
            requested_ms,
            prompt: None,
        }
    }

    #[test]
    fn starts_only_when_enabled_idle_and_fresh() {
        let now = 10 * MAX_AGE_MS;
        assert!(should_start(&req(now), now, true, false));
        // Safety setting off: drop.
        assert!(!should_start(&req(now), now, false, false));
        // Already recording: never interrupt the active session.
        assert!(!should_start(&req(now), now, true, true));
        // Stale request (e.g. app was not running when it was written): drop.
        assert!(!should_start(&req(now - MAX_AGE_MS - 1), now, true, false));
        // Right at the age limit still fires.
        assert!(should_start(&req(now - MAX_AGE_MS), now, true, false));
    }
}
