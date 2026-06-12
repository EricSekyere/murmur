#[cfg(any(feature = "stt", feature = "parakeet"))]
use anyhow::Context;
use anyhow::Result;
#[cfg(any(feature = "stt", feature = "parakeet"))]
use std::panic::AssertUnwindSafe;
use std::time::Instant;

use super::models::SttModel;

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
    /// Whisper's "no speech" probability for this segment, in [0,1].
    /// High values (>~0.6) indicate the model thinks this segment is silence
    /// or non-speech and is hallucinating — caller should reject the result.
    /// `None` for backends that don't expose it (e.g., Parakeet).
    pub no_speech_prob: Option<f32>,
    /// Mean probability of the segment's text tokens, in [0,1]. Low values
    /// (<~0.4) mean the decoder was guessing — typical of hallucinations on
    /// sighs/breaths/noise. `None` for backends that don't expose it.
    pub avg_token_prob: Option<f32>,
}

/// Speech-to-text engine supporting multiple backends.
pub struct SttEngine {
    inner: EngineInner,
    model_path: String,
    /// Model variant hint for tuning inference parameters per model size.
    model: Option<SttModel>,
    /// Optional decoder context — used as Whisper's `initial_prompt` to
    /// preserve punctuation/capitalization continuity across phrases.
    /// Should be the trailing ~200 chars of the prior session transcript.
    initial_prompt: Option<String>,
}

/// Samples in 1ms at 16kHz. Used for leading-silence padding.
const SAMPLES_PER_MS: usize = 16;

/// Default leading-silence padding for Whisper. Whisper's mel preprocessor
/// attenuates the first ~50ms; 100ms keeps utterance starts intact.
#[cfg(feature = "stt")]
const WHISPER_LEAD_MS: usize = 100;

/// Default leading-silence padding for Parakeet. Its mel windowing drops
/// more of the start than Whisper — transcribe-rs uses 250ms.
#[cfg(feature = "parakeet")]
const PARAKEET_LEAD_MS: usize = 250;

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
    /// Pass 0 to auto-detect (uses 6 or available cores, whichever is less —
    /// more than the physical performance-core count regresses ggml's
    /// spin-wait barriers on hybrid Intel CPUs).
    pub fn new_whisper(model_path: &str, n_threads: i32) -> Result<Self> {
        let n_threads = if n_threads <= 0 {
            std::thread::available_parallelism()
                .map(|n| n.get().min(6) as i32)
                .unwrap_or(4)
        } else {
            n_threads
        };

        #[cfg(feature = "stt")]
        {
            whisper_rs::install_logging_hooks();

            #[cfg_attr(not(feature = "cuda"), allow(unused_mut))]
            let mut ctx_params = WhisperContextParameters::default();
            // With the CUDA backend, flash attention substantially speeds up
            // the encoder/decoder attention layers. (whisper-rs enables
            // use_gpu by default when a GPU feature is compiled in.)
            #[cfg(feature = "cuda")]
            {
                ctx_params.flash_attn(true);
                tracing::info!("Whisper CUDA backend enabled (flash attention on)");
            }

            let ctx = WhisperContext::new_with_params(model_path, ctx_params)
                .context("Failed to load whisper model")?;

            tracing::info!(
                "Whisper engine loaded model: {} ({} threads)",
                model_path,
                n_threads
            );

            Ok(Self {
                inner: EngineInner::Whisper { ctx, n_threads },
                model_path: model_path.to_string(),
                model: None,
                initial_prompt: None,
            })
        }

        #[cfg(not(feature = "stt"))]
        {
            let _ = n_threads;
            tracing::warn!("STT feature not enabled, whisper engine is a no-op stub");
            Ok(Self {
                inner: EngineInner::Stub,
                model_path: model_path.to_string(),
                model: None,
                initial_prompt: None,
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

            // Use DirectML (GPU) on Windows for ~5-10x faster inference.
            // Falls back to CPU automatically if GPU is unavailable.
            let config = parakeet_rs::ExecutionConfig::new()
                .with_execution_provider(parakeet_rs::ExecutionProvider::DirectML);

            tracing::info!(
                "Loading Parakeet model from {} with DirectML GPU acceleration...",
                model_dir
            );

            let engine = match parakeet_rs::ParakeetTDT::from_pretrained(model_dir, Some(config)) {
                Ok(e) => {
                    tracing::info!("Parakeet engine loaded successfully from: {}", model_dir);
                    e
                }
                Err(e) => {
                    tracing::error!(
                        "Parakeet model load failed (DirectML): {}. \
                         This may indicate a GPU compatibility issue. \
                         Check that DirectML is supported on this GPU.",
                        e
                    );
                    return Err(anyhow::anyhow!(
                        "Failed to load Parakeet model: {}. \
                         Try a Whisper model if this persists.",
                        e
                    ));
                }
            };

            Ok(Self {
                inner: EngineInner::Parakeet {
                    engine: Box::new(engine),
                },
                model_path: model_dir.to_string(),
                model: None,
                initial_prompt: None,
            })
        }

        #[cfg(not(feature = "parakeet"))]
        {
            let _ = model_dir;
            tracing::warn!("Parakeet feature not enabled, engine is a no-op stub");
            Ok(Self {
                inner: EngineInner::Stub,
                model_path: model_dir.to_string(),
                model: None,
                initial_prompt: None,
            })
        }
    }

    /// Backward-compatible constructor (delegates to new_whisper).
    pub fn new(model_path: &str, n_threads: i32) -> Result<Self> {
        Self::new_whisper(model_path, n_threads)
    }

    /// Set the model variant so the engine can tune inference parameters
    /// (temperature fallback, segment mode, etc.) per model size.
    pub fn set_model(&mut self, model: SttModel) {
        self.model = Some(model);
    }

    /// Set the running session transcript used as decoder context for the
    /// next call. Pass the trailing ~200 chars of prior output. Pass `None`
    /// to clear (e.g., on a fresh session).
    pub fn set_initial_prompt(&mut self, prompt: Option<String>) {
        self.initial_prompt = prompt;
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

        // Pad with leading silence so the model's mel-spectrogram windowing
        // doesn't eat the first phoneme. Different backends need different
        // amounts; segment timestamps are shifted back below to compensate.
        let lead_ms = match &self.inner {
            #[cfg(feature = "stt")]
            EngineInner::Whisper { .. } => WHISPER_LEAD_MS,
            #[cfg(feature = "parakeet")]
            EngineInner::Parakeet { .. } => PARAKEET_LEAD_MS,
            EngineInner::Stub => 0,
        };

        let padded: Vec<f32>;
        // `input` is only consumed by feature-gated arms; when no backend is
        // enabled (Stub-only build), the binding is intentionally unused.
        #[cfg_attr(
            not(any(feature = "stt", feature = "parakeet")),
            allow(unused_variables)
        )]
        let input: &[f32] = if lead_ms > 0 {
            let pad_samples = lead_ms * SAMPLES_PER_MS;
            padded = std::iter::repeat_n(0.0_f32, pad_samples)
                .chain(samples.iter().copied())
                .collect();
            &padded
        } else {
            samples
        };

        let mut result = match &mut self.inner {
            #[cfg(feature = "stt")]
            EngineInner::Whisper { ctx, n_threads } => Self::transcribe_whisper(
                ctx,
                *n_threads,
                input,
                start,
                self.model,
                self.initial_prompt.as_deref(),
            ),
            #[cfg(feature = "parakeet")]
            EngineInner::Parakeet { engine } => Self::transcribe_parakeet(engine, input, start),
            EngineInner::Stub => {
                tracing::warn!("No STT backend enabled, returning empty transcription");
                Ok::<TranscriptionResult, anyhow::Error>(TranscriptionResult {
                    text: String::new(),
                    processing_time_ms: start.elapsed().as_millis() as u64,
                    segments: Vec::new(),
                })
            }
        }?;

        // Shift segment timestamps back to account for the padding so callers
        // see times relative to the original audio. Centiseconds = 10ms units.
        if lead_ms > 0 {
            let lead_cs = (lead_ms / 10) as i64;
            for seg in &mut result.segments {
                seg.start_cs = (seg.start_cs - lead_cs).max(0);
                seg.end_cs = (seg.end_cs - lead_cs).max(0);
            }
        }

        Ok(result)
    }

    #[cfg(feature = "stt")]
    fn transcribe_whisper(
        ctx: &WhisperContext,
        n_threads: i32,
        samples: &[f32],
        start: Instant,
        model: Option<SttModel>,
        initial_prompt: Option<&str>,
    ) -> Result<TranscriptionResult> {
        let mut state = ctx
            .create_state()
            .context("Failed to create whisper state")?;

        let is_large = matches!(
            model,
            Some(SttModel::WhisperMediumEn | SttModel::WhisperLargeV3Turbo)
        );
        let is_base = matches!(model, Some(SttModel::WhisperBaseEn));
        let is_small = matches!(model, Some(SttModel::WhisperSmallEn) | None);

        // Live dictation needs phrase latency well under the phrase length,
        // or output lags further behind with every sentence. Greedy decoding
        // is 2-3x faster than beam search and nearly as accurate on small.en
        // for short conversational phrases — the same trade macOS dictation
        // makes. Larger models keep beam search but at width 3: width 5 on
        // CPU costs seconds per phrase for marginal accuracy.
        let mut params = if is_base || is_small {
            FullParams::new(SamplingStrategy::Greedy { best_of: 1 })
        } else {
            FullParams::new(SamplingStrategy::BeamSearch {
                beam_size: 3,
                patience: -1.0,
            })
        };
        params.set_n_threads(n_threads);
        params.set_language(Some("en"));
        params.set_translate(false);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        // Let whisper decide natural segment boundaries inside a chunk.
        params.set_single_segment(false);
        // Preserve intra-chunk decoder context for better punctuation/continuations.
        params.set_no_context(false);
        params.set_no_timestamps(false);
        params.set_suppress_blank(true);
        params.set_suppress_nst(true);
        // Encourage proper punctuation and capitalization via initial prompt.
        // Whisper uses this as a style hint for the decoder. When the caller
        // has provided session context (the trailing portion of the prior
        // transcript), append it so cross-phrase punctuation/capitalization
        // stays consistent — this is whisper.cpp's intended streaming pattern.
        let style_hint = "Use proper punctuation and capitalization.";
        let prompt_owned: String;
        let prompt: &str = if let Some(prev) = initial_prompt.filter(|s| !s.trim().is_empty()) {
            // Cap at ~200 chars to keep prompt token budget under control.
            let trimmed = prev.trim();
            let start_byte = trimmed
                .char_indices()
                .rev()
                .nth(200)
                .map(|(i, _)| i)
                .unwrap_or(0);
            prompt_owned = format!("{} {}", style_hint, &trimmed[start_byte..]);
            &prompt_owned
        } else {
            style_hint
        };
        params.set_initial_prompt(prompt);
        params.set_temperature(0.0);
        if is_large {
            // Larger models (medium, large-v3-turbo) hit more edge cases where
            // greedy decoding fails. Enable temperature fallback so the model
            // retries with randomness on bad decodes (high compression ratio or
            // low log probability). Increment of 0.4 gives at most 2 retries.
            params.set_temperature_inc(0.4);
        } else if is_base {
            // Base model: prefer deterministic low-latency decode.
            params.set_temperature_inc(0.0);
        } else {
            // Small and larger beam-search models benefit from a light fallback.
            params.set_temperature_inc(0.2);
        }

        // Wrap inference in catch_unwind to prevent whisper.cpp panics from
        // crashing the entire application (larger models hit more edge cases).
        let inference_result =
            std::panic::catch_unwind(AssertUnwindSafe(|| state.full(params, samples)));

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

                // whisper-rs 0.15 exposes this as `no_speech_probability()`;
                // the underlying FFI is `whisper_full_get_segment_no_speech_prob_from_state`.
                let no_speech = Some(seg.no_speech_probability());

                // Mean text-token probability — the decoder's own confidence.
                // Skip special tokens (ids >= EOT: timestamps, markers); their
                // probabilities aren't about the transcribed words.
                let eot = ctx.token_eot();
                let mut prob_sum = 0.0_f32;
                let mut prob_count = 0_u32;
                for t in 0..seg.n_tokens() {
                    if let Some(tok) = seg.get_token(t)
                        && tok.token_id() < eot
                    {
                        prob_sum += tok.token_probability();
                        prob_count += 1;
                    }
                }
                let avg_token_prob = (prob_count > 0).then(|| prob_sum / prob_count as f32);

                segments.push(Segment {
                    text: text.clone(),
                    start_cs: seg.start_timestamp(),
                    end_cs: seg.end_timestamp(),
                    no_speech_prob: no_speech,
                    avg_token_prob,
                });
                full_text.push_str(&text);
            }
        }

        let elapsed = start.elapsed();
        tracing::info!(
            "Whisper transcribed {} samples in {}ms -> {} segment(s), text={:?}",
            samples.len(),
            elapsed.as_millis(),
            segments.len(),
            if full_text.trim().is_empty() {
                "<empty>"
            } else {
                full_text.trim()
            }
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

        tracing::info!(
            "Parakeet: transcribing {} samples ({:.2}s)...",
            samples.len(),
            samples.len() as f32 / 16000.0
        );

        // Wrap parakeet inference in catch_unwind for the same reason we wrap
        // whisper.full(): the underlying ONNX Runtime / DirectML stack can
        // occasionally panic on edge-case inputs. Without this, a panic in
        // GPU code crashes the entire app instead of falling through to the
        // user-facing error path.
        let inference = std::panic::catch_unwind(AssertUnwindSafe(|| {
            engine.transcribe_samples(samples.to_vec(), 16000, 1, None)
        }));

        let result = match inference {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => {
                tracing::error!("Parakeet transcription call failed: {}", e);
                return Err(anyhow::anyhow!(
                    "Parakeet transcription failed: {}. \
                     This may indicate a DirectML/GPU issue.",
                    e
                ));
            }
            Err(panic_info) => {
                let msg = if let Some(s) = panic_info.downcast_ref::<String>() {
                    s.clone()
                } else if let Some(s) = panic_info.downcast_ref::<&str>() {
                    (*s).to_string()
                } else {
                    "unknown panic".to_string()
                };
                tracing::error!("Parakeet inference panicked: {}", msg);
                return Err(anyhow::anyhow!(
                    "Parakeet inference panicked: {}. \
                     This is usually a GPU/DirectML driver issue.",
                    msg
                ));
            }
        };

        let elapsed = start.elapsed();
        tracing::info!(
            "Parakeet transcribed {} samples in {}ms -> {:?}",
            samples.len(),
            elapsed.as_millis(),
            if result.text.is_empty() {
                "<empty>"
            } else {
                &result.text
            }
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
