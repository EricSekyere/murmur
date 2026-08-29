//! Transcribing an audio or video file the user picked.
//!
//! Skips the dictation quality gates: they reject quiet or short audio, which
//! on a live microphone is usually a stray noise, but on a file the user chose
//! would silently drop passages they asked to have transcribed.

use std::path::Path;

use murmur_core::audio::AudioBuffer;
use murmur_core::stt::postprocess::PostProcessor;
use tauri::{Emitter, Manager};

use crate::state::AppState;

/// Audio per transcription call. One call over a whole file would report no
/// progress until it finished, and would exceed what the backends handle well.
const SEGMENT_SECS: usize = 30;
const SAMPLE_RATE: usize = 16_000;

#[derive(Clone, serde::Serialize)]
struct Progress {
    done: usize,
    total: usize,
    /// Text so far, so a long file fills in rather than sitting blank.
    text: String,
}

/// Decode and transcribe `path`, emitting `file-transcribe-progress` as it
/// goes. Blocking and CPU-heavy, so callers run it off the async reactor.
pub(crate) fn run(app: &tauri::AppHandle, path: &Path) -> Result<String, String> {
    let decoded = murmur_core::audio::decode::decode(path).map_err(|e| format!("{e:#}"))?;
    let buffer = AudioBuffer::from_raw(&decoded.samples, decoded.rate, decoded.channels);
    let total_secs = buffer.samples.len() as f32 / SAMPLE_RATE as f32;
    tracing::info!(
        file = %path.display(),
        rate = decoded.rate,
        channels = decoded.channels,
        secs = total_secs,
        "transcribing a file"
    );

    // A segment starting part way through a word transcribes badly or not at all.
    let segments = murmur_core::stt::chunk::windows(&buffer.samples, SEGMENT_SECS * SAMPLE_RATE);
    let total = segments.len();
    let state = app.state::<AppState>();
    let mut text = String::new();

    for (index, range) in segments.iter().enumerate() {
        let piece = {
            let mut guard = state
                .engine
                .lock()
                .unwrap_or_else(|e: std::sync::PoisonError<_>| e.into_inner());
            let engine = guard
                .as_mut()
                .ok_or("The speech model is still loading; try again in a moment")?;
            // A prompt carried across segments lets an early mistake bias the rest.
            engine.set_initial_prompt(None);
            engine
                .transcribe(&buffer.samples[range.clone()])
                .map_err(|e| format!("{e:#}"))?
        };
        let piece = piece.text.trim();
        if !piece.is_empty() {
            if !text.is_empty() {
                text.push(' ');
            }
            text.push_str(piece);
        }
        let _ = app.emit(
            "file-transcribe-progress",
            Progress {
                done: index + 1,
                total,
                text: text.clone(),
            },
        );
    }

    // Follow the user's dictation settings so a file reads the way their
    // dictation does.
    let (developer_mode, clean_speech) = {
        let settings = state
            .settings
            .lock()
            .unwrap_or_else(|e: std::sync::PoisonError<_>| e.into_inner());
        (settings.developer_mode, settings.clean_speech)
    };
    let processed = if developer_mode {
        PostProcessor::process(&text)
    } else if clean_speech {
        PostProcessor::process_prose(&text)
    } else {
        text
    };
    tracing::info!(
        segments = total,
        chars = processed.chars().count(),
        "file transcription finished"
    );
    Ok(processed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_is_split_into_segments_that_can_report_progress() {
        let ten_minutes = vec![0.2_f32; 600 * SAMPLE_RATE];
        let segments = murmur_core::stt::chunk::windows(&ten_minutes, SEGMENT_SECS * SAMPLE_RATE);
        assert!(
            segments.len() >= 20,
            "expected many segments, got {}",
            segments.len()
        );
        for range in &segments {
            assert!(
                range.len() <= SEGMENT_SECS * SAMPLE_RATE,
                "segment {range:?} is longer than one call should take"
            );
        }
    }

    #[test]
    fn segments_cover_the_file_exactly() {
        // A gap loses speech and an overlap repeats it, invisibly.
        let audio = vec![0.2_f32; 95 * SAMPLE_RATE];
        let segments = murmur_core::stt::chunk::windows(&audio, SEGMENT_SECS * SAMPLE_RATE);
        assert_eq!(segments[0].start, 0);
        assert_eq!(segments[segments.len() - 1].end, audio.len());
        for pair in segments.windows(2) {
            assert_eq!(pair[0].end, pair[1].start, "seam at {:?}", pair[0].end);
        }
    }

    #[test]
    fn a_short_file_is_a_single_segment() {
        let audio = vec![0.2_f32; 5 * SAMPLE_RATE];
        assert_eq!(
            murmur_core::stt::chunk::windows(&audio, SEGMENT_SECS * SAMPLE_RATE).len(),
            1
        );
    }
}
