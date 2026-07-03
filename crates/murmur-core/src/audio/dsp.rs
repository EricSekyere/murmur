//! Pure signal-processing helpers shared across the audio pipeline.
//!
//! These have no capture-backend (cpal) dependency, so they live outside the
//! `audio`-gated `capture` module and stay available in every feature
//! configuration. `AudioBuffer::from_raw` and the dictation preview both
//! resample here, so `--no-default-features` builds still compile.

/// Anti-aliased resampler: applies a low-pass filter before downsampling
/// to prevent aliasing artifacts, then uses linear interpolation.
pub(crate) fn resample(samples: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if from_rate == to_rate || samples.is_empty() {
        return samples.to_vec();
    }

    // When downsampling, apply an anti-aliasing low-pass filter at the
    // target Nyquist frequency (to_rate / 2) to prevent aliasing.
    let source = if from_rate > to_rate {
        lowpass_antialias(samples, from_rate, to_rate)
    } else {
        samples.to_vec()
    };

    let ratio = from_rate as f64 / to_rate as f64;
    let output_len = (source.len() as f64 / ratio) as usize;
    let mut output = Vec::with_capacity(output_len);

    for i in 0..output_len {
        let src_pos = i as f64 * ratio;
        let idx = src_pos as usize;
        let frac = (src_pos - idx as f64) as f32;

        let sample = if idx + 1 < source.len() {
            source[idx] * (1.0 - frac) + source[idx + 1] * frac
        } else {
            source[idx.min(source.len() - 1)]
        };

        output.push(sample);
    }

    output
}

/// Two-pass (forward + backward) single-pole IIR low-pass filter.
///
/// Applied before downsampling to prevent aliasing. The two-pass approach
/// gives second-order rolloff (~12dB/octave) with zero phase distortion.
/// Cutoff is set to the target Nyquist frequency (to_rate / 2).
fn lowpass_antialias(samples: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    // RC time constant for cutoff at target_nyquist = to_rate / 2
    let cutoff_hz = to_rate as f32 / 2.0;
    let rc = 1.0 / (std::f32::consts::TAU * cutoff_hz);
    let dt = 1.0 / from_rate as f32;
    let alpha = dt / (rc + dt);

    // An empty input has no signal to filter; bail before any indexing.
    let Some(&first) = samples.first() else {
        return Vec::new();
    };

    // Forward pass
    let mut filtered = Vec::with_capacity(samples.len());
    let mut prev = first;
    for &s in samples {
        prev += alpha * (s - prev);
        filtered.push(prev);
    }

    // Backward pass (zero-phase: eliminates phase distortion from forward pass).
    // `filtered` matches `samples`' non-zero length, so `last()` is always Some.
    let Some(&last) = filtered.last() else {
        return filtered;
    };
    prev = last;
    for s in filtered.iter_mut().rev() {
        prev += alpha * (*s - prev);
        *s = prev;
    }

    filtered
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resample_identity_when_rates_match() {
        let signal = vec![0.1, -0.2, 0.3, -0.4];
        assert_eq!(resample(&signal, 16_000, 16_000), signal);
    }

    #[test]
    fn resample_empty_input_stays_empty() {
        assert!(resample(&[], 48_000, 16_000).is_empty());
    }

    #[test]
    fn resample_downsample_scales_length_by_ratio() {
        // 300 samples at 48 kHz -> ~100 at 16 kHz (ratio 3:1).
        let signal = vec![0.0_f32; 300];
        let out = resample(&signal, 48_000, 16_000);
        assert_eq!(out.len(), 100);
    }

    #[test]
    fn resample_upsample_increases_length() {
        let signal = vec![0.0_f32; 100];
        let out = resample(&signal, 8_000, 16_000);
        assert_eq!(out.len(), 200);
    }

    #[test]
    fn lowpass_preserves_a_constant_signal() {
        // A DC signal has no high-frequency content to attenuate, so a
        // zero-phase low-pass should return it essentially unchanged.
        let dc = vec![0.5_f32; 64];
        let out = lowpass_antialias(&dc, 48_000, 16_000);
        for v in out {
            assert!((v - 0.5).abs() < 1e-3, "constant signal drifted: {v}");
        }
    }
}
