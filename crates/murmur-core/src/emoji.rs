//! Spoken emoji: "emoji <name>" in a dictated phrase becomes the glyph.
//!
//! The explicit "emoji" keyword before each name keeps prose safe from false
//! triggers: "fire" alone is never rewritten, only "emoji fire" is.

use crate::voice_commands::{Word, word_tokens};

/// Spoken names and their glyphs. Names are lowercase, one or two words;
/// the longest name wins at a given position ("ok hand" over "ok").
const EMOJI: &[(&str, &str)] = &[
    ("brain", "🧠"),
    ("bug", "🐛"),
    ("bulb", "💡"),
    ("check", "✅"),
    ("check mark", "✅"),
    ("clap", "👏"),
    ("coffee", "☕"),
    ("cool", "😎"),
    ("crab", "🦀"),
    ("cross", "❌"),
    ("cry", "😢"),
    ("eyes", "👀"),
    ("fire", "🔥"),
    ("heart", "❤️"),
    ("hundred", "💯"),
    ("laugh", "😂"),
    ("light bulb", "💡"),
    ("muscle", "💪"),
    ("ok", "👌"),
    ("ok hand", "👌"),
    ("okay", "👌"),
    ("party", "🎉"),
    ("pray", "🙏"),
    ("rocket", "🚀"),
    ("rust", "🦀"),
    ("shrug", "🤷"),
    ("smile", "🙂"),
    ("snake", "🐍"),
    ("sparkles", "✨"),
    ("star", "⭐"),
    ("sunglasses", "😎"),
    ("tada", "🎉"),
    ("thinking", "🤔"),
    ("thumbs down", "👎"),
    ("thumbs up", "👍"),
    ("warning", "⚠️"),
    ("wave", "👋"),
    ("wink", "😉"),
    ("x", "❌"),
];

/// Replace every "emoji <name>" span in `phrase` with its glyph.
///
/// "emoji" must appear as a whole word (case-insensitive) immediately followed
/// by a known name (one or two words; the longest match wins). Surrounding
/// text and spacing are preserved; an unknown name leaves that occurrence as
/// literal text. Returns `Some` only when at least one substitution actually
/// happened, so ordinary dictation is delivered unchanged.
pub fn substitute_emoji(phrase: &str) -> Option<String> {
    let words = word_tokens(phrase);
    let mut spans: Vec<(usize, usize, &str)> = Vec::new();
    let mut i = 0;
    while i < words.len() {
        if words[i].lower != "emoji" {
            i += 1;
            continue;
        }
        match longest_name_at(&words[i + 1..]) {
            Some((name_len, glyph)) => {
                spans.push((words[i].start, words[i + name_len].end, glyph));
                i += 1 + name_len;
            }
            None => i += 1,
        }
    }
    if spans.is_empty() {
        return None;
    }
    let mut result = String::with_capacity(phrase.len());
    let mut cursor = 0;
    for (start, end, glyph) in spans {
        result.push_str(&phrase[cursor..start]);
        result.push_str(glyph);
        cursor = end;
    }
    result.push_str(&phrase[cursor..]);
    Some(result)
}

/// The longest known name starting at `words[0]`, as (word count, glyph).
fn longest_name_at(words: &[Word]) -> Option<(usize, &'static str)> {
    EMOJI
        .iter()
        .filter_map(|(name, glyph)| Some((match_len(name, words)?, *glyph)))
        .max_by_key(|(len, _)| *len)
}

/// Word count of `name` if it matches the head of `words`, else `None`.
fn match_len(name: &str, words: &[Word]) -> Option<usize> {
    let mut len = 0;
    for part in name.split(' ') {
        if words.get(len)?.lower != part {
            return None;
        }
        len += 1;
    }
    Some(len)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_name_substitutes() {
        assert_eq!(substitute_emoji("emoji fire"), Some("🔥".to_string()));
        assert_eq!(substitute_emoji("emoji thumbs up"), Some("👍".to_string()));
    }

    #[test]
    fn inline_preserves_surrounding_text_and_spacing() {
        assert_eq!(
            substitute_emoji("great work emoji fire today"),
            Some("great work 🔥 today".to_string())
        );
        assert_eq!(
            substitute_emoji("ship it emoji rocket!"),
            Some("ship it 🚀!".to_string())
        );
    }

    #[test]
    fn two_word_name_beats_one_word() {
        // "ok" and "check" are names themselves; the two-word forms must win.
        assert_eq!(substitute_emoji("emoji ok hand"), Some("👌".to_string()));
        assert_eq!(substitute_emoji("emoji check mark"), Some("✅".to_string()));
        // The shorter name still works when nothing follows it.
        assert_eq!(
            substitute_emoji("looks emoji ok to me"),
            Some("looks 👌 to me".to_string())
        );
    }

    #[test]
    fn multiple_emoji_in_one_phrase() {
        assert_eq!(
            substitute_emoji("emoji check merged and emoji tada"),
            Some("✅ merged and 🎉".to_string())
        );
        assert_eq!(
            substitute_emoji("emoji thumbs up emoji rocket"),
            Some("👍 🚀".to_string())
        );
    }

    #[test]
    fn unknown_name_stays_literal() {
        assert_eq!(substitute_emoji("emoji unicorn"), None);
        assert_eq!(
            substitute_emoji("emoji unicorn but emoji fire works"),
            Some("emoji unicorn but 🔥 works".to_string())
        );
    }

    #[test]
    fn trailing_keyword_without_a_name_returns_none() {
        assert_eq!(substitute_emoji("I love a good emoji"), None);
        assert_eq!(substitute_emoji("emoji"), None);
    }

    #[test]
    fn no_emoji_keyword_returns_none() {
        assert_eq!(substitute_emoji("great work fire rocket"), None);
        assert_eq!(substitute_emoji(""), None);
    }

    #[test]
    fn embedded_in_a_word_does_not_match() {
        assert_eq!(substitute_emoji("emojifire"), None);
        assert_eq!(substitute_emoji("my emojis fire me up"), None);
    }

    #[test]
    fn case_insensitive() {
        assert_eq!(substitute_emoji("Emoji Fire"), Some("🔥".to_string()));
        assert_eq!(substitute_emoji("EMOJI THUMBS UP"), Some("👍".to_string()));
    }

    #[test]
    fn punctuation_between_keyword_and_name_is_tolerated() {
        // Whisper often inserts a comma; the whole span is replaced.
        assert_eq!(
            substitute_emoji("nice, emoji, fire."),
            Some("nice, 🔥.".to_string())
        );
    }
}
