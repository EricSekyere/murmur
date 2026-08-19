//! Session gain staging: boost quiet raw-mic audio without ever clipping.

/// Ceiling for a boosted sample's magnitude. Kept below full scale so
/// downstream downmix/resample arithmetic cannot round back up to 1.0.
const HEADROOM_PEAK: f32 = 0.9;

/// Applies the calibrated mic boost with a running-peak ceiling.
///
/// Calibration derives the boost from the ambient floor alone, which says
/// nothing about how loud speech will be; in a quiet room the naive product
/// lands past full scale, and a hard clamp squares the waveform off, which
/// destroys the formant cues STT needs to tell words apart. Here the gain is
/// capped so each chunk's peak stays at [`HEADROOM_PEAK`], and every
/// reduction is kept for the rest of the session: the gain only ratchets
/// down, so levels stay stable (no pumping) and the output is always a
/// purely linear scale of the input. The raw signal is never attenuated:
/// unity is the floor.
pub(crate) struct GainStage {
    gain: f32,
}

impl GainStage {
    pub(crate) fn new(calibrated_gain: f32) -> Self {
        Self {
            gain: calibrated_gain.max(1.0),
        }
    }

    /// Scale `mono` by the effective gain, lowering the gain first (never
    /// below unity) if this chunk's peak would exceed the ceiling.
    pub(crate) fn apply(&mut self, mut mono: Vec<f32>) -> Vec<f32> {
        if self.gain <= 1.0 {
            return mono;
        }
        let peak = mono.iter().fold(0.0_f32, |m, s| m.max(s.abs()));
        if peak > 0.0 {
            let ceiling = HEADROOM_PEAK / peak;
            if ceiling < self.gain {
                self.gain = ceiling.max(1.0);
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
    fn gain_reduction_persists_instead_of_pumping() {
        let mut stage = GainStage::new(4.2);
        let _ = stage.apply(vec![0.3, -0.3]);
        // A later quiet chunk keeps the reduced gain (0.9 / 0.3), not 4.2x.
        let out = stage.apply(vec![0.05_f32]);
        assert!((out[0] - 0.05 * (HEADROOM_PEAK / 0.3)).abs() < 1e-6);
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
