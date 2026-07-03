//! Local LLM inference engine over llama.cpp (via `llama-cpp-2`). CPU only:
//! no GPU offload until a murmur feature exposes it deliberately.

use llama_cpp_2::context::LlamaContext;
use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::{BatchAddError, LlamaBatch};
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{AddBos, LlamaModel};
use llama_cpp_2::sampling::LlamaSampler;
use llama_cpp_2::token::LlamaToken;
use llama_cpp_2::{
    DecodeError, GrammarError, LlamaContextLoadError, LlamaModelLoadError, LogOptions,
    StringToTokenError, TokenToStringError, send_logs_to_tracing,
};
use std::num::NonZeroU32;
use std::path::Path;
use std::sync::OnceLock;

/// Context window for generation. Qwen3 supports far more, but 4k keeps the
/// KV cache small for short cleanup and rewrite prompts.
const N_CTX: u32 = 4096;

/// Default system prompt: answers only, no chatter, suits cleanup/rewrite.
const DEFAULT_SYSTEM_PROMPT: &str =
    "You are a precise text assistant. Reply with only the requested text, no explanations.";

/// Errors from the local LLM runtime.
#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("llama backend initialization failed: {0}")]
    Backend(String),
    #[error("failed to load GGUF model: {0}")]
    ModelLoad(#[from] LlamaModelLoadError),
    #[error("failed to create llama context: {0}")]
    Context(#[from] LlamaContextLoadError),
    #[error("tokenization failed: {0}")]
    Tokenize(#[from] StringToTokenError),
    #[error("token decoding failed: {0}")]
    Detokenize(#[from] TokenToStringError),
    #[error("failed to queue tokens for decode: {0}")]
    Batch(#[from] BatchAddError),
    #[error("decode failed: {0}")]
    Decode(#[from] DecodeError),
    #[error("GBNF grammar failed to build: {0}")]
    Grammar(#[from] GrammarError),
    #[error(
        "prompt of {prompt_tokens} tokens plus {max_tokens} output tokens \
         exceeds the {n_ctx}-token context"
    )]
    PromptTooLong {
        prompt_tokens: usize,
        max_tokens: usize,
        n_ctx: usize,
    },
}

/// A loaded GGUF model ready for bounded text generation.
pub struct LlmEngine {
    model: LlamaModel,
}

impl LlmEngine {
    /// Load a GGUF model from `path`.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, LlmError> {
        let backend = backend()?;
        let path = path.as_ref();
        tracing::info!(?path, "loading LLM model");
        let model = LlamaModel::load_from_file(backend, path, &LlamaModelParams::default())?;
        tracing::info!("LLM model loaded");
        Ok(Self { model })
    }

    /// Generate a completion for `prompt` under the default system prompt,
    /// bounded to `max_tokens` output tokens. Returns the assistant text.
    pub fn generate(&self, prompt: &str, max_tokens: usize) -> Result<String, LlmError> {
        self.generate_with_system(DEFAULT_SYSTEM_PROMPT, prompt, max_tokens)
    }

    /// Generate with an explicit system prompt (AI cleanup and the agent
    /// router carry their own instructions).
    pub fn generate_with_system(
        &self,
        system: &str,
        user: &str,
        max_tokens: usize,
    ) -> Result<String, LlmError> {
        // Greedy keeps cleanup and rewrite output deterministic.
        self.generate_sampled(system, user, max_tokens, LlamaSampler::greedy())
    }

    /// Generate with output constrained to the GBNF grammar `gbnf`, rooted at
    /// its `root` rule, so every emitted token keeps the output inside the
    /// grammar. Used by Tier 3 tool selection: the constraint guarantees
    /// syntax, not the right tool, so the allowlist and confirmation gates
    /// remain the semantic backstop.
    pub fn generate_constrained(
        &self,
        system: &str,
        user: &str,
        gbnf: &str,
        max_tokens: usize,
    ) -> Result<String, LlmError> {
        let grammar = LlamaSampler::grammar(&self.model, gbnf, "root")?;
        // Grammar first masks disallowed tokens; greedy then picks the best
        // token still permitted.
        let sampler = LlamaSampler::chain_simple([grammar, LlamaSampler::greedy()]);
        self.generate_sampled(system, user, max_tokens, sampler)
    }

    /// Shared generation path: format the chat prompt, decode it, then run
    /// the token loop with the given sampler.
    fn generate_sampled(
        &self,
        system: &str,
        user: &str,
        max_tokens: usize,
        sampler: LlamaSampler,
    ) -> Result<String, LlmError> {
        if max_tokens == 0 {
            return Ok(String::new());
        }
        let backend = backend()?;
        let formatted = format_qwen3_chat(system, user);
        // The template carries its own ChatML framing (str_to_token parses the
        // special tokens), so no BOS.
        let tokens = self.model.str_to_token(&formatted, AddBos::Never)?;

        let mut ctx = self.model.new_context(backend, context_params())?;
        let n_ctx = ctx.n_ctx() as usize;
        if tokens.len() + max_tokens > n_ctx || tokens.len() > ctx.n_batch() as usize {
            return Err(LlmError::PromptTooLong {
                prompt_tokens: tokens.len(),
                max_tokens,
                n_ctx,
            });
        }
        tracing::debug!(
            prompt_tokens = tokens.len(),
            max_tokens,
            "starting generation"
        );
        tracing::trace!(%formatted, "llm prompt");

        let mut batch = LlamaBatch::new(tokens.len(), 1);
        let last = tokens.len() - 1;
        for (i, token) in tokens.iter().enumerate() {
            // Logits are only needed for the last prompt token.
            batch.add(*token, i as i32, &[0], i == last)?;
        }
        ctx.decode(&mut batch)?;

        let out_bytes = self.decode_loop(&mut ctx, &mut batch, sampler, max_tokens)?;
        let text = String::from_utf8_lossy(&out_bytes);
        let text = strip_think_block(&text).trim().to_string();
        tracing::debug!(chars = text.len(), "generation done");
        tracing::trace!(%text, "llm output");
        Ok(text)
    }

    /// Token-by-token decode with `sampler` until EOG or the token budget
    /// runs out. Expects `batch` to still hold the just-decoded prompt.
    fn decode_loop(
        &self,
        ctx: &mut LlamaContext<'_>,
        batch: &mut LlamaBatch,
        mut sampler: LlamaSampler,
        max_tokens: usize,
    ) -> Result<Vec<u8>, LlmError> {
        let mut out_bytes: Vec<u8> = Vec::with_capacity(max_tokens * 4);
        let mut n_cur = batch.n_tokens();
        let mut produced = 0usize;

        while produced < max_tokens {
            // sample() also accepts the token into the sampler chain; a
            // second accept here would advance a grammar constraint twice.
            let token = sampler.sample(ctx, batch.n_tokens() - 1);
            if self.model.is_eog_token(token) {
                break;
            }
            out_bytes.extend_from_slice(&self.token_bytes(token)?);

            batch.clear();
            batch.add(token, n_cur, &[0], true)?;
            ctx.decode(batch)?;
            n_cur += 1;
            produced += 1;
        }
        tracing::debug!(generated_tokens = produced, "decode loop finished");
        Ok(out_bytes)
    }

    /// Raw bytes for one token, retrying with the size llama.cpp asks for
    /// when the initial buffer is too small. Special tokens are rendered so
    /// a stray `<think>` block stays visible for stripping.
    fn token_bytes(&self, token: LlamaToken) -> Result<Vec<u8>, LlmError> {
        match self.model.token_to_piece_bytes(token, 32, true, None) {
            Err(TokenToStringError::InsufficientBufferSpace(needed)) => Ok(self
                .model
                .token_to_piece_bytes(token, needed.unsigned_abs() as usize, true, None)?),
            result => Ok(result?),
        }
    }
}

/// Process-wide llama.cpp backend. llama.cpp may only initialize once per
/// process, so cache the result (or the failure), mirroring the ORT init.
fn backend() -> Result<&'static LlamaBackend, LlmError> {
    static BACKEND: OnceLock<Result<LlamaBackend, String>> = OnceLock::new();
    let result = BACKEND.get_or_init(|| {
        send_logs_to_tracing(LogOptions::default());
        LlamaBackend::init().map_err(|e| e.to_string())
    });
    result.as_ref().map_err(|e| LlmError::Backend(e.clone()))
}

fn context_params() -> LlamaContextParams {
    let threads = std::thread::available_parallelism()
        .map(|n| n.get() as i32)
        .unwrap_or(4);
    LlamaContextParams::default()
        .with_n_ctx(NonZeroU32::new(N_CTX))
        .with_n_threads(threads)
        .with_n_threads_batch(threads)
}

/// Qwen3 ChatML with the assistant turn pre-opened by an empty `<think>`
/// block: exactly what the official template renders with
/// `enable_thinking=false`, so the model answers directly instead of spending
/// the token budget on reasoning.
fn format_qwen3_chat(system: &str, user: &str) -> String {
    format!(
        "<|im_start|>system\n{system}<|im_end|>\n\
         <|im_start|>user\n{user}<|im_end|>\n\
         <|im_start|>assistant\n<think>\n\n</think>\n\n"
    )
}

/// Drop a leading `<think>...</think>` block if the model reasons anyway.
fn strip_think_block(text: &str) -> &str {
    let trimmed = text.trim_start();
    if let Some(rest) = trimmed.strip_prefix("<think>")
        && let Some((_, after)) = rest.split_once("</think>")
    {
        return after;
    }
    trimmed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_template_has_chatml_framing_and_empty_think_block() {
        let formatted = format_qwen3_chat("sys instructions", "user text");
        assert!(formatted.starts_with("<|im_start|>system\nsys instructions<|im_end|>\n"));
        assert!(formatted.contains("<|im_start|>user\nuser text<|im_end|>\n"));
        assert!(formatted.ends_with("<|im_start|>assistant\n<think>\n\n</think>\n\n"));
    }

    #[test]
    fn strip_think_block_removes_leading_reasoning() {
        assert_eq!(
            strip_think_block("<think>\nsome reasoning\n</think>\n\nanswer"),
            "\n\nanswer"
        );
        assert_eq!(strip_think_block("plain answer"), "plain answer");
        // Unclosed block is left alone rather than swallowing the output.
        assert_eq!(strip_think_block("<think>oops"), "<think>oops");
    }
}
