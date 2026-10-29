//! Conservative vocabulary formatting after speech recognition.
//!
//! Correct casing and exact word joins, without guessing another spelling.
//! Phonetic similarity alone is not evidence that an ordinary word is wrong:
//! a glossary containing "Fresha" must not change "a fresh approach".

use std::collections::{HashMap, HashSet};

pub mod replacements;

const MAX_WINDOW: usize = 3;

/// Apply exact vocabulary formatting, preserving unmatched text byte for byte.
pub fn correct(text: &str, vocabulary: &[String]) -> String {
    correct_counted(text, vocabulary).0
}

/// Return the formatted text and the number of changed spans.
pub fn correct_counted(text: &str, vocabulary: &[String]) -> (String, usize) {
    if text.is_empty() || vocabulary.is_empty() {
        return (text.to_string(), 0);
    }
    let lookup = Lookup::build(vocabulary);
    let (tokens, trailing_ws) = tokenize(text);
    let mut out = String::with_capacity(text.len() + 16);
    let mut corrections = 0;
    let mut i = 0;
    while i < tokens.len() {
        let tok = &tokens[i];
        out.push_str(tok.ws);
        out.push_str(tok.prefix);
        if let Some((consumed, term)) = longest_match(&tokens, i, &lookup) {
            out.push_str(term);
            out.push_str(tokens[i + consumed - 1].suffix);
            corrections += 1;
            i += consumed;
        } else {
            out.push_str(tok.core);
            out.push_str(tok.suffix);
            i += 1;
        }
    }
    out.push_str(trailing_ws);
    (out, corrections)
}

struct Lookup<'a> {
    exact: HashSet<&'a str>,
    casing: HashMap<String, &'a str>,
    joined: HashMap<String, &'a str>,
}

impl<'a> Lookup<'a> {
    fn build(vocabulary: &'a [String]) -> Self {
        let mut lookup = Self {
            exact: HashSet::new(),
            casing: HashMap::new(),
            joined: HashMap::new(),
        };
        for text in vocabulary
            .iter()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
        {
            lookup.exact.insert(text);
            lookup.casing.entry(text.to_lowercase()).or_insert(text);
            lookup
                .joined
                .entry(spoken_words(text).concat())
                .or_insert(text);
        }
        lookup
    }
}

fn longest_match<'a>(tokens: &[Token], i: usize, lookup: &Lookup<'a>) -> Option<(usize, &'a str)> {
    // An explicit vocabulary hit takes priority over joining it into another term.
    if lookup.exact.contains(tokens[i].core) {
        return None;
    }
    for size in (2..=MAX_WINDOW).rev() {
        if i + size <= tokens.len()
            && let Some(term) = match_window(&tokens[i..i + size], lookup)
        {
            return Some((size, term));
        }
    }
    lookup
        .casing
        .get(&tokens[i].core.to_lowercase())
        .map(|&term| (1, term))
}

fn match_window<'a>(window: &[Token], lookup: &Lookup<'a>) -> Option<&'a str> {
    let last = window.len() - 1;
    for (j, tok) in window.iter().enumerate() {
        if tok.core.is_empty()
            || lookup.exact.contains(tok.core)
            || (j > 0 && (!tok.prefix.is_empty() || tok.ws.contains(['\r', '\n'])))
            || (j < last && !tok.suffix.is_empty())
        {
            return None;
        }
    }
    let joined: String = window.iter().map(|t| t.core.to_lowercase()).collect();
    let &term = lookup.joined.get(&joined)?;
    (term != span_text(window)).then_some(term)
}

/// One whitespace-delimited chunk: the whitespace before it, leading
/// punctuation, the alphanumeric-edged core, and trailing punctuation.
/// Interior punctuation ("don't", "well-known") stays inside the core so
/// words are never split or merged across it.
struct Token<'a> {
    ws: &'a str,
    prefix: &'a str,
    core: &'a str,
    suffix: &'a str,
}

/// Split `text` into tokens plus any trailing whitespace, preserving every
/// byte so an untouched text reconstructs exactly.
fn tokenize(text: &str) -> (Vec<Token<'_>>, &str) {
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < text.len() {
        let ws_end = scan(text, i, char::is_whitespace);
        if ws_end == text.len() {
            return (tokens, &text[i..]);
        }
        let chunk_end = scan(text, ws_end, |c| !c.is_whitespace());
        let (prefix, core, suffix) = split_edges(&text[ws_end..chunk_end]);
        tokens.push(Token {
            ws: &text[i..ws_end],
            prefix,
            core,
            suffix,
        });
        i = chunk_end;
    }
    (tokens, "")
}

/// Byte offset of the first char at or after `start` failing `pred`
/// (always a char boundary).
fn scan(text: &str, start: usize, pred: impl Fn(char) -> bool) -> usize {
    text[start..]
        .char_indices()
        .find(|&(_, c)| !pred(c))
        .map_or(text.len(), |(off, _)| start + off)
}

/// Split a chunk into (leading punctuation, core, trailing punctuation) on
/// the outermost alphanumeric chars. A chunk with none is all prefix.
fn split_edges(chunk: &str) -> (&str, &str, &str) {
    let Some(start) = chunk
        .char_indices()
        .find(|&(_, c)| c.is_alphanumeric())
        .map(|(i, _)| i)
    else {
        return (chunk, "", "");
    };
    let end = chunk
        .char_indices()
        .rev()
        .find(|&(_, c)| c.is_alphanumeric())
        .map_or(chunk.len(), |(i, c)| i + c.len_utf8());
    (&chunk[..start], &chunk[start..end], &chunk[end..])
}

/// The original text of a window: cores joined by the whitespace between
/// them (interior prefixes/suffixes are empty by the window gates).
fn span_text(window: &[Token]) -> String {
    let mut span = String::new();
    for (j, tok) in window.iter().enumerate() {
        if j > 0 {
            span.push_str(tok.ws);
        }
        span.push_str(tok.core);
    }
    span
}

/// Lowercased spoken words of a term, split on camelCase, snake_case, kebab,
/// and letter/digit boundaries — the tokenizer idiom from
/// `indexer::resolve::tokenize`, copied self-contained with digit-boundary
/// splits added ("http2" -> "http", "2").
fn spoken_words(term: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut prev: Option<char> = None;
    for ch in term.chars() {
        if !ch.is_alphanumeric() {
            flush_word(&mut words, &mut current);
            prev = None;
            continue;
        }
        let boundary = prev.is_some_and(|p| {
            (ch.is_uppercase() && (p.is_lowercase() || p.is_numeric()))
                || (ch.is_numeric() != p.is_numeric())
        });
        if boundary {
            flush_word(&mut words, &mut current);
        }
        current.extend(ch.to_lowercase());
        prev = Some(ch);
    }
    flush_word(&mut words, &mut current);
    words
}

fn flush_word(words: &mut Vec<String>, current: &mut String) {
    if !current.is_empty() {
        words.push(std::mem::take(current));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vocab(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn casing_repair_single_word() {
        let v = vocab(&["GitHub", "PostgreSQL"]);
        assert_eq!(
            correct("i pushed to github today", &v),
            "i pushed to GitHub today"
        );
        assert_eq!(correct("postgresql is fast", &v), "PostgreSQL is fast");
    }

    #[test]
    fn exact_match_stays_untouched() {
        let v = vocab(&["GitHub"]);
        assert_eq!(correct("GitHub is great", &v), "GitHub is great");
    }

    #[test]
    fn explicit_common_word_can_still_receive_casing() {
        let v = vocab(&["Will"]);
        assert_eq!(correct("i asked will to help", &v), "i asked Will to help");
    }

    #[test]
    fn similar_spelling_alone_never_authorizes_a_replacement() {
        let v = vocab(&["Kubernetes", "Postgres"]);
        assert_eq!(
            correct("we deployed kubernetis", &v),
            "we deployed kubernetis"
        );
        assert_eq!(
            correct("dump it from postgrez", &v),
            "dump it from postgrez"
        );
    }

    #[test]
    fn preserves_too_short_word() {
        let v = vocab(&["Cat"]);
        assert_eq!(correct("the ct scanner", &v), "the ct scanner");
    }

    #[test]
    fn preserves_stoplist_word() {
        let v = vocab(&["Wold"]);
        assert_eq!(correct("i would go", &v), "i would go");
    }

    #[test]
    fn preserves_distance_too_far() {
        let v = vocab(&["Kubernetes"]);
        assert_eq!(correct("try coopernetes now", &v), "try coopernetes now");
    }

    #[test]
    fn preserves_key_mismatch() {
        let v = vocab(&["Rust"]);
        assert_eq!(correct("a clever ruse", &v), "a clever ruse");
    }

    #[test]
    fn two_word_window_merges() {
        let v = vocab(&["GitHub"]);
        assert_eq!(
            correct("push it to git hub now", &v),
            "push it to GitHub now"
        );
    }

    #[test]
    fn three_word_window_merges() {
        let v = vocab(&["UserControllerTest"]);
        assert_eq!(
            correct("the user controller test is failing", &v),
            "the UserControllerTest is failing"
        );
    }

    #[test]
    fn snake_case_term_matches_spoken_words() {
        let v = vocab(&["user_controller"]);
        assert_eq!(
            correct("open the user controller", &v),
            "open the user_controller"
        );
    }

    #[test]
    fn longest_window_wins() {
        let v = vocab(&["GitHub", "GitHubActions"]);
        assert_eq!(
            correct("git hub actions failed", &v),
            "GitHubActions failed"
        );
    }

    #[test]
    fn words_consumed_at_most_once_left_to_right() {
        let v = vocab(&["GitHub"]);
        assert_eq!(correct("git hub hub", &v), "GitHub hub");
        // The first "git" finds no partner; the later pair still merges.
        assert_eq!(correct("git git hub", &v), "git GitHub");
    }

    #[test]
    fn exact_vocab_token_never_merged_into_window() {
        // "git" is itself a vocab term, so it is frozen rather than fused
        // into "GitHub".
        let v = vocab(&["git", "GitHub"]);
        assert_eq!(correct("git hub", &v), "git hub");
    }

    #[test]
    fn interior_punctuation_breaks_window() {
        let v = vocab(&["GitHub"]);
        assert_eq!(correct("git, hub", &v), "git, hub");
    }

    #[test]
    fn punctuation_preserved_around_corrections() {
        let v = vocab(&["GitHub", "PostgreSQL"]);
        assert_eq!(correct("i love github, daily", &v), "i love GitHub, daily");
        assert_eq!(correct("(git hub)", &v), "(GitHub)");
        assert_eq!(
            correct("we moved to postgresql.", &v),
            "we moved to PostgreSQL."
        );
    }

    #[test]
    fn sentence_positions_start_mid_end() {
        let v = vocab(&["GitHub"]);
        assert_eq!(correct("github hosts code", &v), "GitHub hosts code");
        assert_eq!(
            correct("clone from github then build", &v),
            "clone from GitHub then build"
        );
        assert_eq!(correct("push this to github", &v), "push this to GitHub");
    }

    #[test]
    fn whitespace_preserved_exactly() {
        let v = vocab(&["GitHub"]);
        assert_eq!(
            correct("hello   github\nworld ", &v),
            "hello   GitHub\nworld "
        );
    }

    #[test]
    fn empty_vocab_and_empty_text_fast_paths() {
        assert_eq!(correct("git hub stays as is", &[]), "git hub stays as is");
        assert_eq!(correct("", &vocab(&["GitHub"])), "");
    }

    #[test]
    fn idempotent() {
        let v = vocab(&["GitHub", "Kubernetes", "UserController", "PostgreSQL"]);
        for text in [
            "i pushed the user controller to github and deployed postgresql on kubernetis.",
            "git hub rocks",
            "plain sentence with nothing to fix",
        ] {
            let once = correct(text, &v);
            assert_eq!(correct(&once, &v), once, "not idempotent for {text:?}");
        }
    }

    #[test]
    fn unicode_term_and_text() {
        let v = vocab(&["Müller", "Kubernetes"]);
        assert_eq!(
            correct("we met müller at the kubernetis talk", &v),
            "we met Müller at the kubernetis talk"
        );
        // Unrelated non-ASCII words pass through untouched.
        assert_eq!(correct("the café was naïve", &v), "the café was naïve");
    }

    #[test]
    fn realistic_mixed_sentence() {
        let v = vocab(&["GitHub", "PostgreSQL", "Kubernetes", "UserController"]);
        let (out, count) = correct_counted(
            "i pushed the user controller to github and deployed postgresql on kubernetis.",
            &v,
        );
        assert_eq!(
            out,
            "i pushed the UserController to GitHub and deployed PostgreSQL on kubernetis."
        );
        assert_eq!(count, 3);
    }

    #[test]
    fn ordinary_words_survive_a_collision_heavy_dictionary() {
        let v = vocab(&["Fresha", "Cloude", "Wold", "Oneli", "Rust", "Planer"]);
        for text in [
            "We need a fresh approach.",
            "The cloud is only one option.",
            "I would rather leave the plane alone.",
            "A clever ruse does not change my plan.",
        ] {
            assert_eq!(correct_counted(text, &v), (text.to_string(), 0));
        }
    }

    #[test]
    fn exact_joining_includes_short_words_but_never_crosses_a_line() {
        let v = vocab(&["no-show", "TypeScript", "GitHub"]);
        assert_eq!(
            correct("a no show in type script", &v),
            "a no-show in TypeScript"
        );
        assert_eq!(correct("git\n hub", &v), "git\n hub");
    }

    #[test]
    fn no_corrections_counts_zero() {
        let v = vocab(&["GitHub"]);
        let (out, count) = correct_counted("nothing relevant here", &v);
        assert_eq!(out, "nothing relevant here");
        assert_eq!(count, 0);
    }
}
