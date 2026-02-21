use anyhow::Result;

/// Voice Activity Detection state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VadState {
    /// No speech detected.
    Silence,
    /// Speech is currently being detected.
    Speech,
}

/// Silero VAD wrapper using ONNX Runtime.
///
/// Detects speech start/end in audio frames to avoid sending silence
/// to the STT engine.
///
/// The Silero VAD v5 model expects:
/// - Input: 512 samples at 16kHz (32ms frames)
/// - Internal state: h and c tensors (LSTM hidden state)
/// - Output: speech probability (0.0 - 1.0)
pub struct VoiceActivityDetector {
    /// Probability threshold above which a frame is considered speech.
    threshold: f32,
    state: VadState,
    // TODO: ort::Session, h/c state tensors will go here once model is available
}

impl VoiceActivityDetector {
    /// Create a new VAD instance.
    ///
    /// `model_path` should point to the Silero VAD ONNX model file.
    /// `threshold` is the speech probability threshold (default: 0.5).
    pub fn new(_model_path: &str, threshold: f32) -> Result<Self> {
        // TODO: Load ONNX model via ort::Session::builder().commit_from_file(model_path)
        // TODO: Initialize h and c state tensors as zeros
        tracing::info!("VAD initialized (stub) with threshold {}", threshold);
        Ok(Self {
            threshold,
            state: VadState::Silence,
        })
    }

    /// Process an audio frame and return whether speech is detected.
    ///
    /// `samples` should be 16kHz mono f32 audio, 512 samples (32ms frame).
    pub fn process(&mut self, _samples: &[f32]) -> Result<VadState> {
        // TODO: Run ONNX inference:
        // 1. Create input tensor from samples [1, 512]
        // 2. Pass h, c state tensors
        // 3. Run session
        // 4. Extract speech probability from output
        // 5. Update h, c state from output
        // 6. Compare probability against threshold
        Ok(self.state)
    }

    /// Reset the internal LSTM state. Call between utterances.
    pub fn reset(&mut self) {
        self.state = VadState::Silence;
        // TODO: Reset h and c state tensors to zeros
    }

    /// Get the current VAD state.
    pub fn state(&self) -> VadState {
        self.state
    }

    /// Get the configured threshold.
    pub fn threshold(&self) -> f32 {
        self.threshold
    }
}
