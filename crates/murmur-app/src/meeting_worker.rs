//! Dedicated meeting-recording thread: owns its own mic + system-audio
//! captures, mixes them on a pull cadence, cuts short chunks at quiet points,
//! transcribes each through the shared STT engine, and persists the growing
//! [`MeetingRecord`] after every chunk (crash tolerance without ever storing
//! raw audio).
//!
//! Mirrors `audio_worker`'s discipline: capture and inference never share a
//! thread with the UI, shutdown is a stop flag the loop polls, and panics are
//! contained so a driver bug ends the meeting instead of wedging the app.
//! Transcript text flows to the frontend and the record file only — never to
//! the logs.

mod cut;
mod spool;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use murmur_core::audio::AudioBuffer;
use murmur_core::audio::capture::AudioCapture;
use murmur_core::audio::loopback::LoopbackCapture;
use murmur_core::meeting::assembly::TranscriptSegment;
use murmur_core::meeting::mixer::MeetingMixer;
use murmur_core::meeting::record::MeetingRecord;
use tauri::{Emitter, Manager};

use crate::audio_worker::panic_message;
use crate::state::AppState;

const SAMPLE_RATE: usize = AudioBuffer::SAMPLE_RATE as usize;
/// Buffer-pull cadence. Coarse enough to stay cheap, fine enough that the
/// 60 s live-buffer caps are never approached even across an inference.
const PULL_TICK: Duration = Duration::from_millis(500);
/// Cut a chunk once this much mixed audio has accumulated. Short on purpose:
/// STT quality (Parakeet especially) degrades badly on long single inputs.
const TARGET_CHUNK_SECS: usize = 20;
/// Never let a single chunk exceed this, whatever the cut scan says.
const HARD_CAP_SECS: usize = 30;
/// Emit a periodic `meeting-state` heartbeat every this many pull ticks (~2s).
const STATE_EMIT_TICKS: u32 = 4;

/// Handle to a live meeting held in [`AppState::meeting`].
pub(crate) struct MeetingHandle {
    stop: Arc<AtomicBool>,
    join: std::thread::JoinHandle<()>,
}

impl MeetingHandle {
    /// Signal the worker to stop and wait for its final flush + save.
    /// Blocking (the final chunk still runs inference) — call off the reactor.
    pub fn stop_and_join(self) {
        self.stop.store(true, Ordering::Release);
        if self.join.join().is_err() {
            tracing::error!("Meeting worker thread panicked during shutdown");
        }
    }
}

/// Spawn the meeting worker thread. The caller has already claimed
/// `meeting_active`; the worker clears it (and emits the final inactive
/// state) on every exit path, including panic.
pub(crate) fn spawn(app: tauri::AppHandle) -> MeetingHandle {
    let stop = Arc::new(AtomicBool::new(false));
    let stop_for_worker = Arc::clone(&stop);
    let join = std::thread::spawn(move || {
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_meeting(&app, &stop_for_worker);
        }));
        if let Err(panic_info) = outcome {
            let msg = panic_message(panic_info, "unknown panic in meeting worker");
            tracing::error!("Meeting worker panicked: {}", msg);
            crate::state::emit_hotkey_error(&app, &format!("Meeting recording crashed: {}", msg));
            // Privacy-critical: the panicked session never reached its own
            // spool cleanup, so its raw-audio spool may still be on disk.
            if let Ok(dir) = MeetingRecord::default_dir() {
                murmur_core::meeting::spool::sweep(&dir);
            }
        }
        if let Some(state) = app.try_state::<AppState>() {
            state.meeting_active.store(false, Ordering::Release);
        }
        emit_state(&app, false, false, 0.0, 0);
    });
    MeetingHandle { stop, join }
}

/// Per-meeting capture handles and bookkeeping for the pull loop.
struct MeetingSession {
    capture: AudioCapture,
    /// `None` when system audio is unavailable (non-Windows, or the loopback
    /// stream failed to open) — the meeting continues mic-only.
    loopback: Option<LoopbackCapture>,
    mixer: MeetingMixer,
    /// Mixed audio awaiting the next chunk cut.
    pending: Vec<f32>,
    /// Mixed samples already consumed into chunks; recording-relative chunk
    /// timestamps derive from it.
    consumed: u64,
    record: MeetingRecord,
    dir: std::path::PathBuf,
    /// Raw-audio spool for whole-meeting diarization; `None` when speaker
    /// labels are impossible (feature off, model missing) or the spool broke.
    spool: Option<murmur_core::meeting::spool::SpoolWriter>,
}

fn run_meeting(app: &tauri::AppHandle, stop: &AtomicBool) {
    let Some(mut session) = open_session(app) else {
        return;
    };
    let system_audio = session.loopback.is_some();
    emit_state(app, true, system_audio, 0.0, 0);

    let mut tick: u32 = 0;
    while !stop.load(Ordering::Acquire) {
        std::thread::sleep(PULL_TICK);
        pull_audio(&mut session);

        if session.pending.len() >= TARGET_CHUNK_SECS * SAMPLE_RATE {
            let cut = cut::select_cut_index(&session.pending, SAMPLE_RATE);
            process_chunk(app, &mut session, cut);
        }

        tick += 1;
        if tick.is_multiple_of(STATE_EMIT_TICKS) {
            emit_state(
                app,
                true,
                system_audio,
                session.mixer.duration_secs(),
                session.record.segments.len(),
            );
        }
    }

    finish_meeting(app, &mut session);
}

/// Open captures and the on-disk record. On failure, surfaces the error and
/// returns `None` (the spawn wrapper then clears the active flag).
fn open_session(app: &tauri::AppHandle) -> Option<MeetingSession> {
    let state = app.state::<AppState>();
    let (device, echo_cancellation) = {
        let settings = state.settings.lock().unwrap_or_else(|e| e.into_inner());
        (settings.audio_device.clone(), settings.echo_cancellation)
    };

    let dir = match MeetingRecord::default_dir() {
        Ok(dir) => dir,
        Err(e) => {
            fail_start(app, &format!("Cannot locate the meetings folder: {e}"));
            return None;
        }
    };

    // The meeting opens its OWN captures (WASAPI shared mode tolerates a
    // second mic handle); the dictation worker's warm stream is untouched.
    let mut capture = match AudioCapture::new() {
        Ok(capture) => capture,
        Err(e) => {
            fail_start(app, &format!("Could not create audio capture: {e}"));
            return None;
        }
    };
    // CPAL's native backend can panic on some drivers; contain it like the
    // dictation worker does.
    let started = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        capture.start(device.as_deref(), echo_cancellation)
    }));
    match started {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            fail_start(app, &format!("Could not open the microphone: {e}"));
            return None;
        }
        Err(panic_info) => {
            let msg = panic_message(panic_info, "native audio panic");
            fail_start(app, &format!("Microphone open crashed: {msg}"));
            return None;
        }
    }

    // System audio is best-effort: on the stub platform or a start failure
    // the meeting proceeds mic-only and the UI shows "mic only".
    let mut loopback = LoopbackCapture::new();
    let loopback = match loopback.start() {
        Ok(()) => Some(loopback),
        Err(e) => {
            tracing::warn!("System audio unavailable, recording mic only: {e}");
            None
        }
    };

    let started_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    tracing::info!(
        started_ms,
        system_audio = loopback.is_some(),
        "Meeting recording started"
    );

    // Last, so no earlier failure path can leak a created spool file.
    let audio_spool = spool::open_if_ready(&dir, started_ms);

    Some(MeetingSession {
        capture,
        loopback,
        mixer: MeetingMixer::new(),
        pending: Vec::new(),
        consumed: 0,
        record: MeetingRecord::new(started_ms),
        dir,
        spool: audio_spool,
    })
}

fn fail_start(app: &tauri::AppHandle, message: &str) {
    tracing::error!("{message}");
    crate::state::emit_hotkey_error(app, message);
}

/// Drain both live buffers, convert each slice to 16 kHz mono, and feed the
/// mixer. The drains copy out under the lock and leave capacity in place so
/// the realtime callbacks never reallocate.
fn pull_audio(session: &mut MeetingSession) {
    let mic_raw = drain_live(&session.capture.live_buffer());
    let mic = AudioBuffer::from_raw(
        &mic_raw,
        session.capture.native_rate(),
        session.capture.native_channels(),
    );

    let sys = match &session.loopback {
        Some(loopback) => {
            let raw = drain_live(&loopback.live_buffer());
            AudioBuffer::from_raw(&raw, loopback.native_rate(), loopback.native_channels())
        }
        None => AudioBuffer::new(),
    };

    // Both buffers were drained at the same instant, so the newest loopback
    // sample is aligned with the end of the mic slice: zero lag.
    session.mixer.push(&mic.samples, &sys.samples, 0.0);
    let mixed = session.mixer.take();
    spool::append(&mut session.spool, &mixed);
    session.pending.extend(mixed);
}

fn drain_live(buffer: &Arc<Mutex<Vec<f32>>>) -> Vec<f32> {
    let mut buf = buffer.lock().unwrap_or_else(|e| e.into_inner());
    buf.drain(..).collect()
}

/// Cut `pending[..cut]` into a chunk, transcribe it, append the segment, and
/// save the record atomically so a crash loses at most this one chunk.
fn process_chunk(app: &tauri::AppHandle, session: &mut MeetingSession, cut: usize) {
    let cut = cut.min(session.pending.len());
    if cut == 0 {
        return;
    }
    let chunk: Vec<f32> = session.pending.drain(..cut).collect();
    let start_secs = session.consumed as f32 / SAMPLE_RATE as f32;
    session.consumed += chunk.len() as u64;
    let end_secs = session.consumed as f32 / SAMPLE_RATE as f32;
    session.record.duration_secs = session.mixer.duration_secs();

    if let Some(text) = transcribe_meeting_chunk(app, &chunk) {
        let segment = TranscriptSegment::new(start_secs, end_secs, text);
        let _ = app.emit(
            "meeting-segment",
            serde_json::json!({
                "start_secs": segment.start_secs,
                "end_secs": segment.end_secs,
                "text": segment.text,
            }),
        );
        session.record.segments.push(segment);
    }
    // Save even on a silent/rejected chunk: duration progress is part of the
    // record, and the first save creates the file the list view shows.
    if let Err(e) = session.record.save_in(&session.dir) {
        tracing::warn!("Failed to save meeting record: {e:#}");
    }
}

/// Flush the final partial chunk, stop the captures, and save one last time.
fn finish_meeting(app: &tauri::AppHandle, session: &mut MeetingSession) {
    pull_audio(session);
    while !session.pending.is_empty() {
        // Flush wants completeness, not a tidy cut point — consume as much
        // as the hard cap allows per chunk (the tail can exceed one cap's
        // worth if inference stalled the pull loop).
        let cut = session.pending.len().min(HARD_CAP_SECS * SAMPLE_RATE);
        process_chunk(app, session, cut);
    }

    if let Err(e) = session.capture.stop() {
        tracing::warn!("Failed to stop meeting microphone: {e:#}");
    }
    if let Some(mut loopback) = session.loopback.take()
        && let Err(e) = loopback.stop()
    {
        tracing::warn!("Failed to stop system-audio capture: {e:#}");
    }

    // Whole-meeting diarization, after the last transcript chunk and with the
    // captures already stopped. Deletes the spool on every path; a failure
    // keeps the transcript-only record.
    spool::diarize_into(&mut session.record, session.spool.take());

    session.record.duration_secs = session.mixer.duration_secs();
    if let Err(e) = session.record.save_in(&session.dir) {
        tracing::warn!("Failed to save final meeting record: {e:#}");
    }
    tracing::info!(
        duration_secs = session.record.duration_secs,
        segments = session.record.segments.len(),
        speaker_segments = session.record.speakers.len(),
        "Meeting recording stopped"
    );
}

/// Transcribe one mixed chunk through the shared engine. Returns `None` for
/// anything that produced no usable text. Counts/durations are logged;
/// transcript text never is.
fn transcribe_meeting_chunk(app: &tauri::AppHandle, samples: &[f32]) -> Option<String> {
    let state = app.state::<AppState>();
    let outcome = {
        let mut engine_guard = state.engine.lock().unwrap_or_else(|e| e.into_inner());
        let Some(engine) = engine_guard.as_mut() else {
            tracing::warn!("STT engine unavailable mid-meeting; skipping chunk");
            return None;
        };
        // Meeting audio is multi-speaker conversation: no rolling decoder
        // prompt (a bad chunk would poison later ones) and no dictation
        // vocabulary bias.
        engine.set_initial_prompt(None);
        let language = state
            .settings
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .language
            .clone();
        engine.set_language(Some(language));
        engine.set_translate(false);
        tracing::info!(
            chunk_secs = samples.len() as f32 / SAMPLE_RATE as f32,
            "Transcribing meeting chunk"
        );
        engine.transcribe(samples)
    };
    // An inference just ran: keep the idle unloader's clock fresh so it never
    // reclaims the engine under a live meeting.
    crate::idle_unload::touch(&state);

    match outcome {
        Ok(result) if result.text.trim().is_empty() => None,
        Ok(result) => Some(result.text.trim().to_string()),
        Err(e) => {
            tracing::error!("Meeting chunk transcription failed: {e:#}");
            if e.downcast_ref::<murmur_core::stt::engine::InferencePanic>()
                .is_some()
            {
                // Same recovery as dictation: a panic unwound through the
                // native context, so drop the engine and let the next
                // activation reload a fresh one.
                let broken = state
                    .engine
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .take();
                drop(broken);
                state.engine_loaded.store(false, Ordering::Release);
                state.idle_unloaded.store(true, Ordering::Release);
                tracing::error!("Meeting inference panicked; dropped the engine for reload");
            }
            None
        }
    }
}

fn emit_state(
    app: &tauri::AppHandle,
    active: bool,
    system_audio: bool,
    duration_secs: f32,
    segments: usize,
) {
    let _ = app.emit(
        "meeting-state",
        serde_json::json!({
            "active": active,
            "system_audio": system_audio,
            "duration_secs": duration_secs,
            "segments": segments,
        }),
    );
}

/// Why a meeting cannot start right now, or `None` when it can. Pure so the
/// mutual-exclusion truth table is testable; both dictation's toggle and
/// `start_meeting` consult the same state pair.
pub(crate) fn meeting_start_blocker(
    dictation_recording: bool,
    meeting_active: bool,
    engine_loaded: bool,
) -> Option<&'static str> {
    if meeting_active {
        Some("A meeting is already being recorded")
    } else if dictation_recording {
        Some("Stop dictation before starting a meeting")
    } else if !engine_loaded {
        Some("Model still loading, please wait")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mutual_exclusion_blocks_either_direction() {
        assert!(meeting_start_blocker(true, false, true).is_some());
        assert!(meeting_start_blocker(false, true, true).is_some());
        assert!(meeting_start_blocker(false, false, false).is_some());
        assert!(meeting_start_blocker(false, false, true).is_none());
    }
}
