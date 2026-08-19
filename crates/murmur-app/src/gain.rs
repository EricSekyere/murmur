//! Session gain staging: boost quiet raw-mic audio without ever clipping.

/// Ceiling for a boosted sample's magnitude. Kept below full scale so
/// downstream downmix/resample arithmetic cannot round back up to 1.0.
const HEADROOM_PEAK: f32 = 0.9;

/// Per-chunk recovery factor toward the calibrated gain after a reduction.
/// ~2% per 50ms chunk: unity back to a 3x boost takes ~2.8s of quiet audio,
/// slow enough that levels never pump audibly, fast enough that one keyboard
/// click cannot cancel the boost for the rest of the session.
const RELEASE_PER_CHUNK: f32 = 1.02;

/// Applies the calibrated mic boost with a running-peak ceiling.
///
/// Calibration derives the boost from the ambient floor alone, which says
/// nothing about how loud speech will be; in a quiet room the naive product
/// lands past full scale, and a hard clamp squares the waveform off, which
/// destroys the formant cues STT needs to tell words apart. Here the gain is
/// capped so each chunk's peak stays at [`HEADROOM_PEAK`], and after a
/// reduction it releases back toward the calibrated boost a little per chunk
/// ([`RELEASE_PER_CHUNK`]). Sustained loud speech re-caps every chunk, so
/// levels stay stable while it lasts; a lone transient (a click, the tail of
/// the wake phrase) only dents the gain briefly instead of latching it at
/// unity and starving the rest of the session. Within a chunk the output is
/// always a purely linear scale of the input. The raw signal is never
/// attenuated: unity is the floor.
pub(crate) struct GainStage {
    calibrated: f32,
    gain: f32,
}

impl GainStage {
    pub(crate) fn new(calibrated_gain: f32) -> Self {
        let calibrated = calibrated_gain.max(1.0);
        Self {
            calibrated,
            gain: calibrated,
        }
    }

    /// Scale `mono` by the effective gain: release toward the calibrated
    /// boost, then lower the gain (never below unity) if this chunk's peak
    /// would exceed the ceiling.
    pub(crate) fn apply(&mut self, mut mono: Vec<f32>) -> Vec<f32> {
        if self.calibrated <= 1.0 {
            return mono;
        }
        self.gain = (self.gain * RELEASE_PER_CHUNK).min(self.calibrated);
        let peak = mono.iter().fold(0.0_f32, |m, s| m.max(s.abs()));
        if peak > 0.0 {
            let ceiling = (HEADROOM_PEAK / peak).max(1.0);
            if ceiling < self.gain {
                self.gain = ceiling;
            }
        }
        if self.gain <= 1.0 {
            return mono;
        }
        for s in &mut mono {
            *s *= self.gain;
        }
        mono
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peak(samples: &[f32]) -> f32 {
        samples.iter().fold(0.0_f32, |m, s| m.max(s.abs()))
    }

    /// The regression scenario: calibration picked 4.2x in a quiet room, then
    /// speech peaked at 0.3. Multiply-and-clamp pinned the output at 1.0
    /// (field logs showed crest factor ~2), turning words into confident
    /// mis-decodes downstream.
    #[test]
    fn boosted_speech_never_hits_full_scale() {
        let mut stage = GainStage::new(4.2);
        let chunk: Vec<f32> = (0..480)
            .map(|i| 0.3 * (i as f32 / 480.0 * std::f32::consts::TAU).sin())
            .collect();
        let out = stage.apply(chunk.clone());
        let out_peak = peak(&out);
        assert!(out_peak < 1.0, "boosted peak reached full scale");
        assert!(
            out_peak <= HEADROOM_PEAK + 1e-4,
            "peak {out_peak} above headroom"
        );
        // Still boosted, and still linear: one scale factor for the whole
        // chunk, no flattened tops.
        assert!(out_peak > 0.85);
        let k = out[100] / chunk[100];
        assert!(k > 1.0);
        for (o, i) in out.iter().zip(&chunk) {
            assert!((o - i * k).abs() < 1e-6, "waveform shape distorted");
        }
    }

    #[test]
    fn quiet_input_keeps_the_full_calibrated_boost() {
        let mut stage = GainStage::new(4.2);
        let chunk = vec![0.01_f32, -0.02, 0.05, -0.01];
        let out = stage.apply(chunk.clone());
        for (o, i) in out.iter().zip(&chunk) {
            assert!((o - i * 4.2).abs() < 1e-6);
        }
    }

    #[test]
    fn sustained_loud_speech_stays_capped_without_pumping() {
        let mut stage = GainStage::new(4.2);
        let capped = HEADROOM_PEAK / 0.3;
        for _ in 0..40 {
            let out = stage.apply(vec![0.3_f32, -0.3]);
            // Every chunk of ongoing loud speech re-caps the gain, so the
            // release never lifts levels mid-utterance.
            assert!((out[0] - 0.3 * capped).abs() < 1e-3, "level pumped");
        }
    }

    #[test]
    fn reduction_releases_gently_instead_of_snapping_back() {
        let mut stage = GainStage::new(4.2);
        let _ = stage.apply(vec![0.3, -0.3]);
        // The next quiet chunk gets the reduced gain plus one release step,
        // never the full 4.2x back at once.
        let reduced = HEADROOM_PEAK / 0.3;
        let out = stage.apply(vec![0.05_f32]);
        let k = out[0] / 0.05;
        assert!(k >= reduced, "release moved the wrong way");
        assert!(
            k <= reduced * RELEASE_PER_CHUNK + 1e-4,
            "gain snapped back: {k}"
        );
    }

    /// The starvation bug this release exists for: a single click chunk used
    /// to latch the session at unity, silently cancelling the calibrated
    /// boost for every later phrase.
    #[test]
    fn a_transient_cannot_starve_the_rest_of_the_session() {
        let mut stage = GainStage::new(3.0);
        // One impulsive chunk with a peak at the ceiling forces unity.
        let out = stage.apply(vec![0.92_f32]);
        assert!((out[0] - 0.92).abs() < 1e-6, "raw transient was attenuated");
        // ~6s of ordinary quiet speech afterwards: the boost must come back.
        let mut k = 0.0;
        for _ in 0..120 {
            let out = stage.apply(vec![0.05_f32]);
            k = out[0] / 0.05;
        }
        assert!(k > 2.9, "gain stayed starved after a transient: {k}");
        assert!(k <= 3.0 + 1e-4);
    }

    #[test]
    fn raw_signal_is_never_attenuated() {
        let mut stage = GainStage::new(5.0);
        let chunk = vec![0.95_f32, 0.1];
        assert_eq!(stage.apply(chunk.clone()), chunk);
    }

    #[test]
    fn unity_gain_is_a_passthrough() {
        let mut stage = GainStage::new(1.0);
        let chunk = vec![0.4_f32, -0.9, 0.99];
        assert_eq!(stage.apply(chunk.clone()), chunk);
    }
}
