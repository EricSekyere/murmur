//! Spoken editing commands recognized in a transcribed phrase.
//!
//! A command only fires when it is the *entire* phrase (after normalization),
//! so dictating "press enter to continue" types text rather than executing a
//! newline.

/// The interpretation of one transcribed phrase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VoiceCommand {
    /// Insert a single line break.
    NewLine,
    /// Insert a blank line (two line breaks).
    NewParagraph,
    /// Delete the previously delivered phrase.
    ScratchThat,
    /// Ordinary dictation — deliver the text unchanged.
    Text,
}

/// Classify a transcribed phrase. Matching is case-insensitive and ignores
/// surrounding whitespace and trailing punctuation.
pub fn parse(phrase: &str) -> VoiceCommand {
    let normalized: String = phrase
        .trim()
        .trim_end_matches(['.', '!', '?', ','])
        .to_lowercase();

    match normalized.as_str() {
        "new line" | "newline" => VoiceCommand::NewLine,
        "new paragraph" => VoiceCommand::NewParagraph,
        "scratch that" | "delete that" | "undo that" => VoiceCommand::ScratchThat,
        _ => VoiceCommand::Text,
    }
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
    fn commands_inside_a_sentence_are_plain_text() {
        assert_eq!(parse("press the new line button"), VoiceCommand::Text);
        assert_eq!(parse("scratch that itch"), VoiceCommand::Text);
        assert_eq!(parse("the quick brown fox"), VoiceCommand::Text);
    }
}
