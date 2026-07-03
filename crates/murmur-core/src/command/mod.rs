//! Voice-to-action command mode: the safety spine.
//!
//! A [`Tool`] describes an invokable action with an intrinsic [`RiskTier`];
//! the [`PermissionStore`] holds the user's per-tool policy. The pure
//! [`decide`] function maps policy plus risk to an execution [`Decision`],
//! so the safety table is testable in isolation.
//!
//! The Tier 1 router lives in [`Grammar`]: deterministic template patterns
//! that map a whole spoken phrase to a command id plus typed slots. On a
//! Tier 1 miss, [`route`] tries the optional Tier 2 [`IntentClassifier`]
//! (embedding paraphrase matching; the Help feature's ONNX embedder plugs
//! in), then falls back to Tier 3: grammar-constrained (GBNF) LLM tool
//! selection over the allowlisted tools (feature `llm`).
//!
//! Spoken identifier formatters ([`parse_case_command`] + [`format_identifier`])
//! turn phrases like "snake hello world" into `hello_world`.

mod error;
mod formatters;
mod grammar;
mod intent;
mod permissions;
mod router;
mod tool;

pub use error::CommandError;
pub use formatters::{CaseStyle, format_identifier, parse_case_command};
pub use grammar::{CommandPattern, CompiledPattern, Grammar, GrammarError, Match, SlotValue};
pub use intent::{IntentClassifier, IntentMatch, IntentSet, classify, cosine};
pub use permissions::{Decision, Permission, PermissionStore, decide};
#[cfg(feature = "llm")]
pub use router::route;
pub use router::{RouteOutcome, tool_call_grammar};
pub use tool::{RiskTier, Tool};
