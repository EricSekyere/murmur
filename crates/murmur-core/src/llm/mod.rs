//! On-device LLM runtime: the shared foundation for AI cleanup (roadmap
//! feature 1) and Tier 3 of the agent router. Wraps llama.cpp via
//! `llama-cpp-2` as a peer of the whisper STT path: same native cmake build
//! pattern, catalog-pinned model, checksum-verified download, CPU-only.
//!
//! The model runtime (engine, catalog, model-backed [`rewrite`]) sits behind
//! the `llm` feature. [`RewriteMode`] and [`instruction`] are plain data and
//! stay unconditional so config can reference them without llama.cpp.

#[cfg(feature = "llm")]
mod catalog;
#[cfg(feature = "llm")]
mod engine;
mod rewrite;

#[cfg(feature = "llm")]
pub use catalog::{
    QWEN3_1_7B_FILENAME, QWEN3_1_7B_SHA256, QWEN3_1_7B_SIZE_BYTES, QWEN3_1_7B_URL, download,
    download_with_progress, is_downloaded, llm_dir, model_path,
};
#[cfg(feature = "llm")]
pub use engine::{LlmEngine, LlmError};
#[cfg(feature = "llm")]
pub use rewrite::rewrite;
pub use rewrite::{RewriteMode, assemble_context, instruction};
