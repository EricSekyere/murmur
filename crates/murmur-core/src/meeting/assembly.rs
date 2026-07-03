//! Pure meeting-transcript assembly: fuse a timestamped transcript with speaker
//! [`SpeakerSegment`]s into a speaker-labeled transcript.
//!
//! All logic here is model-free and deterministic so it unit-tests without any
//! ONNX model or the `diarization` feature. The steps are:
//! 1. attribute each transcript segment to the speaker whose diarization span
//!    overlaps it most (ties break toward the earlier diarization segment),
//! 2. merge consecutive segments sharing a speaker into one block, and
//! 3. render each block as a `Speaker N: text` line.

use super::SpeakerSegment;

/// A transcript segment with second-resolution timestamps and its text.
///
/// This is the assembly input, decoupled from the STT engine's centisecond
/// [`crate::stt::engine::Segment`]; a `From` conversion bridges the two.
#[derive(Debug, Clone, PartialEq)]
pub struct TranscriptSegment {
    /// Segment start, seconds from the audio start.
    pub start_secs: f32,
    /// Segment end, seconds from the audio start.
    pub end_secs: f32,
    /// The segment's transcribed text.
    pub text: String,
}

impl TranscriptSegment {
    /// Construct from seconds and text.
    pub fn new(start_secs: f32, end_secs: f32, text: impl Into<String>) -> Self {
        Self {
            start_secs,
            end_secs,
            text: text.into(),
        }
    }
}

impl From<&crate::stt::engine::Segment> for TranscriptSegment {
    /// STT segments carry centisecond (10 ms) timestamps; convert to seconds.
    fn from(seg: &crate::stt::engine::Segment) -> Self {
        Self {
            start_secs: seg.start_cs as f32 / 100.0,
            end_secs: seg.end_cs as f32 / 100.0,
            text: seg.text.clone(),
        }
    }
}

/// A contiguous run of transcript attributed to a single speaker.
#[derive(Debug, Clone, PartialEq)]
pub struct LabeledBlock {
    /// The attributed 0-based speaker, or `None` when no diarization span
    /// overlapped any of the block's segments.
    pub speaker: Option<u32>,
    /// Start of the first segment in the block, in seconds.
    pub start_secs: f32,
    /// End of the last segment in the block, in seconds.
    pub end_secs: f32,
    /// The block's joined, trimmed text.
    pub text: String,
}

/// Attribute one transcript segment to the diarization speaker it overlaps most.
///
/// Returns `None` when no diarization span overlaps the segment (including the
/// empty-diarization case). Overlap is the intersection length in seconds;
/// equal overlaps break toward the earlier entry in `diarization`.
pub fn assign_speaker(segment: &TranscriptSegment, diarization: &[SpeakerSegment]) -> Option<u32> {
    let mut best: Option<(u32, f32)> = None;
    for span in diarization {
        let overlap = (segment.end_secs.min(span.end_secs)
            - segment.start_secs.max(span.start_secs))
        .max(0.0);
        if overlap <= 0.0 {
            continue;
        }
        let improves = best.is_none_or(|(_, best_overlap)| overlap > best_overlap);
        if improves {
            best = Some((span.speaker, overlap));
        }
    }
    best.map(|(speaker, _)| speaker)
}

/// Attribute every transcript segment to a speaker, then merge consecutive
/// same-speaker segments into blocks. Whitespace-only segments are dropped so
/// they neither emit empty blocks nor break an otherwise-continuous run.
pub fn label_transcript(
    transcript: &[TranscriptSegment],
    diarization: &[SpeakerSegment],
) -> Vec<LabeledBlock> {
    let mut blocks: Vec<LabeledBlock> = Vec::new();
    for segment in transcript {
        let text = segment.text.trim();
        if text.is_empty() {
            continue;
        }
        let speaker = assign_speaker(segment, diarization);
        match blocks.last_mut() {
            Some(last) if last.speaker == speaker => {
                last.text.push(' ');
                last.text.push_str(text);
                last.end_secs = last.end_secs.max(segment.end_secs);
            }
            _ => blocks.push(LabeledBlock {
                speaker,
                start_secs: segment.start_secs,
                end_secs: segment.end_secs,
                text: text.to_string(),
            }),
        }
    }
    blocks
}

/// The display label for a block's speaker: 1-based `Speaker N`, or `Unknown`
/// when the block was never attributed.
fn label_for(speaker: Option<u32>) -> String {
    match speaker {
        Some(id) => format!("Speaker {}", id + 1),
        None => "Unknown".to_string(),
    }
}

/// Render labeled blocks as newline-separated `Speaker N: text` lines. Returns
/// an empty string for no blocks. No trailing newline.
pub fn format_transcript(blocks: &[LabeledBlock]) -> String {
    blocks
        .iter()
        .map(|block| format!("{}: {}", label_for(block.speaker), block.text))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Convenience: [`label_transcript`] then [`format_transcript`] in one call.
pub fn assemble(transcript: &[TranscriptSegment], diarization: &[SpeakerSegment]) -> String {
    format_transcript(&label_transcript(transcript, diarization))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spk(start_secs: f32, end_secs: f32, speaker: u32) -> SpeakerSegment {
        SpeakerSegment {
            start_secs,
            end_secs,
            speaker,
        }
    }

    #[test]
    fn assign_speaker_picks_the_maximum_overlap() {
        let diar = [spk(0.0, 1.2, 0), spk(1.2, 3.0, 1)];
        // Mostly inside speaker 1's span.
        let seg = TranscriptSegment::new(1.0, 2.0, "hi");
        assert_eq!(assign_speaker(&seg, &diar), Some(1));
    }

    #[test]
    fn assign_speaker_returns_none_without_overlap() {
        let diar = [spk(0.0, 1.0, 0)];
        // Starts exactly where the diarization span ends: zero overlap.
        let touching = TranscriptSegment::new(1.0, 2.0, "hi");
        assert_eq!(assign_speaker(&touching, &diar), None);
        // Fully outside.
        let outside = TranscriptSegment::new(5.0, 6.0, "hi");
        assert_eq!(assign_speaker(&outside, &diar), None);
    }

    #[test]
    fn assign_speaker_none_on_empty_diarization() {
        let seg = TranscriptSegment::new(0.0, 1.0, "hi");
        assert_eq!(assign_speaker(&seg, &[]), None);
    }

    #[test]
    fn assign_speaker_breaks_ties_toward_earlier_span() {
        // Equal 0.5s overlap with each; the earlier span (speaker 0) wins.
        let diar = [spk(0.0, 1.5, 0), spk(1.5, 3.0, 1)];
        let seg = TranscriptSegment::new(1.0, 2.0, "hi");
        assert_eq!(assign_speaker(&seg, &diar), Some(0));
    }

    #[test]
    fn label_transcript_merges_consecutive_same_speaker() {
        let diar = [spk(0.0, 2.0, 0), spk(2.0, 4.0, 1)];
        let transcript = [
            TranscriptSegment::new(0.0, 1.0, "Hello"),
            TranscriptSegment::new(1.0, 2.0, "there"),
            TranscriptSegment::new(2.0, 3.0, "Hi"),
            TranscriptSegment::new(3.0, 4.0, "back"),
        ];
        let blocks = label_transcript(&transcript, &diar);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].speaker, Some(0));
        assert_eq!(blocks[0].text, "Hello there");
        assert_eq!(blocks[0].start_secs, 0.0);
        assert_eq!(blocks[0].end_secs, 2.0);
        assert_eq!(blocks[1].speaker, Some(1));
        assert_eq!(blocks[1].text, "Hi back");
    }

    #[test]
    fn label_transcript_does_not_merge_across_a_speaker_change_and_back() {
        // A A B A must stay four... three blocks: the trailing A is a new block.
        let diar = [spk(0.0, 1.0, 0), spk(1.0, 2.0, 1), spk(2.0, 3.0, 0)];
        let transcript = [
            TranscriptSegment::new(0.0, 0.5, "one"),
            TranscriptSegment::new(0.5, 1.0, "two"),
            TranscriptSegment::new(1.0, 2.0, "three"),
            TranscriptSegment::new(2.0, 3.0, "four"),
        ];
        let blocks = label_transcript(&transcript, &diar);
        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[0].speaker, Some(0));
        assert_eq!(blocks[0].text, "one two");
        assert_eq!(blocks[1].speaker, Some(1));
        assert_eq!(blocks[1].text, "three");
        assert_eq!(blocks[2].speaker, Some(0));
        assert_eq!(blocks[2].text, "four");
    }

    #[test]
    fn label_transcript_skips_whitespace_only_segments_without_breaking_a_run() {
        let diar = [spk(0.0, 3.0, 0)];
        let transcript = [
            TranscriptSegment::new(0.0, 1.0, "keep"),
            TranscriptSegment::new(1.0, 2.0, "   "),
            TranscriptSegment::new(2.0, 3.0, "going"),
        ];
        let blocks = label_transcript(&transcript, &diar);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].text, "keep going");
    }

    #[test]
    fn label_transcript_marks_unattributed_segments_and_merges_them() {
        // No diarization at all: everything is one Unknown block.
        let transcript = [
            TranscriptSegment::new(0.0, 1.0, "who"),
            TranscriptSegment::new(1.0, 2.0, "said this"),
        ];
        let blocks = label_transcript(&transcript, &[]);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].speaker, None);
        assert_eq!(blocks[0].text, "who said this");
    }

    #[test]
    fn empty_transcript_yields_no_blocks_and_empty_string() {
        assert!(label_transcript(&[], &[]).is_empty());
        assert_eq!(assemble(&[], &[]), "");
    }

    #[test]
    fn all_whitespace_transcript_yields_empty_output() {
        let transcript = [
            TranscriptSegment::new(0.0, 1.0, "  "),
            TranscriptSegment::new(1.0, 2.0, "\t\n"),
        ];
        assert_eq!(assemble(&transcript, &[]), "");
    }

    #[test]
    fn format_transcript_renders_one_based_labels_and_unknown() {
        let blocks = vec![
            LabeledBlock {
                speaker: Some(0),
                start_secs: 0.0,
                end_secs: 1.0,
                text: "Morning all.".to_string(),
            },
            LabeledBlock {
                speaker: Some(1),
                start_secs: 1.0,
                end_secs: 2.0,
                text: "Morning.".to_string(),
            },
            LabeledBlock {
                speaker: None,
                start_secs: 2.0,
                end_secs: 3.0,
                text: "Off-mic aside.".to_string(),
            },
        ];
        assert_eq!(
            format_transcript(&blocks),
            "Speaker 1: Morning all.\nSpeaker 2: Morning.\nUnknown: Off-mic aside."
        );
    }

    #[test]
    fn assemble_produces_a_full_labeled_transcript() {
        let diar = [spk(0.0, 1.5, 0), spk(1.5, 3.0, 1)];
        let transcript = [
            TranscriptSegment::new(0.0, 0.8, "Did the build pass?"),
            TranscriptSegment::new(0.8, 1.5, "Checking now."),
            TranscriptSegment::new(1.5, 3.0, "Green on my end."),
        ];
        assert_eq!(
            assemble(&transcript, &diar),
            "Speaker 1: Did the build pass? Checking now.\nSpeaker 2: Green on my end."
        );
    }

    #[test]
    fn transcript_segment_from_stt_segment_converts_centiseconds() {
        let seg = crate::stt::engine::Segment {
            text: "hello".to_string(),
            start_cs: 120,
            end_cs: 340,
            no_speech_prob: None,
            avg_token_prob: None,
        };
        let converted = TranscriptSegment::from(&seg);
        assert_eq!(converted.start_secs, 1.2);
        assert_eq!(converted.end_secs, 3.4);
        assert_eq!(converted.text, "hello");
    }
}
