//! Meeting-audio spool orchestration for speaker labels.
//!
//! Sortformer needs the FULL meeting audio for consistent speaker indices
//! (per-chunk diarization would relabel speakers arbitrarily at every chunk
//! boundary), so while diarization is possible the worker streams each mixed
//! 16 kHz chunk to `murmur_core::meeting::spool` and diarizes the whole file
//! once at stop, on this same worker thread.
//!
//! PRIVACY-CRITICAL: the spool is raw meeting audio on disk. Deletion paths:
//! 1. normal stop — [`diarize_into`] deletes it right after the read-back,
//!    before inference even starts;
//! 2. read/diarize failure — same function, transcript-only record kept;
//! 3. a mid-meeting write failure — [`append`] deletes it immediately and the
//!    meeting continues transcript-only;
//! 4. worker panic — the spawn wrapper sweeps the meetings dir;
//! 5. app crash — the startup sweep in `lib.rs::setup_app`.
//!
//! When diarization is not possible, no spool is ever written (Phase 1
//! behavior exactly).

use std::path::Path;

use murmur_core::meeting::record::MeetingRecord;
use murmur_core::meeting::spool::SpoolWriter;

/// Open the spool only when this build can diarize AND the model is already
/// on disk (checked once, at meeting start); otherwise no meeting audio ever
/// touches the disk. A create failure just skips speaker labels.
pub(super) fn open_if_ready(dir: &Path, id: u64) -> Option<SpoolWriter> {
    if !crate::meeting_commands::diarization_model_ready() {
        return None;
    }
    match SpoolWriter::create(dir, id) {
        Ok(writer) => Some(writer),
        Err(e) => {
            tracing::warn!("Meeting audio spool unavailable; skipping speaker labels: {e:#}");
            None
        }
    }
}

/// Append one mixed chunk to the spool, if one is open. A write failure
/// abandons speaker labels for this meeting: the partial spool is deleted
/// immediately (never leave audio behind) and the meeting continues
/// transcript-only.
pub(super) fn append(slot: &mut Option<SpoolWriter>, samples: &[f32]) {
    if samples.is_empty() {
        return;
    }
    let Some(writer) = slot.as_mut() else {
        return;
    };
    if let Err(e) = writer.append(samples) {
        tracing::warn!("Meeting audio spool write failed; skipping speaker labels: {e:#}");
        if let Some(writer) = slot.take() {
            writer.delete();
        }
    }
}

/// Diarize the spooled meeting audio and store the speaker spans on the
/// record. Blocking inference, run deliberately on the meeting worker thread;
/// `diarize()` contains its own panic guard, so a native crash surfaces as an
/// error here. Every path deletes the spool, and any failure keeps the
/// transcript-only record — diarization must never fail the meeting.
#[cfg(feature = "diarization")]
pub(super) fn diarize_into(record: &mut MeetingRecord, slot: Option<SpoolWriter>) {
    use murmur_core::meeting::spool;

    let Some(writer) = slot else {
        return;
    };
    let path = writer.finish();
    // One Vec holding the whole meeting (~230 MB/hour at 16 kHz f32): a
    // transient peak accepted for consistent speaker indices across the run.
    let samples = match spool::read(&path) {
        Ok(samples) => samples,
        Err(e) => {
            tracing::warn!("Could not read meeting audio spool; skipping speaker labels: {e:#}");
            spool::remove(&path);
            return;
        }
    };
    // Delete before the (long) inference: the audio now lives only in memory,
    // so even an abort mid-diarization leaves nothing on disk.
    spool::remove(&path);

    match murmur_core::meeting::diarize(&samples, murmur_core::audio::AudioBuffer::SAMPLE_RATE) {
        Ok(speakers) => {
            tracing::info!(
                speaker_segments = speakers.len(),
                "Meeting diarization complete"
            );
            record.speakers = speakers;
        }
        Err(e) => {
            tracing::warn!("Meeting diarization failed; keeping transcript-only record: {e}");
        }
    }
}

/// Without the `diarization` feature [`open_if_ready`] never opens a spool;
/// still, if one somehow exists, never leave audio behind.
#[cfg(not(feature = "diarization"))]
pub(super) fn diarize_into(_record: &mut MeetingRecord, slot: Option<SpoolWriter>) {
    if let Some(writer) = slot {
        writer.delete();
    }
}
