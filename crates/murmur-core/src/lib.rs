//! Core library for Murmur, a local-first voice-to-text tool.
//!
//! Everything shared by the desktop app, CLI, and MCP server lives here:
//! audio capture and VAD ([`audio`]), speech-to-text engines and model
//! management ([`stt`]), meeting recording and diarization ([`meeting`]),
//! local LLM rewrites ([`llm`]), text delivery ([`output`]), voice commands
//! ([`voice_commands`], [`command`]), transcription history ([`history`]),
//! and configuration ([`config`]).
//!
//! Heavy native dependencies (whisper.cpp, ONNX Runtime, llama.cpp,
//! tree-sitter) are behind cargo features so a default build stays light;
//! see the feature table in the crate manifest.

pub mod audio;
pub mod cloud;
pub mod command;
pub mod commit;
pub mod config;
pub mod dictation;
pub mod dictation_junction;
pub mod dictation_request;
pub mod dictation_submit;
pub mod download;
pub mod emoji;
pub mod filler;
pub mod fsutil;
#[cfg(feature = "help")]
pub mod help;
pub mod history;
pub mod hotkey;
#[cfg(feature = "indexer")]
pub mod indexer;
pub mod insights;
pub mod integrity;
pub mod llm;
pub mod meeting;
pub mod output;
pub mod stt;
pub mod vocab_correct;
pub mod voice_commands;
