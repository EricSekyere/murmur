use anyhow::Result;
#[cfg(feature = "stt")]
use anyhow::Context;
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

/// Whisper-based speech-to-text engine.
pub struct SttEngine {
    #[cfg(feature = "stt")]
    ctx: WhisperContext,
    #[cfg(feature = "stt")]
    n_threads: i32,
    model_path: String,
}

impl SttEngine {
    /// Create a new STT engine with the given model file.
    ///
    /// `n_threads` controls how many CPU threads to use for inference.
    /// Pass 0 to auto-detect (uses 4 or available cores, whichever is less).
    pub fn new(model_path: &str, n_threads: i32) -> Result<Self> {
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

            let ctx = WhisperContext::new_with_params(model_path, WhisperContextParameters::default())
                .context("Failed to load whisper model")?;

            tracing::info!(
                "STT engine loaded model: {} ({} threads)",
                model_path,
                n_threads
            );

            Ok(Self {
                ctx,
                model_path: model_path.to_string(),
                n_threads,
            })
        }

        #[cfg(not(feature = "stt"))]
        {
            let _ = n_threads;
            tracing::warn!("STT feature not enabled, engine is a no-op stub");
            Ok(Self {
                model_path: model_path.to_string(),
            })
        }
    }

    /// Transcribe raw PCM audio samples (16kHz mono f32) to text.
    pub fn transcribe(&self, samples: &[f32]) -> Result<TranscriptionResult> {
        if samples.is_empty() {
            return Ok(TranscriptionResult {
                text: String::new(),
                processing_time_ms: 0,
                segments: Vec::new(),
            });
        }

        let start = Instant::now();

        #[cfg(feature = "stt")]
        {
            let mut state = self.ctx.create_state().context("Failed to create whisper state")?;

            let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
            params.set_n_threads(self.n_threads);
            params.set_language(Some("en"));
            params.set_translate(false);
            params.set_print_special(false);
            params.set_print_progress(false);
            params.set_print_realtime(false);
            params.set_print_timestamps(false);
            params.set_single_segment(false);
            params.set_no_timestamps(false);
            params.set_suppress_blank(true);
            params.set_suppress_nst(true);

            state
                .full(params, samples)
                .map_err(|e| anyhow::anyhow!("Whisper transcription failed: {:?}", e))?;

            let n_segments = state.full_n_segments();
            let mut segments = Vec::new();
            let mut full_text = String::new();

            for i in 0..n_segments {
                if let Some(seg) = state.get_segment(i) {
                    let text = seg.to_str_lossy()
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
                "Transcribed {} samples in {}ms -> {} segments",
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

        #[cfg(not(feature = "stt"))]
        {
            let _ = samples;
            tracing::warn!("STT feature not enabled, returning empty transcription");
            Ok(TranscriptionResult {
                text: String::new(),
                processing_time_ms: start.elapsed().as_millis() as u64,
                segments: Vec::new(),
            })
        }
    }

    /// Get the path to the loaded model.
    pub fn model_path(&self) -> &str {
        &self.model_path
    }
}
