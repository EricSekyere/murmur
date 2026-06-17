//! Silero Voice Activity Detection.
//!
//! Wraps the Silero VAD v5 ONNX model. The pipeline accepts 512-sample
//! frames at 16 kHz, runs them through the model with a carried LSTM state
//! and a 64-sample context window, and returns a speech probability in
//! `[0.0, 1.0]`. RMS-based detection still exists in `audio::silence` and
//! remains the fallback when the `vad` feature is disabled or the model
//! file isn't on disk.
//!
//! ONNX I/O (Silero v5):
//!   inputs:
//!     - "input": f32 [1, 576] = context(64) || frame(512)
//!     - "state": f32 [2, 1, 128] (carried)
//!     - "sr":    i64 scalar (16000)
//!   outputs:
//!     - "output": f32 [1, 1] speech probability
//!     - "stateN": f32 [2, 1, 128] new state

use anyhow::Result;

/// Voice Activity Detection state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VadState {
    /// No speech detected.
    Silence,
    /// Speech is currently being detected.
    Speech,
}

/// Number of audio samples Silero v5 expects per frame at 16 kHz.
/// Equals 32 ms; smaller is rejected by the model.
pub const SILERO_FRAME_SAMPLES: usize = 512;

/// Number of context samples carried over from the previous frame.
/// Silero v5 prepends these to each new 512-sample window.
pub const SILERO_CONTEXT_SAMPLES: usize = 64;

/// Recommended speech-probability threshold. Lower than the Whisper-VAD
/// default (0.5) because Silero is conservative; transcribe-rs uses 0.3.
pub const DEFAULT_THRESHOLD: f32 = 0.3;

/// Silero VAD wrapper using ONNX Runtime.
///
/// Detects speech start/end in audio frames so the STT engine never sees
/// pure silence (a major source of "Thanks for watching!" hallucinations).
pub struct VoiceActivityDetector {
    threshold: f32,
    state: VadState,

    #[cfg(feature = "vad")]
    inner: Option<SileroInner>,
}

#[cfg(feature = "vad")]
struct SileroInner {
    session: ort::session::Session,
    /// LSTM hidden state, shape [2, 1, 128] f32. Persists across calls.
    lstm_state: Vec<f32>,
    /// Last 64 samples from the previous frame, prepended to the next one.
    context: Vec<f32>,
}

impl VoiceActivityDetector {
    /// Create a new VAD instance.
    ///
    /// `model_path` should point to the Silero VAD ONNX model file
    /// (silero_vad.onnx, ~2 MB). If loading fails, the VAD falls back to
    /// a passive `Silence` state and `process()` becomes a no-op so the
    /// caller can keep its RMS-based fallback path.
    ///
    /// `threshold` is the speech probability threshold (recommended: 0.3).
    pub fn new(model_path: &str, threshold: f32) -> Result<Self> {
        #[cfg(feature = "vad")]
        {
            // The ORT runtime has to be initialised before any session is
            // created; the parakeet path already does this via stt::runtime.
            // Reusing the same init keeps both code paths consistent and
            // avoids loading the DLL twice.
            crate::stt::runtime::init_ort()
                .map_err(|e| anyhow::anyhow!("ORT init failed for VAD: {}", e))?;

            let session = ort::session::Session::builder()
                .map_err(|e| anyhow::anyhow!("Failed to build VAD session: {}", e))?
                .commit_from_file(model_path)
                .map_err(|e| anyhow::anyhow!("Failed to load VAD model: {}", e))?;

            tracing::info!(
                "Silero VAD initialized from {} (threshold {:.2})",
                model_path,
                threshold
            );

            Ok(Self {
                threshold,
                state: VadState::Silence,
                inner: Some(SileroInner {
                    session,
                    lstm_state: vec![0.0; 2 * 128],
                    context: vec![0.0; SILERO_CONTEXT_SAMPLES],
                }),
            })
        }

        #[cfg(not(feature = "vad"))]
        {
            let _ = model_path;
            tracing::warn!("VAD feature not enabled, VoiceActivityDetector is a no-op stub");
            Ok(Self {
                threshold,
                state: VadState::Silence,
            })
        }
    }

    /// Process one 512-sample frame and return whether speech is detected.
    ///
    /// `samples` must be 16 kHz mono f32 of length `SILERO_FRAME_SAMPLES`.
    /// Frames of the wrong length are skipped (with a debug log) and the
    /// previous state is returned so transient buffer hiccups don't spuriously
    /// flip speech state.
    pub fn process(&mut self, samples: &[f32]) -> Result<VadState> {
        if samples.len() != SILERO_FRAME_SAMPLES {
            tracing::debug!(
                "VAD: expected {} samples, got {} — skipping frame",
                SILERO_FRAME_SAMPLES,
                samples.len()
            );
            return Ok(self.state);
        }

        #[cfg(feature = "vad")]
        {
            let prob = self.run_inference(samples)?;
            self.state = if prob >= self.threshold {
                VadState::Speech
            } else {
                VadState::Silence
            };
            let state_for_log = self.state;
            tracing::trace!(
                prob,
                threshold = self.threshold,
                state = ?state_for_log,
                "VAD frame"
            );
        }

        #[cfg(not(feature = "vad"))]
        {
            let _ = samples;
        }

        Ok(self.state)
    }

    /// Reset the LSTM state and 64-sample context. Call between utterances
    /// — Silero's hidden state assumes a continuous stream within an
    /// utterance, and stale state across phrase boundaries causes false
    /// detections during the start of a new phrase.
    pub fn reset(&mut self) {
        self.state = VadState::Silence;
        #[cfg(feature = "vad")]
        if let Some(inner) = &mut self.inner {
            for v in &mut inner.lstm_state {
                *v = 0.0;
            }
            for v in &mut inner.context {
                *v = 0.0;
            }
        }
    }

    pub fn state(&self) -> VadState {
        self.state
    }

    pub fn threshold(&self) -> f32 {
        self.threshold
    }

    #[cfg(feature = "vad")]
    fn run_inference(&mut self, samples: &[f32]) -> Result<f32> {
        let inner = self
            .inner
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("VAD inference called on a stub VAD"))?;
        inner.run(samples)
    }
}

#[cfg(feature = "vad")]
impl SileroInner {
    fn run(&mut self, frame: &[f32]) -> Result<f32> {
        debug_assert_eq!(frame.len(), SILERO_FRAME_SAMPLES);

        // Build [context (64) || frame (512)] = 576 samples.
        let mut input_data = Vec::with_capacity(SILERO_CONTEXT_SAMPLES + SILERO_FRAME_SAMPLES);
        input_data.extend_from_slice(&self.context);
        input_data.extend_from_slice(frame);

        // Update context BEFORE moving input_data into the tensor — saves
        // a 576-float clone per frame. Matches the reference C++ impl:
        // `std::copy(new_data.end() - context_samples, new_data.end(), _context.begin())`.
        let tail_start = input_data.len() - SILERO_CONTEXT_SAMPLES;
        self.context.copy_from_slice(&input_data[tail_start..]);
        let input_len = input_data.len() as i64;

        let input_tensor = ort::value::Tensor::from_array((vec![1_i64, input_len], input_data))
            .map_err(|e| anyhow::anyhow!("VAD input tensor: {}", e))?;

        let state_tensor =
            ort::value::Tensor::from_array((vec![2_i64, 1, 128], self.lstm_state.clone()))
                .map_err(|e| anyhow::anyhow!("VAD state tensor: {}", e))?;

        // Sample-rate input. Silero accepts a 1-D `[1]` int64 tensor.
        let sr_tensor = ort::value::Tensor::from_array((vec![1_i64], vec![16000_i64]))
            .map_err(|e| anyhow::anyhow!("VAD sr tensor: {}", e))?;

        let outputs = self
            .session
            .run(ort::inputs![
                "input" => input_tensor,
                "state" => state_tensor,
                "sr" => sr_tensor,
            ])
            .map_err(|e| anyhow::anyhow!("VAD inference failed: {}", e))?;

        // Speech probability — output is shape [1, 1] f32; first element.
        let (_, prob_data) = outputs["output"]
            .try_extract_tensor::<f32>()
            .map_err(|e| anyhow::anyhow!("VAD output extract: {}", e))?;
        let prob = *prob_data.first().unwrap_or(&0.0);

        // Update LSTM state from "stateN".
        let (_, new_state) = outputs["stateN"]
            .try_extract_tensor::<f32>()
            .map_err(|e| anyhow::anyhow!("VAD stateN extract: {}", e))?;
        if new_state.len() == self.lstm_state.len() {
            self.lstm_state.copy_from_slice(new_state);
        } else {
            tracing::warn!(
                "VAD stateN size mismatch: expected {}, got {}; resetting",
                self.lstm_state.len(),
                new_state.len()
            );
            for v in &mut self.lstm_state {
                *v = 0.0;
            }
        }

        Ok(prob)
    }
}

// ── Model download ──────────────────────────────────────────────────────────

#[cfg(feature = "vad")]
const SILERO_VAD_URL: &str =
    "https://github.com/snakers4/silero-vad/raw/master/src/silero_vad/data/silero_vad.onnx";

/// Pinned SHA256 of the Silero VAD model (empty until pinned).
#[cfg(feature = "vad")]
const SILERO_VAD_SHA256: &str = "";

/// Filesystem path where the Silero VAD ONNX model is cached.
#[cfg(feature = "vad")]
pub fn model_path() -> Result<std::path::PathBuf> {
    let dir = dirs::data_dir()
        .ok_or_else(|| anyhow::anyhow!("Could not determine data directory"))?
        .join("murmur")
        .join("vad");
    Ok(dir.join("silero_vad.onnx"))
}

#[cfg(feature = "vad")]
pub fn is_downloaded() -> bool {
    model_path().map(|p| p.exists()).unwrap_or(false)
}

/// Download the Silero VAD ONNX model (~2 MB) into the user's data dir.
/// Idempotent: returns the cached path if the file already exists.
#[cfg(feature = "vad")]
pub async fn download() -> Result<std::path::PathBuf> {
    use anyhow::Context;
    let dest = model_path()?;
    if dest.exists() {
        return Ok(dest);
    }

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).context("Failed to create VAD model directory")?;
    }

    tracing::info!("Downloading Silero VAD model to {}", dest.display());
    let bytes = reqwest::get(SILERO_VAD_URL)
        .await
        .context("Silero VAD download request failed")?
        .error_for_status()
        .context("Silero VAD download non-OK status")?
        .bytes()
        .await
        .context("Failed to read Silero VAD response body")?;

    crate::integrity::verify_or_log_sha256(&bytes, SILERO_VAD_SHA256, "Silero VAD model")?;

    // Write atomically: tempfile + rename.
    let tmp = dest.with_extension("partial");
    std::fs::write(&tmp, &bytes).context("Failed to write Silero VAD model")?;
    std::fs::rename(&tmp, &dest).context("Failed to finalize Silero VAD model")?;

    tracing::info!("Silero VAD model ready ({} bytes)", bytes.len());
    Ok(dest)
}
