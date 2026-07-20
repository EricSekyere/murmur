//! Tauri commands for meeting mode: start/stop the recording worker and
//! manage the saved meeting records (list, fetch, export, delete).

use std::sync::atomic::Ordering;

use murmur_core::meeting::record::{self, MeetingRecord};
use tauri::State;

use crate::meeting_worker;
use crate::state::AppState;

/// Start recording a meeting. Refuses while dictation is recording (and vice
/// versa — `session::start_session` checks `meeting_active`): the two modes
/// would fight over the microphone and the STT engine.
#[tauri::command]
pub(crate) fn start_meeting(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    // Claim under the recording lock so a dictation toggle racing this start
    // sees a consistent pair of flags (it checks meeting_active under the
    // same lock before claiming `recording`).
    let recording = state.recording.lock().unwrap_or_else(|e| e.into_inner());
    let blocker = meeting_worker::meeting_start_blocker(
        *recording,
        state.meeting_active.load(Ordering::Acquire),
        state.engine_loaded.load(Ordering::Acquire),
    );
    if let Some(reason) = blocker {
        return Err(reason.to_string());
    }
    state.meeting_active.store(true, Ordering::Release);
    drop(recording);

    let handle = meeting_worker::spawn(app);
    *state.meeting.lock().unwrap_or_else(|e| e.into_inner()) = Some(handle);
    Ok(())
}

/// Stop the running meeting, waiting for the final chunk to transcribe and
/// the record to save. Async so the (possibly seconds-long) final inference
/// never blocks the UI thread.
#[tauri::command]
pub(crate) async fn stop_meeting(state: State<'_, AppState>) -> Result<(), String> {
    let handle = state
        .meeting
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
        .ok_or("No meeting is being recorded")?;
    tokio::task::spawn_blocking(move || handle.stop_and_join())
        .await
        .map_err(|e| format!("Meeting shutdown failed: {e}"))
}

/// Saved meetings, newest first: id, start time, duration, and segment count.
#[tauri::command]
pub(crate) fn list_meetings() -> Result<Vec<serde_json::Value>, String> {
    let dir = MeetingRecord::default_dir().map_err(|e| e.to_string())?;
    Ok(MeetingRecord::list(&dir)
        .iter()
        .map(|record| {
            serde_json::json!({
                "id": record.started_ms,
                "started_ms": record.started_ms,
                "duration_secs": record.duration_secs,
                "segments": record.segments.len(),
            })
        })
        .collect())
}

/// The full record (timestamps + transcript) of one saved meeting.
#[tauri::command]
pub(crate) fn get_meeting(id: u64) -> Result<MeetingRecord, String> {
    let dir = MeetingRecord::default_dir().map_err(|e| e.to_string())?;
    MeetingRecord::load(&MeetingRecord::path_in(&dir, id)).map_err(|e| e.to_string())
}

/// Export one meeting as Markdown next to its record, returning the path.
#[tauri::command]
pub(crate) fn export_meeting(id: u64) -> Result<String, String> {
    let dir = MeetingRecord::default_dir().map_err(|e| e.to_string())?;
    let meeting =
        MeetingRecord::load(&MeetingRecord::path_in(&dir, id)).map_err(|e| e.to_string())?;
    let markdown = record::export_markdown(&meeting);
    let path = dir.join(format!("{id}.md"));
    murmur_core::fsutil::atomic_write(&path, markdown.as_bytes())
        .map_err(|e| format!("Failed to write export: {e}"))?;
    Ok(path.to_string_lossy().into_owned())
}

/// Delete one saved meeting (record + any export). Meetings are user data:
/// this explicit command is the only thing allowed to remove them.
#[tauri::command]
pub(crate) fn delete_meeting(id: u64) -> Result<(), String> {
    let dir = MeetingRecord::default_dir().map_err(|e| e.to_string())?;
    MeetingRecord::delete_in(&dir, id).map_err(|e| e.to_string())
}
