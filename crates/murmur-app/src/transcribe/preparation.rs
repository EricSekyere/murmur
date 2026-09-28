//! Signal checks run before normalization so gain cannot turn noise into speech.

use murmur_core::audio::silence::compute_rms;
use murmur_core::config::TranscriptionProfile;

const SAMPLE_RATE: f32 = 16_000.0;
const FRAME_SIZE: usize = 512;
const CONTEXT_FRAMES: usize = 5;
const MIN_AUDIBLE_FRAMES: usize = 2;
const RELATIVE_TRIM_THRESHOLD: f32 = 0.1;

pub(crate) struct PreparedAudio {
    pub samples: Vec<f32>,
    /// Signal levels before normalization, used for diagnostics and rejection.
    pub peak: f32,
    pub rms: f32,
    pub duration_secs: f32,
}

struct Limits {
    min_duration: f32,
    trim_threshold: f32,
    min_peak: f32,
    min_rms: f32,
}

impl Limits {
    fn for_profile(profile: TranscriptionProfile) -> Self {
        match profile {
            TranscriptionProfile::Relaxed => Self {
                min_duration: 0.12,
                trim_threshold: 0.003,
                min_peak: 0.008,
                min_rms: 0.0008,
            },
            TranscriptionProfile::Strict => Self {
                min_duration: 0.15,
                trim_threshold: 0.005,
                min_peak: 0.012,
                min_rms: 0.0012,
            },
        }
    }
}

pub(crate) fn prepare_audio(
    samples: &[f32],
    profile: TranscriptionProfile,
) -> Result<PreparedAudio, &'static str> {
    let limits = Limits::for_profile(profile);
    if samples.is_empty() {
        return Err("empty_audio");
    }
    if samples.iter().any(|s| !s.is_finite()) {
        return Err("invalid_audio");
    }
    if samples.len() as f32 / SAMPLE_RATE < limits.min_duration {
        return Err("too_short_raw");
    }
    let trimmed = trim_silence(samples, limits.trim_threshold);
    let duration_secs = trimmed.len() as f32 / SAMPLE_RATE;
    if duration_secs < limits.min_duration {
        return Err("too_short_trimmed");
    }
    let peak = trimmed.iter().map(|s| s.abs()).fold(0.0_f32, f32::max);
    let rms = compute_rms(trimmed);
    if peak < limits.min_peak || rms < limits.min_rms {
        return Err("too_quiet");
    }
    Ok(PreparedAudio {
        samples: normalize_peak(trimmed, peak),
        peak,
        rms,
        duration_secs,
    })
}

fn normalize_peak(samples: &[f32], peak: f32) -> Vec<f32> {
    if peak >= 0.1 || peak <= 0.0 {
        return samples.to_vec();
    }
    let scale = (0.5 / peak).min(5.0);
    samples
        .iter()
        .map(|s| (s * scale).clamp(-1.0, 1.0))
        .collect()
}

fn trim_silence(samples: &[f32], threshold: f32) -> &[f32] {
    let loudest_frame = samples
        .chunks(FRAME_SIZE)
        .map(compute_rms)
        .fold(0.0_f32, f32::max);
    if loudest_frame == 0.0 {
        return &samples[..0];
    }
    // A fixed floor cuts quiet words even when the clip passes the signal
    // checks. Adapt only trimming; admission still uses the raw peak and RMS.
    let threshold = threshold.min(loudest_frame * RELATIVE_TRIM_THRESHOLD);
    let mut audible = samples
        .chunks(FRAME_SIZE)
        .enumerate()
        .filter_map(|(i, frame)| (compute_rms(frame) >= threshold).then_some(i));
    let Some(first) = audible.next() else {
        return &samples[..0];
    };
    let mut last = first;
    let mut audible_frames = 1;
    for index in audible {
        last = index;
        audible_frames += 1;
    }
    // Context padding must not make a one-frame click look long enough to decode.
    if audible_frames < MIN_AUDIBLE_FRAMES {
        return &samples[..0];
    }
    let start = first.saturating_sub(CONTEXT_FRAMES) * FRAME_SIZE;
    // Keep 160 ms around speech so quiet consonants survive at either edge.
    let end = ((last + 1 + CONTEXT_FRAMES) * FRAME_SIZE).min(samples.len());
    &samples[start..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signals_below_admission_floors_are_rejected() {
        for profile in [TranscriptionProfile::Relaxed, TranscriptionProfile::Strict] {
            for level in [0.0, 0.0005, 0.002] {
                let samples = vec![level; 16_000];
                assert!(prepare_audio(&samples, profile).is_err(), "level {level}");
            }
        }
    }

    #[test]
    fn normalization_cannot_rescue_below_floor_signal() {
        let samples = vec![0.004; 16_000];
        assert!(matches!(
            prepare_audio(&samples, TranscriptionProfile::Relaxed),
            Err("too_quiet")
        ));
    }

    #[test]
    fn preserves_quiet_word_endings_and_trims_dead_air() {
        let mut samples = vec![0.0; 16_000];
        samples[4_096..8_192].fill(0.1);
        samples[8_192..8_992].fill(0.002);
        let trimmed = trim_silence(&samples, 0.003);
        assert_eq!(trimmed, &samples[1_536..10_752]);
    }

    #[test]
    fn quiet_speech_is_still_normalized_with_original_diagnostics() {
        let samples = vec![0.02; 16_000];
        let prepared =
            prepare_audio(&samples, TranscriptionProfile::Relaxed).expect("audible signal");
        assert_eq!(prepared.peak, 0.02);
        assert!((prepared.rms - 0.02).abs() < 0.0001);
        assert!(prepared.samples[0] > samples[0]);
    }

    #[test]
    fn quiet_waveform_below_absolute_trim_floor_survives() {
        let mut samples: Vec<f32> = (0..16_000)
            .map(|i| 0.002 * (std::f32::consts::TAU * i as f32 / 64.0).sin())
            .collect();
        for i in (0..samples.len()).step_by(64) {
            samples[i] = 0.01;
        }
        assert!(samples.chunks(FRAME_SIZE).all(|f| compute_rms(f) < 0.003));
        let prepared = prepare_audio(&samples, TranscriptionProfile::Relaxed)
            .expect("quiet signal above raw admission floors");
        assert_eq!(prepared.samples.len(), samples.len());
        assert!(prepared.rms >= 0.0008);
    }

    #[test]
    fn quiet_phrase_edges_survive_volume_changes() {
        let mut samples = vec![0.0; 16_384];
        samples[4_096..6_144].fill(0.005);
        samples[6_144..10_240].fill(0.02);
        samples[10_240..12_288].fill(0.005);
        let quiet: Vec<f32> = samples.iter().map(|s| s * 0.5).collect();
        for audio in [&samples, &quiet] {
            let prepared =
                prepare_audio(audio, TranscriptionProfile::Relaxed).expect("quiet phrase edges");
            assert_eq!(prepared.samples.len(), 13_312);
        }
    }

    #[test]
    fn invalid_audio_is_rejected_before_inference() {
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let mut samples = vec![0.1; 16_000];
            samples[8_000] = value;
            assert!(matches!(
                prepare_audio(&samples, TranscriptionProfile::Relaxed),
                Err("invalid_audio")
            ));
        }
    }

    #[test]
    fn short_click_is_rejected_but_short_word_survives() {
        let mut samples = vec![0.0; 16_000];
        samples[8_000] = 0.5;
        assert!(prepare_audio(&samples, TranscriptionProfile::Relaxed).is_err());
        samples[8_000..12_000].fill(0.05);
        assert!(prepare_audio(&samples, TranscriptionProfile::Relaxed).is_ok());
    }
}
