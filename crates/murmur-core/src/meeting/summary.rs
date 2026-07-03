//! Meeting summary (roadmap feature 6, gated on `llm`).
//!
//! Meeting mode has no summary model of its own: it reuses feature 1's local
//! LLM through [`crate::llm::rewrite`] with [`RewriteMode::Summarize`]. Feed it
//! the speaker-labeled transcript from [`crate::meeting::assembly`].

use crate::llm::{LlmEngine, LlmError, RewriteMode, rewrite};

/// Summarize a speaker-labeled meeting transcript with the local LLM, bounded to
/// `max_tokens` output tokens. Empty input returns unchanged without touching
/// the model (see [`crate::llm::rewrite`]).
pub fn summarize_meeting(
    engine: &LlmEngine,
    labeled_transcript: &str,
    max_tokens: usize,
) -> Result<String, LlmError> {
    rewrite(
        engine,
        labeled_transcript,
        RewriteMode::Summarize,
        max_tokens,
    )
}
