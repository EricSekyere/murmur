//! Semantic rewrite layer over the local LLM: tone, format, and summary
//! transforms for dictated text. Runs after `stt::postprocess`, which already
//! owns fillers, punctuation, and disfluencies; this module only does the
//! meaning-level work that needs a model.

use serde::{Deserialize, Serialize};

#[cfg(feature = "llm")]
use super::{LlmEngine, LlmError};

/// Semantic transform applied to dictated text by the local LLM.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RewriteMode {
    /// Fix grammar and wording while preserving meaning.
    #[default]
    CleanUp,
    /// Shift to a formal, professional tone.
    Formal,
    /// Shift to a relaxed, conversational tone.
    Casual,
    /// Reformat as a concise bullet list.
    BulletList,
    /// Condense to a one- or two-sentence summary.
    Summarize,
}

/// The imperative instruction the model receives for `mode`.
pub fn instruction(mode: RewriteMode) -> &'static str {
    match mode {
        RewriteMode::CleanUp => {
            "Fix grammar, spelling, and capitalization. Keep the original meaning \
             and wording as close as possible. Do not add or remove content."
        }
        RewriteMode::Formal => {
            "Rewrite the text in a formal, professional tone. Keep the meaning unchanged."
        }
        RewriteMode::Casual => {
            "Rewrite the text in a relaxed, conversational tone. Keep the meaning unchanged."
        }
        RewriteMode::BulletList => {
            "Turn the text into a concise bullet list. \
             One point per line, each line starting with \"- \"."
        }
        RewriteMode::Summarize => "Summarize the text in one or two sentences.",
    }
}

/// Placeholder for roadmap feature 7's on-device context injection.
///
/// App integration will capture the active-app name, the current selection,
/// and recent clipboard text (all locally, never uploaded) and this function
/// will format them into a prompt fragment that biases the rewrite's tone and
/// vocabulary. Capture belongs to the app layer, which owns the OS hooks;
/// until it lands this returns an empty string, meaning "no extra context".
pub fn assemble_context() -> String {
    String::new()
}

/// Rewrite `text` with the local LLM according to `mode`, bounded to
/// `max_tokens` output tokens. Empty or whitespace-only input is returned
/// unchanged without touching the model.
#[cfg(feature = "llm")]
pub fn rewrite(
    engine: &LlmEngine,
    text: &str,
    mode: RewriteMode,
    max_tokens: usize,
) -> Result<String, LlmError> {
    rewrite_with(
        |system, user| engine.generate_with_system(system, user, max_tokens),
        text,
        mode,
    )
}

/// Core of [`rewrite`] with generation injected, so the guard and output
/// cleanup are unit-testable without a loaded model.
#[cfg(feature = "llm")]
fn rewrite_with<F>(generate: F, text: &str, mode: RewriteMode) -> Result<String, LlmError>
where
    F: FnOnce(&str, &str) -> Result<String, LlmError>,
{
    if text.trim().is_empty() {
        return Ok(text.to_string());
    }
    tracing::debug!(?mode, chars = text.len(), "rewriting text");
    let system = format!(
        "{} Reply with only the resulting text, no explanations.",
        instruction(mode)
    );
    let raw = generate(&system, text)?;
    Ok(clean_output(&raw))
}

/// Normalize model output: drop a conversational lead-in the model may emit
/// despite the system prompt, then trim wrapping quotes and whitespace.
#[cfg(any(test, feature = "llm"))]
fn clean_output(raw: &str) -> String {
    let text = strip_preamble(raw.trim());
    strip_wrapping_quotes(text).trim().to_string()
}

/// Openers a lead-in starts with ("Sure, here is the rewritten text:").
#[cfg(any(test, feature = "llm"))]
const PREAMBLE_OPENERS: &[&str] = &[
    "sure",
    "certainly",
    "of course",
    "okay",
    "here is",
    "here's",
    "here you go",
];

/// Words a lead-in uses to name its payload ("the rewritten text:").
#[cfg(any(test, feature = "llm"))]
const PAYLOAD_WORDS: &[&str] = &["text", "version", "list", "summary", "rewrite", "rewritten"];

/// Strip a leading "Sure, here is ...:" style preamble, keeping the payload.
#[cfg(any(test, feature = "llm"))]
fn strip_preamble(text: &str) -> &str {
    let Some((head, rest)) = text.split_once(':') else {
        return text;
    };
    let content = rest.trim_start();
    let head_lower = head.to_lowercase();
    if content.is_empty()
        || head.len() > 80
        || head.contains('\n')
        || !PREAMBLE_OPENERS.iter().any(|o| head_lower.starts_with(o))
    {
        return text;
    }
    // A lead-in with the payload on the same line must name it ("here is the
    // rewritten text: ..."), so output that merely opens casually ("Okay,
    // here's the deal: ...") is left alone.
    if rest.starts_with('\n') || PAYLOAD_WORDS.iter().any(|w| head_lower.contains(w)) {
        content
    } else {
        text
    }
}

/// Remove one pair of quotes wrapping the entire text, if present.
#[cfg(any(test, feature = "llm"))]
fn strip_wrapping_quotes(text: &str) -> &str {
    const PAIRS: &[(char, char)] = &[
        ('"', '"'),
        ('\'', '\''),
        ('\u{201C}', '\u{201D}'),
        ('\u{2018}', '\u{2019}'),
    ];
    let trimmed = text.trim();
    for (open, close) in PAIRS {
        if let Some(inner) = trimmed
            .strip_prefix(*open)
            .and_then(|s| s.strip_suffix(*close))
            && !inner.is_empty()
        {
            return inner;
        }
    }
    trimmed
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_MODES: [RewriteMode; 5] = [
        RewriteMode::CleanUp,
        RewriteMode::Formal,
        RewriteMode::Casual,
        RewriteMode::BulletList,
        RewriteMode::Summarize,
    ];

    #[test]
    fn instructions_are_non_empty_and_distinct() {
        let mut seen = std::collections::HashSet::new();
        for mode in ALL_MODES {
            let inst = instruction(mode);
            assert!(!inst.trim().is_empty(), "empty instruction for {mode:?}");
            assert!(seen.insert(inst), "duplicate instruction for {mode:?}");
        }
    }

    #[test]
    fn mode_serializes_as_snake_case() {
        let json = serde_json::to_string(&RewriteMode::BulletList).unwrap();
        assert_eq!(json, "\"bullet_list\"");
        let mode: RewriteMode = serde_json::from_str("\"clean_up\"").unwrap();
        assert_eq!(mode, RewriteMode::CleanUp);
    }

    #[test]
    fn assemble_context_is_empty_until_app_integration_lands() {
        assert!(assemble_context().is_empty());
    }

    #[cfg(feature = "llm")]
    #[test]
    fn empty_input_skips_the_model_and_returns_unchanged() {
        let input = "  \n\t ";
        let result = rewrite_with(
            |_, _| panic!("model must not be called for empty input"),
            input,
            RewriteMode::CleanUp,
        )
        .unwrap();
        assert_eq!(result, input);
    }

    #[cfg(feature = "llm")]
    #[test]
    fn rewrite_cleans_model_output() {
        let result = rewrite_with(
            |_, _| Ok("Sure, here is the rewritten text:\n\"Hello world.\"".to_string()),
            "hello world",
            RewriteMode::CleanUp,
        )
        .unwrap();
        assert_eq!(result, "Hello world.");
    }

    #[test]
    fn clean_output_strips_preamble_and_wrapping_quotes() {
        assert_eq!(
            clean_output("Sure, here is the rewritten text:\n\"Hello world.\""),
            "Hello world."
        );
    }

    #[test]
    fn strip_preamble_removes_lead_in_on_its_own_line() {
        assert_eq!(
            strip_preamble("Sure, here is the rewritten text:\nShip it Friday."),
            "Ship it Friday."
        );
        assert_eq!(
            strip_preamble("Certainly! Here is the summary:\n\nOne line."),
            "One line."
        );
    }

    #[test]
    fn strip_preamble_handles_same_line_payload() {
        assert_eq!(
            strip_preamble("Here's the formal version: Dear team, we ship Friday."),
            "Dear team, we ship Friday."
        );
    }

    #[test]
    fn strip_preamble_leaves_legitimate_text_alone() {
        // Casual opener that is itself the rewrite, not a lead-in.
        let casual = "Okay, here's the deal: we ship Friday.";
        assert_eq!(strip_preamble(casual), casual);
        let plain = "We should ship on Friday.";
        assert_eq!(strip_preamble(plain), plain);
        // Colon later in the body, not a leading preamble.
        let body = "Agenda\nItems: one, two.";
        assert_eq!(strip_preamble(body), body);
    }

    #[test]
    fn strip_wrapping_quotes_removes_one_matching_pair() {
        assert_eq!(strip_wrapping_quotes("\"quoted\""), "quoted");
        assert_eq!(strip_wrapping_quotes("'quoted'"), "quoted");
        assert_eq!(strip_wrapping_quotes("\u{201C}curly\u{201D}"), "curly");
        assert_eq!(strip_wrapping_quotes("no quotes"), "no quotes");
        // Mismatched or lone quotes are untouched.
        assert_eq!(strip_wrapping_quotes("\"open only"), "\"open only");
        assert_eq!(strip_wrapping_quotes("\""), "\"");
    }
}
