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

/// Classify a transcribed phrase as a built-in editing command, or `Text`.
pub fn parse(phrase: &str) -> VoiceCommand {
    match normalize(phrase).as_str() {
        "new line" | "newline" => VoiceCommand::NewLine,
        "new paragraph" => VoiceCommand::NewParagraph,
        "scratch that" | "delete that" => VoiceCommand::ScratchThat,
        "copy that" | "copy selection" => VoiceCommand::Copy,
        "undo" | "undo that" => VoiceCommand::Undo,
        "redo" | "redo that" => VoiceCommand::Redo,
        "press tab" | "tab key" => VoiceCommand::Tab,
        "press escape" | "escape key" => VoiceCommand::Escape,
        _ => VoiceCommand::Text,
    }
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
        assert_eq!(parse("undo"), VoiceCommand::Undo);
        assert_eq!(parse("redo that"), VoiceCommand::Redo);
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
