#[cfg(any(feature = "stt", feature = "parakeet"))]
use anyhow::Context;
use anyhow::Result;
use std::panic::AssertUnwindSafe;
use std::time::Instant;

#[cfg(feature = "stt")]
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

/// Result of a transcription.
#[derive(Debug, Clone)]
pub struct TranscriptionResult {
    /// The transcribed text.
    pub text: String,
    /// Processing time in milliseconds.
    pub processing_time_ms: u64,
    /// Number of segments returned by the model.
    pub segments: Vec<Segment>,
}

/// A single transcription segment with timestamps.
#[derive(Debug, Clone)]
pub struct Segment {
    pub text: String,
    /// Start time in centiseconds (10ms units).
    pub start_cs: i64,
    /// End time in centiseconds (10ms units).
    pub end_cs: i64,
}

/// Speech-to-text engine supporting multiple backends.
pub struct SttEngine {
    inner: EngineInner,
    model_path: String,
}

enum EngineInner {
    #[cfg(feature = "stt")]
    Whisper { ctx: WhisperContext, n_threads: i32 },
    #[cfg(feature = "parakeet")]
    Parakeet {
        engine: Box<parakeet_rs::ParakeetTDT>,
    },
    /// Stub backend when no STT features are enabled.
    #[allow(dead_code)]
    Stub,
}

impl SttEngine {
    /// Create a new STT engine with a Whisper model file.
    ///
    /// `n_threads` controls how many CPU threads to use for inference.
    /// Pass 0 to auto-detect (uses 4 or available cores, whichever is less).
    pub fn new_whisper(model_path: &str, n_threads: i32) -> Result<Self> {
        let n_threads = if n_threads <= 0 {
            std::thread::available_parallelism()
                .map(|n| n.get().min(4) as i32)
                .unwrap_or(4)
        } else {
            n_threads
        };

        #[cfg(feature = "stt")]
        {
            whisper_rs::install_logging_hooks();

            let ctx =
                WhisperContext::new_with_params(model_path, WhisperContextParameters::default())
                    .context("Failed to load whisper model")?;

            tracing::info!(
                "Whisper engine loaded model: {} ({} threads)",
                model_path,
                n_threads
            );

            Ok(Self {
                inner: EngineInner::Whisper { ctx, n_threads },
                model_path: model_path.to_string(),
            })
        }

        #[cfg(not(feature = "stt"))]
        {
            let _ = n_threads;
            tracing::warn!("STT feature not enabled, whisper engine is a no-op stub");
            Ok(Self {
                inner: EngineInner::Stub,
                model_path: model_path.to_string(),
            })
        }
    }

    /// Create a new STT engine with a Parakeet model directory.
    ///
    /// The directory must contain: encoder-model.onnx, decoder_joint-model.onnx, vocab.txt
    ///
    /// Automatically initializes the ONNX Runtime environment (loads DLL) on first call.
    pub fn new_parakeet(model_dir: &str) -> Result<Self> {
        #[cfg(feature = "parakeet")]
        {
            // Ensure ONNX Runtime DLL is loaded before creating any sessions
            super::runtime::init_ort().context("Failed to initialize ONNX Runtime for Parakeet")?;

            let engine = parakeet_rs::ParakeetTDT::from_pretrained(model_dir, None)
                .map_err(|e| anyhow::anyhow!("Failed to load Parakeet model: {}", e))?;

            tracing::info!("Parakeet engine loaded model from: {}", model_dir);

            Ok(Self {
                inner: EngineInner::Parakeet {
                    engine: Box::new(engine),
                },
                model_path: model_dir.to_string(),
            })
        }

        #[cfg(not(feature = "parakeet"))]
        {
            let _ = model_dir;
            tracing::warn!("Parakeet feature not enabled, engine is a no-op stub");
            Ok(Self {
                inner: EngineInner::Stub,
                model_path: model_dir.to_string(),
            })
        }
    }

    /// Backward-compatible constructor (delegates to new_whisper).
    pub fn new(model_path: &str, n_threads: i32) -> Result<Self> {
        Self::new_whisper(model_path, n_threads)
    }

    /// Transcribe raw PCM audio samples (16kHz mono f32) to text.
    pub fn transcribe(&mut self, samples: &[f32]) -> Result<TranscriptionResult> {
        if samples.is_empty() {
            return Ok(TranscriptionResult {
                text: String::new(),
                processing_time_ms: 0,
                segments: Vec::new(),
            });
        }

        let start = Instant::now();

        match &mut self.inner {
            #[cfg(feature = "stt")]
            EngineInner::Whisper { ctx, n_threads } => {
                Self::transcribe_whisper(ctx, *n_threads, samples, start)
            }
            #[cfg(feature = "parakeet")]
            EngineInner::Parakeet { engine } => Self::transcribe_parakeet(engine, samples, start),
            EngineInner::Stub => {
                tracing::warn!("No STT backend enabled, returning empty transcription");
                Ok(TranscriptionResult {
                    text: String::new(),
                    processing_time_ms: start.elapsed().as_millis() as u64,
                    segments: Vec::new(),
                })
            }
        }
    }

    #[cfg(feature = "stt")]
    fn transcribe_whisper(
        ctx: &WhisperContext,
        n_threads: i32,
        samples: &[f32],
        start: Instant,
    ) -> Result<TranscriptionResult> {
        let mut state = ctx
            .create_state()
            .context("Failed to create whisper state")?;

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_n_threads(n_threads);
        params.set_language(Some("en"));
        params.set_translate(false);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_single_segment(true);
        params.set_no_context(true);
        params.set_no_timestamps(false);
        params.set_suppress_blank(true);
        params.set_suppress_nst(true);
        // Encourage proper punctuation and capitalization via initial prompt.
        // Whisper uses this as a style hint for the decoder.
        params.set_initial_prompt("Use proper punctuation and capitalization.");
        // Disable temperature fallback retries — each retry on large models
        // allocates significant memory and can cause OOM/crashes.
        params.set_temperature(0.0);
        params.set_temperature_inc(0.0);

        // Wrap inference in catch_unwind to prevent whisper.cpp panics from
        // crashing the entire application (larger models hit more edge cases).
        let inference_result = std::panic::catch_unwind(AssertUnwindSafe(|| {
            state.full(params, samples)
        }));

        match inference_result {
            Ok(Ok(_)) => {}
            Ok(Err(e)) => {
                return Err(anyhow::anyhow!("Whisper transcription failed: {:?}", e));
            }
            Err(panic_info) => {
                let msg = if let Some(s) = panic_info.downcast_ref::<String>() {
                    s.clone()
                } else if let Some(s) = panic_info.downcast_ref::<&str>() {
                    (*s).to_string()
                } else {
                    "unknown panic".to_string()
                };
                tracing::error!("Whisper inference panicked: {}", msg);
                return Err(anyhow::anyhow!("Whisper inference panicked: {}", msg));
            }
        }

        let n_segments = state.full_n_segments();
        let mut segments = Vec::new();
        let mut full_text = String::new();

        for i in 0..n_segments {
            if let Some(seg) = state.get_segment(i) {
                let text = seg
                    .to_str_lossy()
                    .map_err(|e| anyhow::anyhow!("Failed to read segment text: {:?}", e))?
                    .into_owned();

                segments.push(Segment {
                    text: text.clone(),
                    start_cs: seg.start_timestamp(),
                    end_cs: seg.end_timestamp(),
                });
                full_text.push_str(&text);
            }
        }

        let elapsed = start.elapsed();
        tracing::debug!(
            "Whisper transcribed {} samples in {}ms -> {} segments",
            samples.len(),
            elapsed.as_millis(),
            segments.len()
        );

        Ok(TranscriptionResult {
            text: full_text.trim().to_string(),
            processing_time_ms: elapsed.as_millis() as u64,
            segments,
        })
    }

    #[cfg(feature = "parakeet")]
    fn transcribe_parakeet(
        engine: &mut parakeet_rs::ParakeetTDT,
        samples: &[f32],
        start: Instant,
    ) -> Result<TranscriptionResult> {
        use parakeet_rs::Transcriber;

        let result = engine
            .transcribe_samples(samples.to_vec(), 16000, 1, None)
            .map_err(|e| anyhow::anyhow!("Parakeet transcription failed: {}", e))?;

        let elapsed = start.elapsed();
        tracing::debug!(
            "Parakeet transcribed {} samples in {}ms",
            samples.len(),
            elapsed.as_millis(),
        );

        Ok(TranscriptionResult {
            text: result.text.trim().to_string(),
            processing_time_ms: elapsed.as_millis() as u64,
            segments: Vec::new(),
        })
    }

    /// Get the path to the loaded model.
    pub fn model_path(&self) -> &str {
        &self.model_path
    }
}
