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
/// Shared with the command-mode grammar so both matchers hear the same words.
pub(crate) fn normalize(phrase: &str) -> String {
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
    // A trigger that normalizes to "" (empty, or pure punctuation like "...")
    // would match the bare "." / "?" that Whisper emits on silence or noise,
    // auto-typing the expansion. Never fire on an empty match.
    if normalized.is_empty() {
        return None;
    }
    snippets
        .iter()
        .find(|s| normalize(&s.trigger) == normalized)
        .map(|s| s.expansion.as_str())
}

/// Splice the clipboard text over every spoken placeholder in `phrase`.
///
/// Placeholders match case-insensitively as whole words: a multi-word
/// placeholder must appear as a contiguous word sequence ("insert clipboard
/// data" contains it; "reinserts clipboard" does not). `read_clipboard` is
/// only invoked when a placeholder actually occurs, so ordinary dictation
/// never touches the clipboard. Returns `None` — deliver the phrase
/// unchanged — when nothing matches or the clipboard is empty/unreadable,
/// so the placeholder words are never silently deleted.
pub fn substitute_clipboard(
    phrase: &str,
    placeholders: &[String],
    read_clipboard: impl FnOnce() -> Option<String>,
) -> Option<String> {
    let patterns: Vec<Vec<String>> = placeholders
        .iter()
        .map(|p| word_tokens(p).into_iter().map(|w| w.lower).collect())
        .filter(|words: &Vec<String>| !words.is_empty())
        .collect();
    if patterns.is_empty() {
        return None;
    }
    let spans = placeholder_spans(&word_tokens(phrase), &patterns);
    if spans.is_empty() {
        return None;
    }
    let clip = read_clipboard()?;
    if clip.trim().is_empty() {
        return None;
    }
    let mut result = String::with_capacity(phrase.len() + clip.len());
    let mut cursor = 0;
    for (start, end) in spans {
        result.push_str(&phrase[cursor..start]);
        result.push_str(&clip);
        cursor = end;
    }
    result.push_str(&phrase[cursor..]);
    Some(result)
}

/// A word in the original string: its byte span plus lowercased text.
/// Shared with the emoji substituter so word boundaries stay consistent.
pub(crate) struct Word {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) lower: String,
}

/// Maximal alphanumeric runs, so punctuation and whitespace act as word
/// boundaries ("clipboard," matches "clipboard"; "reinserts" stays one word).
pub(crate) fn word_tokens(text: &str) -> Vec<Word> {
    let mut words = Vec::new();
    let mut current: Option<Word> = None;
    for (i, ch) in text.char_indices() {
        if ch.is_alphanumeric() {
            let word = current.get_or_insert_with(|| Word {
                start: i,
                end: i,
                lower: String::new(),
            });
            word.end = i + ch.len_utf8();
            word.lower.extend(ch.to_lowercase());
        } else if let Some(word) = current.take() {
            words.push(word);
        }
    }
    words.extend(current);
    words
}

/// Non-overlapping byte spans where any pattern occurs as a contiguous word
/// sequence; the longest pattern wins at each position.
fn placeholder_spans(words: &[Word], patterns: &[Vec<String>]) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let hit = patterns
            .iter()
            .filter(|pat| {
                words[i..].len() >= pat.len()
                    && words[i..i + pat.len()]
                        .iter()
                        .zip(pat.iter())
                        .all(|(w, p)| w.lower == *p)
            })
            .max_by_key(|pat| pat.len());
        match hit {
            Some(pat) => {
                spans.push((words[i].start, words[i + pat.len() - 1].end));
                i += pat.len();
            }
            None => i += 1,
        }
    }
    spans
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
    fn empty_or_punctuation_trigger_never_fires_on_silence() {
        // A hand-edited or punctuation-only trigger normalizes to "" and would
        // otherwise match the bare "." / "?" Whisper emits on silence/noise.
        let snippets = vec![
            Snippet {
                trigger: "...".to_string(),
                expansion: "my-secret-signature".to_string(),
            },
            Snippet {
                trigger: "".to_string(),
                expansion: "boom".to_string(),
            },
        ];
        for phrase in [".", "?", "!!", "  ,  ", "", "..."] {
            assert_eq!(
                match_snippet(phrase, &snippets),
                None,
                "phrase {phrase:?} must not match an empty-normalized trigger"
            );
        }
        // A real trigger still works.
        let real = vec![Snippet {
            trigger: "my email".to_string(),
            expansion: "a@b.com".to_string(),
        }];
        assert_eq!(match_snippet("my email", &real), Some("a@b.com"));
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

    fn placeholders() -> Vec<String> {
        vec![
            "insert clipboard".to_string(),
            "paste clipboard".to_string(),
        ]
    }

    #[test]
    fn clipboard_placeholder_matches_case_and_whitespace_insensitively() {
        let result = substitute_clipboard("Here: INSERT   Clipboard.", &placeholders(), || {
            Some("copied".to_string())
        });
        assert_eq!(result, Some("Here: copied.".to_string()));
    }

    #[test]
    fn clipboard_placeholder_no_match_returns_none() {
        let result = substitute_clipboard("just ordinary dictation", &placeholders(), || {
            Some("copied".to_string())
        });
        assert_eq!(result, None);
    }

    #[test]
    fn clipboard_empty_or_unreadable_leaves_phrase_unchanged() {
        for clip in [None, Some(String::new()), Some("   \n".to_string())] {
            let result = substitute_clipboard("insert clipboard", &placeholders(), || clip.clone());
            assert_eq!(result, None, "clipboard {clip:?} must not substitute");
        }
    }

    #[test]
    fn clipboard_placeholder_replaces_every_occurrence() {
        let result = substitute_clipboard(
            "insert clipboard and then paste clipboard again",
            &placeholders(),
            || Some("X".to_string()),
        );
        assert_eq!(result, Some("X and then X again".to_string()));
    }

    #[test]
    fn clipboard_placeholder_embedded_in_a_larger_word_does_not_match() {
        for phrase in ["reinserts clipboard data", "insert clipboards"] {
            let result =
                substitute_clipboard(phrase, &placeholders(), || Some("copied".to_string()));
            assert_eq!(result, None, "phrase {phrase:?} must not match");
        }
        // But extra surrounding words still match the whole-word sequence.
        let result = substitute_clipboard("please insert clipboard data", &placeholders(), || {
            Some("copied".to_string())
        });
        assert_eq!(result, Some("please copied data".to_string()));
    }

    #[test]
    fn clipboard_is_not_read_without_a_placeholder() {
        let read = std::cell::Cell::new(false);
        let result = substitute_clipboard("no placeholder here", &placeholders(), || {
            read.set(true);
            Some("copied".to_string())
        });
        assert_eq!(result, None);
        assert!(!read.get(), "clipboard must not be read on a miss");
    }

    #[test]
    fn empty_and_whitespace_placeholder_entries_are_ignored() {
        let junk = vec!["".to_string(), "   ".to_string(), "...".to_string()];
        let read = std::cell::Cell::new(false);
        let result = substitute_clipboard("any words at all", &junk, || {
            read.set(true);
            Some("copied".to_string())
        });
        assert_eq!(result, None);
        assert!(!read.get());
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
