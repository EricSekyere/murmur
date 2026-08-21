//! Windowing for backends that do not segment long audio themselves.
//!
//! whisper.cpp windows internally, so only the Parakeet path needs this.
//! Its ONNX encoder carries a fixed positional encoding and simply fails
//! past the end of it, and long input degrades badly well before that point.

use std::ops::Range;

/// Samples per second the STT path is normalised to before inference.
const RATE: usize = 16_000;

/// Longest audio handed to Parakeet in one inference.
///
/// This is a safety net against a hard failure, not an accuracy control.
/// The encoder's positional encoding ends at 5000 frames, which at the
/// model's 80 ms stride is 400 s, and a single call past it fails outright
/// with a broadcast error: 490 s of speech returned nothing at all rather
/// than a partial transcript. 240 s leaves generous margin.
///
/// It is deliberately not lower. Shorter windows do not recover accuracy on
/// long audio, because Parakeet degrades with input length no matter where
/// the boundaries fall, and windowing shorter measurably made mid-length
/// input worse: a 33 s passage scored 96% coverage in one call against 68%
/// when split. Anything under this threshold is therefore left exactly as it
/// was. Long recordings belong on the Whisper backend, which windows
/// internally and scored 99.8% coverage on the same 490 s audio.
pub(crate) const PARAKEET_MAX_CHUNK_SAMPLES: usize = 240 * RATE;

/// Energy at or below this fraction of the search region's loudest probe
/// counts as silence.
///
/// This is strict because the failure it avoids is severe rather than
/// gradual. Parakeet returns an empty transcript for audio that begins part
/// way through a word: slicing one clean utterance at 0 s yielded 16 words
/// while the same clip cut at 2, 4, 6 and 8 s each yielded nothing. A cut
/// must therefore land in a real gap so the next window opens on a word
/// boundary, and merely picking the quietest available point is not enough
/// when the whole region is continuous speech.
const SILENCE_RATIO: f32 = 0.02;

/// Shortest silent run treated as a pause, in probes. 100 ms clears the stop
/// closures inside normal speech, which would otherwise offer boundaries that
/// sit mid-word and defeat the point of cutting at silence at all.
const MIN_SILENT_PROBES: usize = 5;

/// Width of the moving window used to score quietness. 20 ms is short enough
/// to find a gap between words and long enough not to trip on a single
/// zero crossing inside a vowel.
const PROBE_SAMPLES: usize = RATE / 50;

/// Split `samples` into consecutive windows of at most `max_len`.
///
/// Boundaries are placed in silence wherever silence exists. The audio is
/// first divided at every silent gap, then consecutive speech runs are packed
/// greedily into windows, so a window ends at a real pause rather than at a
/// fixed offset. Searching only near a nominal boundary is not enough: gaps
/// arrive at the speaker's pace, not the window's, so a fixed search region
/// misses them whenever a sentence runs long.
///
/// The ranges tile the input exactly, no overlap and no gaps, so transcripts
/// concatenate without a seam to dedupe.
///
/// A `max_len` of zero yields a single range covering everything, since a
/// zero-length window could not make progress. Continuous speech longer than
/// `max_len` is cut at `max_len`, since something has to give.
pub(crate) fn windows(samples: &[f32], max_len: usize) -> Vec<Range<usize>> {
    if max_len == 0 || samples.len() <= max_len {
        return std::iter::once(0..samples.len()).collect();
    }
    let target = max_len;

    // Whole segments only. Cutting inside one splits an utterance, and the
    // window that then begins part way through a word decodes as nothing.
    // Two utterances of 16.7 s and 16.0 s scored 37/39 and 32/32 words when
    // kept whole, against 48 of 71 when the same audio was cut near a fixed
    // target instead.
    let mut out: Vec<Range<usize>> = Vec::new();
    for segment in segments(samples) {
        for piece in split_oversized(segment, max_len) {
            match out.last_mut() {
                // Merge only while the result still fits the target, so short
                // segments coalesce and long ones stand alone.
                Some(last) if piece.end - last.start <= target => last.end = piece.end,
                _ => out.push(piece),
            }
        }
    }
    if out.is_empty() {
        return std::iter::once(0..samples.len()).collect();
    }
    out
}

/// Speech runs delimited by silence, tiling the input exactly.
fn segments(samples: &[f32]) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut start = 0;
    for boundary in silence_boundaries(samples) {
        if boundary > start {
            out.push(start..boundary);
            start = boundary;
        }
    }
    if start < samples.len() {
        out.push(start..samples.len());
    }
    out
}

/// Break a segment longer than `max_len` into equal pieces.
///
/// Only reached by unbroken speech with no pause to cut at, where any
/// boundary is arbitrary; equal pieces at least keep each one short.
fn split_oversized(segment: Range<usize>, max_len: usize) -> Vec<Range<usize>> {
    let len = segment.end - segment.start;
    if len <= max_len {
        return vec![segment];
    }
    let pieces = len.div_ceil(max_len);
    let each = len.div_ceil(pieces);
    (0..pieces)
        .map(|i| {
            let lo = segment.start + i * each;
            (lo..(lo + each).min(segment.end)).clone()
        })
        .filter(|r| !r.is_empty())
        .collect()
}

/// Midpoints of every silent run in `samples`, ascending.
fn silence_boundaries(samples: &[f32]) -> Vec<usize> {
    let probes = probe_energies(samples, 0, samples.len());
    if probes.is_empty() {
        return Vec::new();
    }
    let loudest = probes.iter().copied().fold(0.0_f32, f32::max);
    if loudest <= 0.0 {
        return Vec::new();
    }
    let floor = loudest * SILENCE_RATIO;

    let mut out = Vec::new();
    let mut run_start: Option<usize> = None;
    for (i, &e) in probes.iter().enumerate() {
        if e <= floor {
            run_start.get_or_insert(i);
        } else if let Some(s) = run_start.take() {
            push_boundary(&mut out, s, i);
        }
    }
    if let Some(s) = run_start {
        push_boundary(&mut out, s, probes.len());
    }
    out
}

/// Record the middle of a silent run, ignoring runs too brief to be a pause.
fn push_boundary(out: &mut Vec<usize>, first: usize, last: usize) {
    if last - first < MIN_SILENT_PROBES {
        return;
    }
    out.push((first + last) * PROBE_SAMPLES / 2);
}

/// Mean square energy of each probe-sized slice of `[lo, hi)`.
fn probe_energies(samples: &[f32], lo: usize, hi: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity((hi - lo) / PROBE_SAMPLES + 1);
    let mut probe = lo;
    while probe < hi {
        let stop = (probe + PROBE_SAMPLES).min(hi);
        let sum: f32 = samples[probe..stop].iter().map(|s| s * s).sum();
        out.push(sum / (stop - probe).max(1) as f32);
        probe += PROBE_SAMPLES;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(n: usize) -> Vec<f32> {
        (0..n).map(|i| (i as f32 * 0.05).sin() * 0.5).collect()
    }

    #[test]
    fn short_audio_is_a_single_window() {
        let a = tone(RATE);
        assert_eq!(windows(&a, PARAKEET_MAX_CHUNK_SAMPLES), vec![0..RATE]);
    }

    #[test]
    fn exactly_max_is_still_one_window() {
        let a = tone(100);
        assert_eq!(windows(&a, 100), vec![0..100]);
    }

    #[test]
    fn windows_tile_the_input_without_gaps_or_overlap() {
        // The seam is the whole point: transcripts are concatenated, so any
        // gap loses words and any overlap repeats them.
        let a = tone(10_000);
        let w = windows(&a, 1_000);
        assert!(w.len() > 1);
        assert_eq!(w[0].start, 0);
        assert_eq!(w[w.len() - 1].end, a.len());
        for pair in w.windows(2) {
            assert_eq!(pair[0].end, pair[1].start, "seam {pair:?}");
        }
    }

    #[test]
    fn no_window_exceeds_the_maximum() {
        let a = tone(10_000);
        for r in windows(&a, 1_000) {
            assert!(r.len() <= 1_000, "window {r:?} over max");
        }
    }

    #[test]
    fn every_window_makes_progress() {
        // A zero-length window would loop forever.
        let a = tone(5_000);
        for r in windows(&a, 300) {
            assert!(!r.is_empty(), "empty window {r:?}");
        }
    }

    /// Speech-scale fixture: `secs` of tone with silent gaps punched in.
    fn with_gaps(secs: f32, gaps: &[(f32, f32)]) -> Vec<f32> {
        let mut a = tone((secs * RATE as f32) as usize);
        let n = a.len();
        for &(at, len) in gaps {
            let lo = (at * RATE as f32) as usize;
            let hi = (((at + len) * RATE as f32) as usize).min(n);
            for s in &mut a[lo..hi] {
                *s = 0.0;
            }
        }
        a
    }

    #[test]
    fn the_cut_prefers_a_silent_gap() {
        let a = with_gaps(6.0, &[(3.5, 0.3)]);
        let cut = windows(&a, 4 * RATE)[0].end;
        let secs = cut as f32 / RATE as f32;
        assert!(
            (3.5..3.8).contains(&secs),
            "cut at {secs:.2}s missed the gap at 3.5-3.8s"
        );
    }

    #[test]
    fn a_real_gap_beats_a_merely_quieter_moment() {
        // The regression this guards: cutting at the quietest available point
        // lands mid-word when the region is unbroken speech, and the window
        // that follows transcribes as nothing. A true gap must win even when a
        // softer passage sits closer to the nominal boundary.
        let mut a = with_gaps(6.0, &[(3.0, 0.3)]);
        for s in &mut a[(3.6 * RATE as f32) as usize..(3.9 * RATE as f32) as usize] {
            *s *= 0.3;
        }
        let cut = windows(&a, 4 * RATE)[0].end;
        let secs = cut as f32 / RATE as f32;
        assert!(
            (3.0..3.3).contains(&secs),
            "cut at {secs:.2}s took the quiet passage instead of the gap"
        );
    }

    #[test]
    fn the_latest_usable_gap_is_chosen() {
        let a = with_gaps(6.0, &[(2.6, 0.1), (3.4, 0.5)]);
        let cut = windows(&a, 4 * RATE)[0].end;
        let secs = cut as f32 / RATE as f32;
        assert!(
            (3.4..3.9).contains(&secs),
            "cut at {secs:.2}s not inside the latest gap at 3.4-3.9s"
        );
    }

    #[test]
    fn uniform_audio_still_terminates_and_covers_everything() {
        let a = vec![0.4_f32; 7_777];
        let w = windows(&a, 1_000);
        assert_eq!(w[0].start, 0);
        assert_eq!(w[w.len() - 1].end, a.len());
        for pair in w.windows(2) {
            assert_eq!(pair[0].end, pair[1].start);
        }
    }

    #[test]
    fn empty_input_yields_one_empty_range() {
        assert_eq!(windows(&[], 1_000), vec![0..0]);
    }

    #[test]
    fn zero_max_len_degrades_to_a_single_window() {
        let a = tone(500);
        assert_eq!(windows(&a, 0), vec![0..500]);
    }

    #[test]
    fn a_long_input_is_split_below_the_encoder_limit() {
        // 8 minutes: the duration that previously failed outright.
        let a = vec![0.1_f32; 480 * RATE];
        let w = windows(&a, PARAKEET_MAX_CHUNK_SAMPLES);
        assert!(w.len() > 1, "480 s must be split, got {}", w.len());
        // 5000 encoder frames at 80 ms is the hard ceiling; stay well under.
        for r in &w {
            assert!(r.len() <= PARAKEET_MAX_CHUNK_SAMPLES);
        }
        assert_eq!(w[w.len() - 1].end, a.len());
    }
}
