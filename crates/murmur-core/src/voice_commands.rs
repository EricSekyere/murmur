//! Spoken editing commands and user text snippets recognized in a phrase.
//!
//! A command or snippet only fires when it is the *entire* phrase (after
//! normalization), so dictating "press enter to continue" types text rather
//! than executing a newline.

use serde::{Deserialize, Serialize};

/// A user-defined expansion: say `trigger`, get `expansion` typed instead.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snippet {
    pub trigger: String,
    pub expansion: String,
}

/// The interpretation of one transcribed phrase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VoiceCommand {
    /// Insert a single line break.
    NewLine,
    /// Insert a blank line (two line breaks).
    NewParagraph,
    /// Delete the previously delivered phrase.
    ScratchThat,
    /// Copy the selection (Ctrl/Cmd+C).
    Copy,
    /// Undo (Ctrl/Cmd+Z).
    Undo,
    /// Redo (Ctrl+Y / Cmd+Shift+Z).
    Redo,
    /// Press Tab.
    Tab,
    /// Press Escape.
    Escape,
    /// Ordinary dictation — deliver the text unchanged.
    Text,
}

/// Lowercase, trimmed, and stripped of trailing sentence punctuation, so
/// matching ignores case, surrounding whitespace, and a trailing "." or "?".
fn normalize(phrase: &str) -> String {
    phrase
        .trim()
        .trim_end_matches(['.', '!', '?', ','])
        .trim()
        .to_lowercase()
}

/// Map a normalized phrase to a built-in command, if it is one. Single source
/// of truth for both [`parse`] and collision detection in [`snippet_warnings`].
fn builtin_command(normalized: &str) -> Option<VoiceCommand> {
    Some(match normalized {
        "new line" | "newline" => VoiceCommand::NewLine,
        "new paragraph" => VoiceCommand::NewParagraph,
        "scratch that" | "delete that" => VoiceCommand::ScratchThat,
        "copy that" | "copy selection" => VoiceCommand::Copy,
        // Require the two-word form: a bare "undo"/"redo" is an easy
        // misrecognition that would destroy real edits via Ctrl+Z / Ctrl+Y.
        "undo that" => VoiceCommand::Undo,
        "redo that" => VoiceCommand::Redo,
        "press tab" | "tab key" => VoiceCommand::Tab,
        "press escape" | "escape key" => VoiceCommand::Escape,
        _ => return None,
    })
}

/// Classify a transcribed phrase as a built-in editing command, or `Text`.
pub fn parse(phrase: &str) -> VoiceCommand {
    builtin_command(&normalize(phrase)).unwrap_or(VoiceCommand::Text)
}

/// Spoken literal escape: "literally <command>" returns the remainder to type
/// verbatim, but only when it would otherwise act (a command or snippet), so
/// prose that merely begins with "literally" is untouched.
pub fn literal_escape(phrase: &str, snippets: &[Snippet]) -> Option<String> {
    let rest = strip_literal_prefix(phrase.trim_start())?.trim_start();
    let would_act =
        builtin_command(&normalize(rest)).is_some() || match_snippet(rest, snippets).is_some();
    would_act.then(|| rest.to_string())
}

/// Strip a leading "literally "/"literal " escape (case-insensitive), if present.
fn strip_literal_prefix(s: &str) -> Option<&str> {
    ["literally ", "literal "].iter().find_map(|prefix| {
        s.get(..prefix.len())
            .filter(|head| head.eq_ignore_ascii_case(prefix))
            .map(|_| &s[prefix.len()..])
    })
}

/// If the phrase exactly matches a snippet trigger, return its expansion.
/// Built-in commands take precedence, so callers should only consult this
/// after [`parse`] returns [`VoiceCommand::Text`].
pub fn match_snippet<'a>(phrase: &str, snippets: &'a [Snippet]) -> Option<&'a str> {
    let normalized = normalize(phrase);
    snippets
        .iter()
        .find(|s| normalize(&s.trigger) == normalized)
        .map(|s| s.expansion.as_str())
}

/// Warnings for snippets that will never fire: a trigger shadowed by a built-in
/// command, or a duplicate of an earlier trigger (only the first one wins).
pub fn snippet_warnings(snippets: &[Snippet]) -> Vec<String> {
    let mut warnings = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for s in snippets {
        let norm = normalize(&s.trigger);
        if norm.is_empty() {
            continue;
        }
        if builtin_command(&norm).is_some() {
            warnings.push(format!(
                "Snippet \"{}\" is shadowed by a built-in command and will never fire.",
                s.trigger.trim()
            ));
        }
        if seen.contains(&norm) {
            warnings.push(format!(
                "Duplicate snippet trigger \"{}\"; only the first one fires.",
                s.trigger.trim()
            ));
        } else {
            seen.push(norm);
        }
    }
    warnings
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_commands_case_and_punctuation_insensitive() {
        assert_eq!(parse("New line."), VoiceCommand::NewLine);
        assert_eq!(parse("  NEW PARAGRAPH  "), VoiceCommand::NewParagraph);
        assert_eq!(parse("scratch that!"), VoiceCommand::ScratchThat);
        assert_eq!(parse("delete that"), VoiceCommand::ScratchThat);
    }

    #[test]
    fn recognizes_editing_commands() {
        assert_eq!(parse("Copy that."), VoiceCommand::Copy);
        assert_eq!(parse("undo that"), VoiceCommand::Undo);
        assert_eq!(parse("redo that"), VoiceCommand::Redo);
        // Bare single words are too easy to misrecognize into a command.
        assert_eq!(parse("undo"), VoiceCommand::Text);
        assert_eq!(parse("redo"), VoiceCommand::Text);
        assert_eq!(parse("press tab"), VoiceCommand::Tab);
        assert_eq!(parse("press escape"), VoiceCommand::Escape);
    }

    #[test]
    fn destructive_commands_are_not_voice_triggered() {
        // Paste/cut/select-all can inject the clipboard or destroy a document
        // from a single misrecognition, so they are deliberately plain text.
        assert_eq!(parse("paste"), VoiceCommand::Text);
        assert_eq!(parse("paste that"), VoiceCommand::Text);
        assert_eq!(parse("cut that"), VoiceCommand::Text);
        assert_eq!(parse("select all"), VoiceCommand::Text);
    }

    #[test]
    fn commands_inside_a_sentence_are_plain_text() {
        assert_eq!(parse("press the new line button"), VoiceCommand::Text);
        assert_eq!(parse("scratch that itch"), VoiceCommand::Text);
        assert_eq!(parse("the quick brown fox"), VoiceCommand::Text);
        assert_eq!(parse("copy that file to the server"), VoiceCommand::Text);
    }

    #[test]
    fn literal_escape_types_command_words() {
        // "literally scratch that" should be delivered as the text "scratch that".
        assert_eq!(
            literal_escape("literally scratch that", &[]),
            Some("scratch that".to_string())
        );
        assert_eq!(
            literal_escape("Literal copy that", &[]),
            Some("copy that".to_string())
        );
    }

    #[test]
    fn literal_escape_leaves_ordinary_prose_alone() {
        // Begins with "literally" but the remainder is not a command/snippet.
        assert_eq!(literal_escape("literally everyone agrees", &[]), None);
        assert_eq!(literal_escape("the quick brown fox", &[]), None);
    }

    #[test]
    fn literal_escape_applies_to_snippets() {
        let snippets = vec![Snippet {
            trigger: "my email".to_string(),
            expansion: "user@example.com".to_string(),
        }];
        assert_eq!(
            literal_escape("literally my email", &snippets),
            Some("my email".to_string())
        );
    }

    #[test]
    fn snippet_warnings_flags_shadowed_and_duplicate() {
        let snippets = vec![
            Snippet {
                trigger: "scratch that".to_string(),
                expansion: "x".to_string(),
            },
            Snippet {
                trigger: "sig".to_string(),
                expansion: "a".to_string(),
            },
            Snippet {
                trigger: "Sig".to_string(),
                expansion: "b".to_string(),
            },
        ];
        let warnings = snippet_warnings(&snippets);
        assert_eq!(warnings.len(), 2);
        assert!(warnings[0].contains("scratch that"));
        assert!(warnings[1].contains("Duplicate"));
    }

    #[test]
    fn snippet_matches_whole_phrase_only() {
        let snippets = vec![Snippet {
            trigger: "my email".to_string(),
            expansion: "user@example.com".to_string(),
        }];
        assert_eq!(
            match_snippet("My email.", &snippets),
            Some("user@example.com")
        );
        assert_eq!(match_snippet("send my email now", &snippets), None);
        assert_eq!(match_snippet("my address", &snippets), None);
    }
}
