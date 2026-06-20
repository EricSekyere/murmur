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
    /// Comma-separated user glossary (names, jargon) included in the prompt
    /// so whisper spells them correctly.
    vocabulary: Option<String>,
    /// Spoken-language hint: `None` or "auto" auto-detects, "en"/"es"/… force
    /// a language. Ignored for English-only models (always "en").
    language: Option<String>,
    /// Translate the recognized speech to English (multilingual models only).
    translate: bool,
}

/// Samples in 1ms at 16kHz. Used for leading-silence padding.
const SAMPLES_PER_MS: usize = 16;

/// Default leading-silence padding for Whisper. Whisper's mel preprocessor
/// attenuates the first ~50ms; 100ms keeps utterance starts intact.
#[cfg(feature = "stt")]
const WHISPER_LEAD_MS: usize = 100;

/// Char budget for the vocabulary clause in Whisper's prompt. The prompt window
/// is ~224 tokens, shared with the style hint and rolling context, so a long
/// glossary (a big manual list, or the codebase indexer) must be capped or it
/// crowds out decoding and degrades transcription.
#[cfg(feature = "stt")]
const MAX_VOCAB_PROMPT_CHARS: usize = 400;

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
                vocabulary: None,
                language: None,
                translate: false,
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
                vocabulary: None,
                language: None,
                translate: false,
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

            // Parakeet is a small int8 model on short dictation clips, so the
            // default CPU provider is the fast path: it warms up in ~90ms. A GPU
            // provider (DirectML) is a net loss here — its first inference spends
            // ~40s compiling shaders every launch, and the TDT/LSTM decoder
            // ping-pongs tensors host<->device. Use CPU on every platform.
            let config: Option<parakeet_rs::ExecutionConfig> = None;

            tracing::info!("Loading Parakeet model from {}...", model_dir);

            let engine = match parakeet_rs::ParakeetTDT::from_pretrained(model_dir, config) {
                Ok(e) => {
                    tracing::info!("Parakeet engine loaded from: {}", model_dir);
                    e
                }
                Err(e) => {
                    tracing::error!("Parakeet model load failed: {}", e);
                    return Err(anyhow::anyhow!(
                        "Failed to load Parakeet model: {}. Try a Whisper model if this persists.",
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
                vocabulary: None,
                language: None,
                translate: false,
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
                vocabulary: None,
                language: None,
                translate: false,
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

    /// The model this engine was loaded with, if set.
    pub fn model(&self) -> Option<SttModel> {
        self.model
    }

    /// Whether this backend decodes fast enough to drive the live-preview loop,
    /// which re-transcribes the growing in-progress phrase every ~0.7s. A CPU
    /// Whisper decode pays the fixed ~30s mel-encoder cost per call (seconds per
    /// phrase), so previewing would run it 6-7x per phrase and starve the final
    /// decode — disable preview there. Parakeet (and a GPU-accelerated Whisper
    /// build) are fast enough to preview.
    pub fn supports_realtime_preview(&self) -> bool {
        match &self.inner {
            #[cfg(feature = "parakeet")]
            EngineInner::Parakeet { .. } => true,
            #[cfg(feature = "stt")]
            EngineInner::Whisper { .. } => cfg!(feature = "cuda"),
            #[allow(unreachable_patterns)]
            _ => false,
        }
    }

    /// Set the running session transcript used as decoder context for the
    /// next call. Pass the trailing ~200 chars of prior output. Pass `None`
    /// to clear (e.g., on a fresh session).
    pub fn set_initial_prompt(&mut self, prompt: Option<String>) {
        self.initial_prompt = prompt;
    }

    /// Set the user glossary. Words are joined into a comma-separated clause
    /// that biases whisper's decoder toward their spelling. Pass an empty
    /// slice to clear.
    pub fn set_vocabulary(&mut self, words: &[String]) {
        let cleaned: Vec<&str> = words
            .iter()
            .map(|w| w.trim())
            .filter(|w| !w.is_empty())
            .collect();
        self.vocabulary = if cleaned.is_empty() {
            None
        } else {
            Some(cleaned.join(", "))
        };
    }

    /// Set the spoken-language hint. `None` or "auto" auto-detects; a code like
    /// "es" forces a language. Only honored by multilingual models.
    pub fn set_language(&mut self, language: Option<String>) {
        self.language = language.filter(|l| !l.trim().is_empty());
    }

    /// Translate recognized speech to English. Only honored by multilingual
    /// models; English-only models always transcribe verbatim.
    pub fn set_translate(&mut self, translate: bool) {
        self.translate = translate;
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
                self.vocabulary.as_deref(),
                self.language.as_deref(),
                self.translate,
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

    /// Trim a comma-joined glossary to a char budget at a comma boundary, so the
    /// vocabulary clause cannot exceed the prompt's token budget no matter how
    /// many words the caller supplies.
    #[cfg(feature = "stt")]
    fn cap_glossary(glossary: &str, max_chars: usize) -> &str {
        if glossary.chars().count() <= max_chars {
            return glossary;
        }
        let cut = glossary
            .char_indices()
            .nth(max_chars)
            .map(|(i, _)| i)
            .unwrap_or(glossary.len());
        let head = &glossary[..cut];
        match head.rfind(',') {
            Some(comma) => &head[..comma],
            None => head,
        }
    }

    #[cfg(feature = "stt")]
    #[allow(clippy::too_many_arguments)]
    fn transcribe_whisper(
        ctx: &WhisperContext,
        n_threads: i32,
        samples: &[f32],
        start: Instant,
        model: Option<SttModel>,
        initial_prompt: Option<&str>,
        vocabulary: Option<&str>,
        language: Option<&str>,
        translate: bool,
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
        // Whisper always pads a clip's mel to a fixed 30s (1500 encoder frames),
        // so a 2-5s dictation phrase otherwise pays the full-30s encoder cost.
        // Size the audio context to the actual clip length (50 ctx frames/sec)
        // plus ~2.5s of headroom, clamped, so short phrases decode ~3x faster
        // with no accuracy loss. Clips near/over 30s keep the full context.
        let clip_secs = samples.len() as f32 / 16_000.0;
        if clip_secs < 28.0 {
            let audio_ctx = ((clip_secs * 50.0).ceil() as i32 + 128).clamp(256, 1500);
            params.set_audio_ctx(audio_ctx);
        }
        // Language/translation only apply to multilingual checkpoints; the
        // `.en` models can't do either, so force English and no translation.
        let multilingual = model.map(|m| m.is_multilingual()).unwrap_or(false);
        let (effective_lang, effective_translate) = if multilingual {
            (language.unwrap_or("auto"), translate)
        } else {
            ("en", false)
        };
        params.set_language(Some(effective_lang));
        params.set_translate(effective_translate);
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
        // Base style hint, optionally extended with a user glossary so names
        // and jargon spell correctly. Both are prepended to the rolling
        // session context (whisper.cpp's streaming pattern).
        // The English style hint only helps when the output is English; on a
        // non-English transcription it can nudge whisper to code-switch.
        let output_is_english = effective_translate || effective_lang == "en";
        let mut prompt = String::new();
        if output_is_english {
            prompt.push_str("Use proper punctuation and capitalization.");
        }
        if let Some(glossary) = vocabulary.filter(|g| !g.trim().is_empty()) {
            if !prompt.is_empty() {
                prompt.push(' ');
            }
            prompt.push_str("Vocabulary: ");
            prompt.push_str(Self::cap_glossary(glossary.trim(), MAX_VOCAB_PROMPT_CHARS));
            prompt.push('.');
        }
        // The rolling session context is the prior English transcript, so only
        // feed it back when the output is English. On a non-English decode it
        // would push whisper to code-switch into English.
        if output_is_english && let Some(prev) = initial_prompt.filter(|s| !s.trim().is_empty()) {
            // Cap at ~200 chars to keep the prompt token budget bounded.
            let trimmed = prev.trim();
            let start_byte = trimmed
                .char_indices()
                .rev()
                .nth(200)
                .map(|(i, _)| i)
                .unwrap_or(0);
            if !prompt.is_empty() {
                prompt.push(' ');
            }
            prompt.push_str(&trimmed[start_byte..]);
        }
        if !prompt.is_empty() {
            params.set_initial_prompt(&prompt);
        }
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
            "Whisper transcribed {} samples in {}ms -> {} segment(s), {} chars",
            samples.len(),
            elapsed.as_millis(),
            segments.len(),
            full_text.trim().chars().count()
        );
        tracing::trace!("Whisper text: {:?}", full_text.trim());

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
            "Parakeet transcribed {} samples in {}ms -> {} chars",
            samples.len(),
            elapsed.as_millis(),
            result.text.trim().chars().count()
        );
        tracing::trace!("Parakeet text: {:?}", result.text.trim());

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

#[cfg(all(test, feature = "stt"))]
mod tests {
    use super::*;

    #[test]
    fn cap_glossary_passes_through_within_budget() {
        let g = "FooBar, baz, qux";
        assert_eq!(SttEngine::cap_glossary(g, 100), g);
    }

    #[test]
    fn cap_glossary_trims_at_comma_boundary() {
        // The 20-char prefix lands inside "charlie"; trimming backs up to the
        // last comma so no entry is split.
        let g = "alpha, bravo, charlie, delta, echo";
        assert_eq!(SttEngine::cap_glossary(g, 20), "alpha, bravo");
    }
}
