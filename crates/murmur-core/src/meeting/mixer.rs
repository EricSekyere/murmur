//! Pure two-stream meeting mixer: microphone + system-audio (loopback) into
//! one 16 kHz mono stream on the microphone's clock.
//!
//! Hardware-free by design so it unit-tests with synthetic vectors. The mic is
//! the timekeeper: it delivers continuously (a connected mic produces silence
//! samples even when nobody speaks), so total mic samples pushed define the
//! meeting timeline. WASAPI loopback is the opposite — it delivers **zero
//! callbacks while nothing is playing** (see [`crate::audio::loopback`]), so
//! its samples arrive in bursts that must be placed on the mic timeline by
//! wall time, with the gaps left as silence. Without that gap fill the two
//! streams drift apart by exactly the far side's quiet time.
//!
//! Mixing sums the two aligned streams with a proportional scale-back when
//! the sum would leave full scale, never a hard truncation of one side.

/// Everything in the meeting pipeline runs at the STT rate.
const SAMPLE_RATE: u32 = crate::audio::AudioBuffer::SAMPLE_RATE;

/// Mixes periodic pulls of mic and loopback audio (both already 16 kHz mono)
/// into one stream on the mic clock.
///
/// Feed with [`Self::push`] on a cadence (the app uses ~500 ms), drain with
/// [`Self::take`]. Positions are tracked as absolute sample counts since the
/// meeting start so alignment cannot drift with the number of pulls.
#[derive(Debug, Default)]
pub struct MeetingMixer {
    /// Mixed, not-yet-drained output.
    mixed: Vec<f32>,
    /// Total mic samples ever pushed — the meeting timeline length.
    timeline_end: u64,
    /// Absolute timeline position of `mixed[0]` (samples already drained).
    drained: u64,
    /// Absolute timeline position up to which loopback audio has been placed.
    /// Monotonic, so jittered arrival estimates can never double-mix a span.
    loopback_pos: u64,
}

impl MeetingMixer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Meeting duration so far, in seconds of mic timeline.
    pub fn duration_secs(&self) -> f32 {
        self.timeline_end as f32 / SAMPLE_RATE as f32
    }

    /// Feed one pull: `mic` is the next contiguous mic slice (it advances the
    /// clock), `loopback` is whatever system audio arrived since the previous
    /// pull, and `loopback_lag_secs` is how far before "now" (the end of the
    /// mic slice) the newest loopback sample was delivered — pass 0.0 when
    /// both buffers were drained at the same instant, the normal case.
    ///
    /// The loopback slice is placed ending `loopback_lag_secs` before the new
    /// timeline end; any span between the previous loopback position and the
    /// slice start stays mic-only (the zero-fill for loopback starvation).
    pub fn push(&mut self, mic: &[f32], loopback: &[f32], loopback_lag_secs: f32) {
        self.mixed.extend_from_slice(mic);
        self.timeline_end += mic.len() as u64;

        if loopback.is_empty() {
            return;
        }
        let lag = (loopback_lag_secs.max(0.0) * SAMPLE_RATE as f32).round() as u64;
        let end = self.timeline_end.saturating_sub(lag);
        let start = end.saturating_sub(loopback.len() as u64);
        // Clamp to what is still mixable: never behind already-placed loopback
        // (monotonic, no double-mix) and never behind drained output. Trimming
        // the slice head instead of shifting it keeps the tail — the freshest
        // audio — placed at its true wall position, which bounds drift.
        let start = start.max(self.loopback_pos).max(self.drained);
        if start >= end {
            self.loopback_pos = self.loopback_pos.max(end);
            return;
        }
        let skip = (loopback.len() as u64 - (end - start)) as usize;
        let offset = (start - self.drained) as usize;
        for (i, &sys) in loopback[skip..].iter().enumerate() {
            let slot = &mut self.mixed[offset + i];
            *slot = mix_sample(*slot, sys);
        }
        self.loopback_pos = end;
    }

    /// Drain the mixed stream accumulated so far.
    pub fn take(&mut self) -> Vec<f32> {
        self.drained += self.mixed.len() as u64;
        std::mem::take(&mut self.mixed)
    }
}

/// Sum two samples, scaling the sum back to full scale when it would exceed
/// ±1.0. Scaling (rather than clamping an already-computed wave) keeps the
/// sign and never emits out-of-range or non-finite values.
fn mix_sample(a: f32, b: f32) -> f32 {
    let sum = a + b;
    if sum.abs() > 1.0 {
        sum / sum.abs()
    } else {
        sum
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: usize = SAMPLE_RATE as usize;

    #[test]
    fn mic_only_passes_through() {
        let mut mixer = MeetingMixer::new();
        let mic = vec![0.25_f32; 800];
        mixer.push(&mic, &[], 0.0);
        assert_eq!(mixer.take(), mic);
        assert!((mixer.duration_secs() - 0.05).abs() < 1e-6);
    }

    #[test]
    fn loopback_aligns_to_the_end_of_the_mic_slice() {
        let mut mixer = MeetingMixer::new();
        let mic = vec![0.0_f32; 1000];
        let sys = vec![0.5_f32; 400];
        mixer.push(&mic, &sys, 0.0);
        let out = mixer.take();
        assert_eq!(out.len(), 1000);
        // Loopback occupies the trailing 400 samples; the head stays mic-only.
        assert!(out[..600].iter().all(|&s| s == 0.0));
        assert!(out[600..].iter().all(|&s| s == 0.5));
    }

    #[test]
    fn lag_shifts_loopback_placement_back_in_time() {
        let mut mixer = MeetingMixer::new();
        let mic = vec![0.0_f32; 1600];
        let sys = vec![0.5_f32; 160];
        // 25 ms lag at 16 kHz = 400 samples before the timeline end.
        mixer.push(&mic, &sys, 0.025);
        let out = mixer.take();
        assert!(out[1040..1200].iter().all(|&s| s == 0.5));
        assert!(out[..1040].iter().all(|&s| s == 0.0));
        assert!(out[1200..].iter().all(|&s| s == 0.0));
    }

    #[test]
    fn loopback_gaps_stay_zero_filled() {
        let mut mixer = MeetingMixer::new();
        // Far side quiet for two pulls (loopback starvation: no samples at
        // all), then talks again. The quiet span must remain mic-only, and
        // the resumed audio lands at its wall position, not right after the
        // previous burst.
        mixer.push(&vec![0.0; 800], &vec![0.5; 800], 0.0);
        mixer.push(&vec![0.0; 800], &[], 0.0);
        mixer.push(&vec![0.0; 800], &[], 0.0);
        mixer.push(&vec![0.0; 800], &vec![0.5; 800], 0.0);
        let out = mixer.take();
        assert_eq!(out.len(), 3200);
        assert!(out[..800].iter().all(|&s| s == 0.5), "first burst");
        assert!(out[800..2400].iter().all(|&s| s == 0.0), "gap zero-filled");
        assert!(out[2400..].iter().all(|&s| s == 0.5), "resumed burst");
    }

    #[test]
    fn summing_soft_clips_instead_of_overflowing() {
        let mut mixer = MeetingMixer::new();
        mixer.push(&vec![0.8_f32; 100], &vec![0.8_f32; 100], 0.0);
        let out = mixer.take();
        assert!(out.iter().all(|&s| (-1.0..=1.0).contains(&s)));
        assert!(out.iter().all(|&s| s == 1.0));

        let mut mixer = MeetingMixer::new();
        mixer.push(&vec![-0.9_f32; 100], &vec![-0.9_f32; 100], 0.0);
        assert!(mixer.take().iter().all(|&s| s == -1.0));

        // Below full scale the sum is untouched.
        let mut mixer = MeetingMixer::new();
        mixer.push(&vec![0.25_f32; 100], &vec![0.25_f32; 100], 0.0);
        assert!(mixer.take().iter().all(|&s| (s - 0.5).abs() < 1e-6));
    }

    #[test]
    fn overlapping_arrival_estimates_never_double_mix() {
        let mut mixer = MeetingMixer::new();
        // Two bursts whose wall-time estimates overlap by half a burst: the
        // second burst's overlapped head is trimmed, so no sample receives
        // loopback audio twice (which would read 1.0 after soft-clip).
        mixer.push(&vec![0.0; 800], &vec![0.6; 800], 0.0);
        mixer.push(&vec![0.0; 400], &vec![0.6; 800], 0.0);
        let out = mixer.take();
        assert!(out.iter().all(|&s| s == 0.0 || s == 0.6));
    }

    #[test]
    fn drift_stays_bounded_over_simulated_minutes() {
        let mut mixer = MeetingMixer::new();
        let pull = RATE / 2; // 500 ms cadence
        let minutes = 5;
        let mut total = Vec::new();
        // Loopback alternates 10 s of delivery with 10 s of starvation, the
        // realistic worst case for clock drift. Delivered bursts always carry
        // 0.5; the mic carries 0.0, so misplacement is directly visible.
        for tick in 0..(minutes * 60 * 2) {
            let delivering = (tick / 20) % 2 == 0;
            let sys = if delivering {
                vec![0.5; pull]
            } else {
                Vec::new()
            };
            mixer.push(&vec![0.0; pull], &sys, 0.0);
            total.extend(mixer.take());
        }
        assert_eq!(total.len(), minutes * 60 * RATE);
        // Every delivering 10s block must be loopback audio at its own wall
        // position; every starved block must be silent. Zero tolerance: the
        // absolute-position bookkeeping cannot accumulate error.
        for (block, chunk) in total.chunks(10 * RATE).enumerate() {
            let expected = if block % 2 == 0 { 0.5 } else { 0.0 };
            assert!(
                chunk.iter().all(|&s| s == expected),
                "block {block} drifted"
            );
        }
    }

    #[test]
    fn take_then_late_loopback_does_not_panic_or_misplace() {
        let mut mixer = MeetingMixer::new();
        mixer.push(&vec![0.0; 800], &[], 0.0);
        let _ = mixer.take();
        // A loopback burst claiming to start inside already-drained audio is
        // trimmed to the drain point rather than indexing out of range.
        mixer.push(&vec![0.0; 100], &vec![0.5; 800], 0.0);
        let out = mixer.take();
        assert_eq!(out.len(), 100);
        assert!(out.iter().all(|&s| s == 0.5));
    }
}
