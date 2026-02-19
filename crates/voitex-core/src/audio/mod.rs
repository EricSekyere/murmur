#[cfg(feature = "audio")]
pub mod capture;
pub mod vad;

/// Holds captured PCM audio samples (16kHz mono f32).
/// Always available regardless of feature flags.
#[derive(Debug, Clone)]
pub struct AudioBuffer {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
}

impl AudioBuffer {
    pub const SAMPLE_RATE: u32 = 16_000;

    pub fn new() -> Self {
        Self {
            samples: Vec::new(),
            sample_rate: Self::SAMPLE_RATE,
        }
    }

    pub fn duration_secs(&self) -> f32 {
        self.samples.len() as f32 / self.sample_rate as f32
    }
}
