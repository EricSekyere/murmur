//! Meeting mode: local speaker diarization plus a pure,
//! testable assembly step that fuses a timestamped transcript with speaker
//! segments into a "Speaker N: ..." transcript, plus an optional local-LLM
//! summary.
//!
//! Layering keeps the model out of the logic that needs testing:
//! - [`SpeakerSegment`] and [`assembly`] are plain data and pure functions,
//!   always compiled and unit-tested without any model.
//! - [`diarize`] (feature `diarization`) produces [`SpeakerSegment`]s from audio
//!   via NVIDIA Sortformer v2 over the ORT runtime the STT path already loads.
//! - [`summary`] (feature `llm`) condenses the assembled transcript by reusing
//!   feature 1's rewrite path; no new model.
//!
//! The full streaming-STT pipeline and the meeting UI are out of scope here;
//! this crate provides the offline diarization backend and the assembly/summary
//! core the app layer will drive.

pub mod assembly;
pub mod mixer;
pub mod record;
pub mod spool;

#[cfg(feature = "diarization")]
mod diarize;
#[cfg(feature = "diarization")]
pub use diarize::{DiarizeError, diarize, download, is_downloaded, model_path};

#[cfg(feature = "llm")]
mod summary;
#[cfg(feature = "llm")]
pub use summary::summarize_meeting;

/// A span of audio attributed to one speaker, in seconds.
///
/// Speaker indices are 0-based and stable within a single diarization run
/// (Sortformer v2 resolves up to four speakers). Produced by [`diarize`] and
/// consumed by [`assembly`]; kept free of any backend type so the assembly
/// logic tests without the `diarization` feature. Serde derives let
/// [`record`] persist speaker spans; `#[serde(default)]` keeps old record
/// files loadable if fields grow (same convention as
/// [`assembly::TranscriptSegment`]).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SpeakerSegment {
    /// Segment start, seconds from the audio start.
    #[serde(default)]
    pub start_secs: f32,
    /// Segment end, seconds from the audio start.
    #[serde(default)]
    pub end_secs: f32,
    /// 0-based speaker index within this run.
    #[serde(default)]
    pub speaker: u32,
}
