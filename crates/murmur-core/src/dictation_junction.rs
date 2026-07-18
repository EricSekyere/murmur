//! Junction repair for premature terminal punctuation between streamed
//! dictation phrases ("smart punctuation").
//!
//! A short pause flushes a phrase mid-sentence and the STT model punctuates
//! the fragment as a finished utterance ("...store." + "And bought...").
//! When the next phrase clearly continues the sentence, the stale mark and
//! its trailing space are backspaced and the phrases are joined. Pure
//! decision logic only — the app layer verifies the environment (focus,
//! output mode, delivery kind) and performs the keystrokes.

use std::time::Duration;

/// Longest gap between two deliveries that still counts as one sentence.
pub const JOIN_WINDOW: Duration = Duration::from_secs(8);

/// Keystrokes removing the previous phrase's trailing space + terminal mark.
const JUNCTION_BACKSPACES: usize = 2;

/// Conjunctions the model capitalizes at a spurious sentence start. A
/// capitalized word outside this list is taken as a genuinely new sentence;
/// a lowercase first letter marks a continuation regardless of the word.
const CAPITALIZED_CONJUNCTIONS: &[&str] = &["and", "but", "or", "so", "because", "which"];

/// Sentence-final tokens whose '.' is not a terminal mark.
const ABBREVIATIONS: &[&str] = &["e.g.", "i.e.", "etc.", "dr.", "mr.", "mrs.", "ms.", "vs."];

/// Environment conditions the app layer verifies; the decision trusts them
/// and falls back to unchanged delivery when any is false.
#[derive(Debug, Clone, Copy)]
pub struct JunctionGates {
    /// The `smart_punctuation` setting is on.
    pub enabled: bool,
    /// The previous delivery was typed keystrokes (not clipboard/stdout).
    pub prev_typed: bool,
    /// The previous delivery was plain dictation (no snippet expansion,
    /// commit line, literal escape, or clipboard substitution).
    pub prev_plain_dictation: bool,
    /// The current delivery is plain dictation (same exclusions).
    pub next_plain_dictation: bool,
    /// The current output mode types keystrokes (Auto or Keyboard; never
    /// clipboard-paste, where stray backspaces can misfire).
    pub typing_mode: bool,
    /// The target window is unchanged since the previous delivery.
    pub same_target: bool,
}

impl JunctionGates {
    fn all_pass(&self) -> bool {
        self.enabled
            && self.prev_typed
            && self.prev_plain_dictation
            && self.next_plain_dictation
            && self.typing_mode
            && self.same_target
    }
}

/// What to do with an arriving phrase relative to the previous delivery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JunctionAction {
    /// Deliver the phrase unchanged (today's behavior).
    DeliverAsIs,
    /// Backspace `backspaces` characters (the previous phrase's trailing
    /// space + terminal mark), then deliver `replacement` with a joining
    /// leading space and the usual trailing space.
    Repair {
        backspaces: usize,
        replacement: String,
    },
}

/// Decide whether `next_text` continues the sentence `prev_text` was cut
/// from. `prev_text` is the previously delivered text (trimmed, without the
/// trailing auto-space); `elapsed` is wall-clock time since that delivery.
pub fn junction_action(
    prev_text: &str,
    next_text: &str,
    elapsed: Duration,
    gates: &JunctionGates,
) -> JunctionAction {
    if !gates.all_pass() || elapsed > JOIN_WINDOW {
        return JunctionAction::DeliverAsIs;
    }
    if !ends_in_repairable_terminal(prev_text) {
        return JunctionAction::DeliverAsIs;
    }
    match continuation_replacement(next_text) {
        Some(replacement) => JunctionAction::Repair {
            backspaces: JUNCTION_BACKSPACES,
            replacement,
        },
        None => JunctionAction::DeliverAsIs,
    }
}

/// Whether the previous phrase ends in a terminal mark that is safe to remove.
fn ends_in_repairable_terminal(prev_text: &str) -> bool {
    let trimmed = prev_text.trim_end();
    let mut rev = trimmed.chars().rev();
    let Some(last) = rev.next() else {
        return false;
    };
    if !matches!(last, '.' | '?' | '!') {
        return false;
    }
    if last != '.' {
        return true;
    }
    let before = rev.next();
    // A '.' before the mark is an ellipsis ("..."), not a sentence end.
    if before == Some('.') {
        return false;
    }
    // A digit before the '.' reads as a number ("3.14.", "version 2."),
    // where removing the mark could corrupt meaning.
    if before.is_some_and(|c| c.is_ascii_digit()) {
        return false;
    }
    !ends_in_abbreviation(trimmed)
}

/// Whether the final token is a dotted abbreviation rather than a sentence end.
fn ends_in_abbreviation(trimmed: &str) -> bool {
    let Some(token) = trimmed.split_whitespace().last() else {
        return false;
    };
    let lower = token.to_lowercase();
    if ABBREVIATIONS.contains(&lower.as_str()) {
        return true;
    }
    // Generic dotted shapes the list can't enumerate: an interior '.'
    // ("U.S.") or a single letter before the final '.' ("J.").
    let Some(stem) = lower.strip_suffix('.') else {
        return false;
    };
    stem.contains('.') || stem.chars().count() == 1
}

/// The next phrase lowercased at its first letter, when it reads as a
/// continuation; None when it looks like a legitimate new sentence.
fn continuation_replacement(next_text: &str) -> Option<String> {
    let (first, word) = leading_word(next_text)?;
    let continues = first.is_lowercase()
        || (first.is_uppercase()
            && CAPITALIZED_CONJUNCTIONS.contains(&word.to_lowercase().as_str()));
    continues.then(|| lowercase_first_letter(next_text))
}

/// First letter and first word of the phrase, past any leading quotes or
/// brackets. None when the phrase doesn't start with a letter.
fn leading_word(text: &str) -> Option<(char, String)> {
    let rest = text.trim_start().trim_start_matches(|c: char| {
        matches!(
            c,
            '"' | '\'' | '(' | '[' | '{' | '\u{2018}' | '\u{2019}' | '\u{201C}' | '\u{201D}'
        )
    });
    let first = rest.chars().next()?;
    if !first.is_alphabetic() {
        return None;
    }
    let word: String = rest
        .chars()
        .take_while(|c| c.is_alphabetic() || *c == '\'')
        .collect();
    Some((first, word))
}

/// Lowercase the first alphabetic character, Unicode-aware (a single
/// uppercase letter may lowercase to multiple chars).
fn lowercase_first_letter(text: &str) -> String {
    let Some((idx, ch)) = text.char_indices().find(|(_, c)| c.is_alphabetic()) else {
        return text.to_string();
    };
    let mut out = String::with_capacity(text.len());
    out.push_str(&text[..idx]);
    out.extend(ch.to_lowercase());
    out.push_str(&text[idx + ch.len_utf8()..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const ELAPSED: Duration = Duration::from_secs(2);

    fn open_gates() -> JunctionGates {
        JunctionGates {
            enabled: true,
            prev_typed: true,
            prev_plain_dictation: true,
            next_plain_dictation: true,
            typing_mode: true,
            same_target: true,
        }
    }

    fn repair(replacement: &str) -> JunctionAction {
        JunctionAction::Repair {
            backspaces: 2,
            replacement: replacement.to_string(),
        }
    }

    #[test]
    fn repairs_lowercase_continuation() {
        assert_eq!(
            junction_action(
                "I went to the store.",
                "and bought some milk.",
                ELAPSED,
                &open_gates()
            ),
            repair("and bought some milk.")
        );
        // Lowercase start continues even for a non-conjunction word.
        assert_eq!(
            junction_action(
                "I went to the store.",
                "went on foot.",
                ELAPSED,
                &open_gates()
            ),
            repair("went on foot.")
        );
    }

    #[test]
    fn repairs_capitalized_short_conjunctions() {
        for (next, joined) in [
            ("And bought some milk.", "and bought some milk."),
            ("But not the bread.", "but not the bread."),
            ("Or maybe tomorrow.", "or maybe tomorrow."),
            ("So we left early.", "so we left early."),
            ("Because it was late.", "because it was late."),
            ("Which was closed.", "which was closed."),
        ] {
            assert_eq!(
                junction_action("I went to the store.", next, ELAPSED, &open_gates()),
                repair(joined),
                "should repair: {next:?}"
            );
        }
    }

    #[test]
    fn capitalized_ordinary_word_is_a_new_sentence() {
        for next in [
            "Tomorrow we ship.",
            "The dog barked.",
            // "that" is a continuation word only when lowercase; capitalized
            // it is outside the short conjunction list by design.
            "That said, fine.",
            "Then we left.",
        ] {
            assert_eq!(
                junction_action("I went to the store.", next, ELAPSED, &open_gates()),
                JunctionAction::DeliverAsIs,
                "should not repair: {next:?}"
            );
        }
    }

    #[test]
    fn question_and_exclamation_marks_repair_like_periods() {
        assert_eq!(
            junction_action("Is that right?", "and then what?", ELAPSED, &open_gates()),
            repair("and then what?")
        );
        assert_eq!(
            junction_action("Stop!", "and listen.", ELAPSED, &open_gates()),
            repair("and listen.")
        );
    }

    #[test]
    fn leading_quote_is_skipped_when_classifying_and_lowercasing() {
        assert_eq!(
            junction_action(
                "I went to the store.",
                "\"And so it goes.\"",
                ELAPSED,
                &open_gates()
            ),
            repair("\"and so it goes.\"")
        );
    }

    #[test]
    fn each_gate_individually_blocks() {
        let flips: [fn(&mut JunctionGates); 6] = [
            |g| g.enabled = false,
            |g| g.prev_typed = false,
            |g| g.prev_plain_dictation = false,
            |g| g.next_plain_dictation = false,
            |g| g.typing_mode = false,
            |g| g.same_target = false,
        ];
        for (i, flip) in flips.iter().enumerate() {
            let mut gates = open_gates();
            flip(&mut gates);
            assert_eq!(
                junction_action("I went to the store.", "and bought milk.", ELAPSED, &gates),
                JunctionAction::DeliverAsIs,
                "gate {i} should block"
            );
        }
    }

    #[test]
    fn elapsed_within_window_repairs_but_longer_blocks() {
        assert_eq!(
            junction_action(
                "I went to the store.",
                "and bought milk.",
                JOIN_WINDOW,
                &open_gates()
            ),
            repair("and bought milk.")
        );
        assert_eq!(
            junction_action(
                "I went to the store.",
                "and bought milk.",
                JOIN_WINDOW + Duration::from_secs(1),
                &open_gates()
            ),
            JunctionAction::DeliverAsIs
        );
    }

    #[test]
    fn previous_without_terminal_mark_is_left_alone() {
        for prev in ["I went to the store", "I went to the store,", ""] {
            assert_eq!(
                junction_action(prev, "and bought milk.", ELAPSED, &open_gates()),
                JunctionAction::DeliverAsIs,
                "should not repair after: {prev:?}"
            );
        }
    }

    #[test]
    fn abbreviation_endings_are_refused() {
        for prev in [
            "We talked to Dr.",
            "Ask Mr.",
            "Ask Mrs.",
            "Ask Ms.",
            "for example e.g.",
            "that is i.e.",
            "apples, pears, etc.",
            "cats vs.",
            // Interior-dot and single-letter shapes outside the list.
            "made in the U.S.",
            "signed J.",
        ] {
            assert_eq!(
                junction_action(prev, "and more.", ELAPSED, &open_gates()),
                JunctionAction::DeliverAsIs,
                "should not repair after: {prev:?}"
            );
        }
    }

    #[test]
    fn ellipsis_is_refused() {
        assert_eq!(
            junction_action("Well...", "and then.", ELAPSED, &open_gates()),
            JunctionAction::DeliverAsIs
        );
    }

    #[test]
    fn digit_before_the_mark_is_refused() {
        for prev in ["The value is 3.14.", "It costs 42."] {
            assert_eq!(
                junction_action(prev, "and more.", ELAPSED, &open_gates()),
                JunctionAction::DeliverAsIs,
                "should not repair after: {prev:?}"
            );
        }
    }

    #[test]
    fn backspace_count_and_replacement_are_exact() {
        let action = junction_action(
            "I went to the store.",
            "And bought some milk.",
            ELAPSED,
            &open_gates(),
        );
        assert_eq!(
            action,
            JunctionAction::Repair {
                backspaces: 2,
                replacement: "and bought some milk.".to_string(),
            }
        );
    }

    #[test]
    fn first_letter_lowercasing_is_unicode_safe() {
        assert_eq!(lowercase_first_letter("Épico final."), "épico final.");
        assert_eq!(lowercase_first_letter("\"Über uns\""), "\"über uns\"");
        // 'İ' lowercases to two chars; the tail must stay intact.
        assert_eq!(lowercase_first_letter("İstanbul"), "i\u{307}stanbul");
    }

    #[test]
    fn capitalized_non_letter_start_is_left_alone() {
        assert_eq!(
            junction_action("I went to the store.", "42 items.", ELAPSED, &open_gates()),
            JunctionAction::DeliverAsIs
        );
    }
}
