//! Tauri commands for meeting mode: start/stop the recording worker and
//! manage the saved meeting records (list, fetch, export, delete, summarize),
//! plus the on-demand diarization-model download.

use std::sync::atomic::Ordering;

use murmur_core::meeting::assembly;
use murmur_core::meeting::record::{self, MeetingRecord};
use tauri::State;

use crate::meeting_worker;
use crate::state::AppState;

/// Output-token budget for a meeting summary.
#[cfg(feature = "llm")]
const SUMMARY_MAX_TOKENS: usize = 512;

/// Same "not available in this build" shape as `rewrite.rs`'s Unavailable
/// outcome, so the UI wording stays consistent across LLM features.
#[cfg(not(feature = "llm"))]
const SUMMARY_UNAVAILABLE: &str =
    "The local LLM is not available in this build of Murmur (built without the llm feature).";

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

/// The full record of one saved meeting, plus precomputed speaker-labeled
/// blocks so the frontend never reimplements speaker assignment.
#[tauri::command]
pub(crate) fn get_meeting(id: u64) -> Result<serde_json::Value, String> {
    let dir = MeetingRecord::default_dir().map_err(|e| e.to_string())?;
    let meeting =
        MeetingRecord::load(&MeetingRecord::path_in(&dir, id)).map_err(|e| e.to_string())?;
    Ok(meeting_detail(&meeting))
}

/// Pure detail DTO for [`get_meeting`]: the record's fields plus the
/// assembled `blocks` (speaker, start/end, merged text).
fn meeting_detail(record: &MeetingRecord) -> serde_json::Value {
    let blocks = assembly::label_transcript(&record.segments, &record.speakers);
    serde_json::json!({
        "id": record.started_ms,
        "schema_version": record.schema_version,
        "started_ms": record.started_ms,
        "duration_secs": record.duration_secs,
        "segments": record.segments,
        "speakers": record.speakers,
        "summary": record.summary,
        "blocks": blocks,
    })
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

/// Whether this build can produce speaker labels right now: the
/// `diarization` feature is compiled in AND the Sortformer model is on disk.
/// Consulted by the meeting worker at start and surfaced via `get_status`.
#[cfg(feature = "diarization")]
pub(crate) fn diarization_model_ready() -> bool {
    murmur_core::meeting::is_downloaded()
}

#[cfg(not(feature = "diarization"))]
pub(crate) fn diarization_model_ready() -> bool {
    false
}

/// Download the Sortformer diarization model (~469 MB, SHA256-verified,
/// idempotent). The UI disables its button while this runs; meetings started
/// after it completes get speaker labels.
#[cfg(feature = "diarization")]
#[tauri::command]
pub(crate) async fn download_diarization_model() -> Result<(), String> {
    murmur_core::meeting::download()
        .await
        .map(|_| ())
        .map_err(|e| format!("{e:#}"))
}

#[cfg(not(feature = "diarization"))]
#[tauri::command]
pub(crate) async fn download_diarization_model() -> Result<(), String> {
    Err(
        "Speaker labels are not available in this build of Murmur (built without the diarization \
         feature)."
            .to_string(),
    )
}

/// Summarize one saved meeting with the local LLM, on demand (never
/// automatic), persist the summary into the record, and return it.
#[tauri::command]
pub(crate) async fn summarize_saved_meeting(
    state: State<'_, AppState>,
    id: u64,
) -> Result<String, String> {
    // Both the STT/LLM engines and the record file are busy mid-meeting.
    if state.meeting_active.load(Ordering::Acquire) {
        return Err("Stop the meeting before summarizing".to_string());
    }
    let dir = MeetingRecord::default_dir().map_err(|e| e.to_string())?;
    let meeting =
        MeetingRecord::load(&MeetingRecord::path_in(&dir, id)).map_err(|e| e.to_string())?;
    let transcript = transcript_for_summary(&meeting);
    if transcript.trim().is_empty() {
        return Err("This meeting has no transcript to summarize".to_string());
    }
    summarize_record(&state, &dir, meeting, transcript).await
}

/// The transcript the summarizer sees: speaker-labeled when diarization ran,
/// plainly joined text otherwise (all-"Unknown:" labels would only add noise).
fn transcript_for_summary(record: &MeetingRecord) -> String {
    if record.speakers.is_empty() {
        record
            .segments
            .iter()
            .map(|seg| seg.text.trim())
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        assembly::assemble(&record.segments, &record.speakers)
    }
}

/// The blocking model half of [`summarize_saved_meeting`], mirroring
/// `rewrite.rs`: lazy engine load into the shared `state.llm` slot, inference
/// on `spawn_blocking`, `idle_unload::touch` after. Transcript and summary
/// text never reach the logs.
#[cfg(feature = "llm")]
async fn summarize_record(
    state: &State<'_, AppState>,
    dir: &std::path::Path,
    mut record: MeetingRecord,
    transcript: String,
) -> Result<String, String> {
    use anyhow::Context;
    use murmur_core::llm;

    if !llm::is_downloaded() {
        let expected = llm::model_path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "the murmur data directory".to_string());
        return Err(format!(
            "The local rewrite model is not downloaded yet (expected at {expected})."
        ));
    }

    let engine_slot = std::sync::Arc::clone(&state.llm);
    let outcome = tauri::async_runtime::spawn_blocking(move || -> anyhow::Result<String> {
        // Load on first use and keep it cached: the model holds ~1 GB
        // resident, and the idle unloader reclaims it later.
        let mut slot = engine_slot.lock().unwrap_or_else(|e| e.into_inner());
        if slot.is_none() {
            let path = llm::model_path().context("resolving the rewrite model path")?;
            *slot = Some(llm::LlmEngine::load(&path).context("loading the rewrite model")?);
        }
        let engine = slot
            .as_ref()
            .context("summary engine unavailable after load")?;
        murmur_core::meeting::summarize_meeting(engine, &transcript, SUMMARY_MAX_TOKENS)
            .context("summarizing the meeting")
    })
    .await
    .map_err(|e| format!("summary task failed: {e}"))?
    .map_err(|e| format!("{e:#}"));
    // Model activity either way: keep the idle-unload clock fresh.
    crate::idle_unload::touch(state);

    let summary = outcome?.trim().to_string();
    if summary.is_empty() {
        return Err("The model produced no summary; the meeting was left unchanged".to_string());
    }
    record.summary = Some(summary.clone());
    record.save_in(dir).map_err(|e| format!("{e:#}"))?;
    Ok(summary)
}

#[cfg(not(feature = "llm"))]
async fn summarize_record(
    _state: &State<'_, AppState>,
    _dir: &std::path::Path,
    _record: MeetingRecord,
    _transcript: String,
) -> Result<String, String> {
    Err(SUMMARY_UNAVAILABLE.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use murmur_core::meeting::SpeakerSegment;
    use murmur_core::meeting::assembly::TranscriptSegment;

    fn record_with_speakers() -> MeetingRecord {
        MeetingRecord {
            segments: vec![
                TranscriptSegment::new(0.0, 1.0, "Hello"),
                TranscriptSegment::new(1.0, 2.0, "there"),
                TranscriptSegment::new(2.0, 4.0, "Hi back"),
            ],
            speakers: vec![
                SpeakerSegment {
                    start_secs: 0.0,
                    end_secs: 2.0,
                    speaker: 0,
                },
                SpeakerSegment {
                    start_secs: 2.0,
                    end_secs: 4.0,
                    speaker: 1,
                },
            ],
            ..MeetingRecord::new(42)
        }
    }

    #[test]
    fn meeting_detail_precomputes_speaker_blocks() {
        let detail = meeting_detail(&record_with_speakers());
        assert_eq!(detail["id"], 42);
        assert_eq!(detail["speakers"].as_array().map(Vec::len), Some(2));

        let blocks = detail["blocks"].as_array().expect("blocks array");
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0]["speaker"], 0);
        assert_eq!(blocks[0]["start_secs"], 0.0);
        assert_eq!(blocks[0]["end_secs"], 2.0);
        assert_eq!(blocks[0]["text"], "Hello there");
        assert_eq!(blocks[1]["speaker"], 1);
        assert_eq!(blocks[1]["text"], "Hi back");
    }

    #[test]
    fn meeting_detail_without_speakers_has_null_speaker_blocks() {
        let record = MeetingRecord {
            segments: vec![TranscriptSegment::new(0.0, 2.0, "solo line")],
            ..MeetingRecord::new(7)
        };
        let detail = meeting_detail(&record);
        assert_eq!(detail["speakers"].as_array().map(Vec::len), Some(0));
        assert_eq!(detail["summary"], serde_json::Value::Null);
        let blocks = detail["blocks"].as_array().expect("blocks array");
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0]["speaker"], serde_json::Value::Null);
        assert_eq!(blocks[0]["text"], "solo line");
    }

    #[test]
    fn summary_transcript_is_labeled_only_when_speakers_exist() {
        assert_eq!(
            transcript_for_summary(&record_with_speakers()),
            "Speaker 1: Hello there\nSpeaker 2: Hi back"
        );

        let unlabeled = MeetingRecord {
            segments: vec![
                TranscriptSegment::new(0.0, 1.0, " one "),
                TranscriptSegment::new(1.0, 2.0, ""),
                TranscriptSegment::new(2.0, 3.0, "two"),
            ],
            ..MeetingRecord::new(7)
        };
        assert_eq!(transcript_for_summary(&unlabeled), "one\ntwo");
        assert_eq!(transcript_for_summary(&MeetingRecord::new(8)), "");
    }

    /// The non-llm fallback must use the same "not available in this build"
    /// shape as rewrite.rs so the UI wording stays consistent.
    #[cfg(not(feature = "llm"))]
    #[test]
    fn summary_unavailable_message_names_the_missing_feature() {
        assert!(SUMMARY_UNAVAILABLE.contains("not available in this build"));
        assert!(SUMMARY_UNAVAILABLE.contains("llm feature"));
    }
}
