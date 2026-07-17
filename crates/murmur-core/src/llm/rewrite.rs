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

/// A resolved rewrite instruction: a built-in [`RewriteMode`] or a per-app
/// custom prompt from the user's app profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RewriteInstruction {
    Mode(RewriteMode),
    Custom(String),
}

impl RewriteInstruction {
    /// The imperative instruction text the model receives.
    pub fn text(&self) -> &str {
        match self {
            Self::Mode(mode) => instruction(*mode),
            Self::Custom(text) => text,
        }
    }
}

/// Caps for the context fragment, so it stays a small, bounded slice of the
/// prompt budget no matter what is on the clipboard.
const MAX_CONTEXT_APP_CHARS: usize = 80;
const MAX_CONTEXT_CLIPBOARD_CHARS: usize = 500;

/// Format the opt-in local context fragment appended to a rewrite prompt:
/// the app the text is being written in and the user's current clipboard.
///
/// Capture belongs to the app layer, which owns the OS hooks; this function
/// only formats what it is handed. The result is strictly on-device prompt
/// material: it must never be logged above trace or leave the machine.
/// Blank or missing inputs contribute nothing; when both are blank the
/// result is empty, meaning "no extra context".
pub fn assemble_context(app: Option<&str>, clipboard: Option<&str>) -> String {
    let mut parts = Vec::new();
    if let Some(app) = trimmed_non_empty(app) {
        parts.push(format!(
            "The text is being written in {}.",
            cap_chars(app, MAX_CONTEXT_APP_CHARS)
        ));
    }
    if let Some(clip) = trimmed_non_empty(clipboard) {
        parts.push(format!(
            "Possibly relevant clipboard context (do not repeat it verbatim \
             unless asked): \"\"\"{}\"\"\"",
            cap_chars(clip, MAX_CONTEXT_CLIPBOARD_CHARS)
        ));
    }
    parts.join("\n")
}

fn trimmed_non_empty(text: Option<&str>) -> Option<&str> {
    text.map(str::trim).filter(|t| !t.is_empty())
}

/// Truncate to at most `max` characters (never mid code point), marking the
/// cut with an ellipsis.
fn cap_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut capped: String = text.chars().take(max).collect();
    capped.push('…');
    capped
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
    rewrite_instructed(
        engine,
        text,
        &RewriteInstruction::Mode(mode),
        "",
        max_tokens,
    )
}

/// Like [`rewrite`], but with a resolved instruction (built-in mode or per-app
/// custom prompt) and an optional [`assemble_context`] fragment appended to
/// the system prompt. An empty `context` adds nothing.
#[cfg(feature = "llm")]
pub fn rewrite_instructed(
    engine: &LlmEngine,
    text: &str,
    instruction: &RewriteInstruction,
    context: &str,
    max_tokens: usize,
) -> Result<String, LlmError> {
    rewrite_with(
        |system, user| engine.generate_with_system(system, user, max_tokens),
        text,
        instruction,
        context,
    )
}

/// Core of [`rewrite_instructed`] with generation injected, so the guard,
/// prompt assembly, and output cleanup are unit-testable without a model.
#[cfg(any(test, feature = "llm"))]
fn rewrite_with<F, E>(
    generate: F,
    text: &str,
    instruction: &RewriteInstruction,
    context: &str,
) -> Result<String, E>
where
    F: FnOnce(&str, &str) -> Result<String, E>,
{
    if text.trim().is_empty() {
        return Ok(text.to_string());
    }
    // Shape only: the text, a custom instruction, and the context fragment are
    // all user content and must stay out of the log.
    tracing::debug!(
        chars = text.len(),
        custom_instruction = matches!(instruction, RewriteInstruction::Custom(_)),
        has_context = !context.is_empty(),
        "rewriting text"
    );
    let mut system = format!(
        "{} Reply with only the resulting text, no explanations.",
        instruction.text()
    );
    // Context goes after the instruction, clearly separated, so it biases tone
    // and vocabulary without displacing what the model is asked to do.
    if !context.is_empty() {
        system.push_str("\n\n");
        system.push_str(context);
    }
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
    fn instruction_resolves_mode_to_its_builtin_text() {
        for mode in ALL_MODES {
            assert_eq!(RewriteInstruction::Mode(mode).text(), instruction(mode));
        }
    }

    #[test]
    fn instruction_resolves_custom_to_its_own_text() {
        let custom = RewriteInstruction::Custom("Rewrite as a haiku.".to_string());
        assert_eq!(custom.text(), "Rewrite as a haiku.");
    }

    #[test]
    fn assemble_context_is_empty_without_inputs() {
        assert!(assemble_context(None, None).is_empty());
        assert!(assemble_context(Some("  "), Some("\n\t")).is_empty());
    }

    #[test]
    fn assemble_context_includes_app_and_clipboard() {
        let both = assemble_context(Some("Code.exe"), Some("fn main() {}"));
        assert!(both.contains("The text is being written in Code.exe."));
        assert!(both.contains("clipboard context"));
        assert!(both.contains("\"\"\"fn main() {}\"\"\""));

        let app_only = assemble_context(Some("slack.exe"), None);
        assert!(app_only.contains("slack.exe"));
        assert!(!app_only.contains("clipboard"));

        let clip_only = assemble_context(None, Some("meeting notes"));
        assert!(clip_only.contains("\"\"\"meeting notes\"\"\""));
        assert!(!clip_only.contains("written in"));
    }

    #[test]
    fn assemble_context_caps_clipboard_on_a_char_boundary() {
        // Multi-byte chars: a byte-index truncation would split a code point.
        let long: String = "é".repeat(MAX_CONTEXT_CLIPBOARD_CHARS + 50);
        let fragment = assemble_context(None, Some(&long));
        let capped = format!("{}…", "é".repeat(MAX_CONTEXT_CLIPBOARD_CHARS));
        assert!(fragment.contains(&capped));
        assert!(!fragment.contains(&"é".repeat(MAX_CONTEXT_CLIPBOARD_CHARS + 1)));
    }

    #[test]
    fn assemble_context_caps_app_name() {
        let long_app = "a".repeat(MAX_CONTEXT_APP_CHARS + 20);
        let fragment = assemble_context(Some(&long_app), None);
        assert!(fragment.contains(&format!("{}…", "a".repeat(MAX_CONTEXT_APP_CHARS))));
    }

    #[test]
    fn empty_input_skips_the_model_and_returns_unchanged() {
        let input = "  \n\t ";
        let result = rewrite_with(
            |_: &str, _: &str| -> Result<String, ()> {
                panic!("model must not be called for empty input")
            },
            input,
            &RewriteInstruction::Mode(RewriteMode::CleanUp),
            "",
        )
        .unwrap();
        assert_eq!(result, input);
    }

    #[test]
    fn rewrite_cleans_model_output() {
        let result = rewrite_with(
            |_, _| Ok::<_, ()>("Sure, here is the rewritten text:\n\"Hello world.\"".to_string()),
            "hello world",
            &RewriteInstruction::Mode(RewriteMode::CleanUp),
            "",
        )
        .unwrap();
        assert_eq!(result, "Hello world.");
    }

    #[test]
    fn system_prompt_carries_custom_instruction_and_context() {
        let mut seen_system = String::new();
        let instruction = RewriteInstruction::Custom("Rewrite as a commit message.".to_string());
        let context = assemble_context(Some("Code.exe"), Some("fix: focus bug"));
        rewrite_with(
            |system, _| {
                seen_system = system.to_string();
                Ok::<_, ()>("done".to_string())
            },
            "some text",
            &instruction,
            &context,
        )
        .unwrap();
        assert!(seen_system.starts_with("Rewrite as a commit message."));
        // Instruction first, context after, separated by a blank line.
        let context_at = seen_system.find(&context).expect("context in system");
        assert!(context_at > 0);
        assert!(seen_system[..context_at].ends_with("\n\n"));
    }

    #[test]
    fn system_prompt_omits_separator_without_context() {
        let mut seen_system = String::new();
        rewrite_with(
            |system, _| {
                seen_system = system.to_string();
                Ok::<_, ()>("done".to_string())
            },
            "some text",
            &RewriteInstruction::Mode(RewriteMode::Formal),
            "",
        )
        .unwrap();
        assert!(seen_system.contains(instruction(RewriteMode::Formal)));
        assert!(!seen_system.contains("\n\n"));
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
