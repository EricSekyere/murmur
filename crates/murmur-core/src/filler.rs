//! Deterministic filler-word detection: a small static phrase list matched
//! over tokenized text. Shared by usage analytics and future transcript
//! cleanup passes so the canonical set lives in exactly one place.

/// Single-token fillers, lowercase.
pub const SINGLE_WORD_FILLERS: &[&str] =
    &["um", "uh", "like", "basically", "actually", "literally"];

/// Two-token fillers, lowercase. Tried before single tokens at each position
/// so a pair counts once and consumes both words.
pub const TWO_WORD_FILLERS: &[[&str; 2]] = &[["you", "know"], ["sort", "of"], ["kind", "of"]];

/// Count filler occurrences in `text`: case-insensitive, whole-word only
/// ("likely" never matches "like"). Returns a total count and records nothing
/// about which fillers matched — callers must keep it that way for privacy.
pub fn count_fillers(text: &str) -> usize {
    // Same word boundary as history mining: `_` stays inside a token so
    // identifiers like `sort_of` don't tokenize into a filler pair.
    let tokens: Vec<String> = text
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|t| !t.is_empty())
        .map(str::to_lowercase)
        .collect();

    let mut count = 0;
    let mut i = 0;
    while i < tokens.len() {
        let is_pair = i + 1 < tokens.len()
            && TWO_WORD_FILLERS
                .iter()
                .any(|[a, b]| *a == tokens[i] && *b == tokens[i + 1]);
        if is_pair {
            count += 1;
            i += 2;
            continue;
        }
        if SINGLE_WORD_FILLERS.contains(&tokens[i].as_str()) {
            count += 1;
        }
        i += 1;
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_text_counts_zero() {
        assert_eq!(count_fillers(""), 0);
        assert_eq!(count_fillers("   \n\t"), 0);
    }

    #[test]
    fn counts_singles_and_pairs_together() {
        // um + like + "you know" (one pair, not two singles).
        assert_eq!(count_fillers("um so like you know"), 3);
    }

    #[test]
    fn whole_word_only_never_matches_substrings() {
        assert_eq!(count_fillers("likely the unlike button"), 0);
        assert_eq!(count_fillers("alike and unlikely"), 0);
    }

    #[test]
    fn matching_is_case_insensitive() {
        assert_eq!(count_fillers("Actually, BASICALLY yes"), 2);
        assert_eq!(count_fillers("You Know what I mean"), 1);
    }

    #[test]
    fn adjacent_pairs_each_count_once() {
        assert_eq!(count_fillers("sort of kind of"), 2);
    }

    #[test]
    fn pair_consumes_both_tokens_without_double_counting() {
        // "you know" is one filler; "know" alone is not a filler.
        assert_eq!(count_fillers("you know"), 1);
        assert_eq!(count_fillers("know you"), 0);
    }
}
