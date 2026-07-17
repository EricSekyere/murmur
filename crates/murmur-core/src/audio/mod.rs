#[cfg(feature = "audio")]
pub(crate) mod aec_health;
#[cfg(feature = "audio")]
pub mod capture;
pub(crate) mod dsp;
#[cfg(all(feature = "audio", target_os = "linux"))]
pub mod pulse_aec;
pub mod silence;
pub mod vad;
#[cfg(feature = "audio")]
pub(crate) mod warm;
#[cfg(all(feature = "audio", windows))]
pub mod wasapi;

/// Holds captured PCM audio samples (16kHz mono f32).
/// Always available regardless of feature flags.
#[derive(Debug, Clone)]
pub struct AudioBuffer {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
}

impl Default for AudioBuffer {
    fn default() -> Self {
        Self {
            samples: Vec::new(),
            sample_rate: Self::SAMPLE_RATE,
        }
    }
}

impl AudioBuffer {
    pub const SAMPLE_RATE: u32 = 16_000;

    pub fn new() -> Self {
        Self::default()
    }

    /// Build from raw multi-channel audio at a native rate, downmixing and
    /// resampling to 16 kHz mono.
    pub fn from_raw(raw: &[f32], native_rate: u32, native_channels: u16) -> Self {
        let mono = if native_channels > 1 {
            let ch = native_channels as usize;
            raw.chunks_exact(ch)
                .map(|frame| frame.iter().sum::<f32>() / ch as f32)
                .collect::<Vec<f32>>()
        } else {
            raw.to_vec()
        };

        let target_rate = Self::SAMPLE_RATE;
        let resampled = if native_rate != target_rate {
            dsp::resample(&mono, native_rate, target_rate)
        } else {
            mono
        };

        Self {
            samples: resampled,
            sample_rate: target_rate,
        }
    }

    pub fn duration_secs(&self) -> f32 {
        self.samples.len() as f32 / self.sample_rate as f32
    }
}
