//! Vocabulary-aware correction of STT output text.
//!
//! Whisper biases decoding toward the Personal Dictionary through a prompt
//! clause, but Parakeet has no biasing API, so vocabulary terms were silently
//! ignored on that engine. This pass instead corrects the *output* text of
//! both engines, deterministically mapping misrecognized words back to
//! vocabulary terms (complementary to Whisper's prompt biasing: it fixes what
//! biasing missed).
//!
//! Two correction classes:
//! - Casing repair (safe, aggressive): a word that case-insensitively equals
//!   a term is rewritten to the term's casing ("github" -> "GitHub").
//! - Phonetic repair (guarded, conservative): a word — or a 2..=3 word window
//!   for terms spoken as several words — is replaced by a term only when the
//!   phonetic keys match AND a Levenshtein distance guard passes.
//!
//! Phonetic algorithm: a compact Soundex refinement. Consonants map to the
//! six classic Soundex classes, vowels separate runs of equal codes, and
//! `h`/`w` are transparent — but unlike classic Soundex the first letter is
//! coded by class too (so hard/soft confusions like "cubernetes"/"kubernetes"
//! share a key) and keys are variable-length instead of truncated to four
//! chars, which would over-merge long identifiers. Digits and non-ASCII
//! letters code as themselves, so non-English words only bucket together
//! when nearly identical.
//!
//! False-positive gates: words under three chars and a stoplist of common
//! English words are never phonetically corrected (casing repair stays
//! allowed); words exactly equal to a vocabulary term are frozen — never
//! rewritten or merged into a window — which also makes the pass idempotent;
//! punctuation around a corrected word is preserved exactly. There is
//! deliberately no user setting: this is the Personal Dictionary doing its
//! job, with the gates bounding the risk.

use std::collections::{HashMap, HashSet};

/// Common English words never phonetically corrected, so "there" can't become
/// a vocab term like "Terra". Casing repair still applies when such a word is
/// genuinely in the vocabulary. Alphabetized for `binary_search`.
const STOPLIST: &[&str] = &[
    "a",
    "about",
    "above",
    "actually",
    "after",
    "again",
    "against",
    "all",
    "almost",
    "also",
    "always",
    "am",
    "an",
    "and",
    "another",
    "any",
    "anything",
    "are",
    "around",
    "as",
    "ask",
    "asked",
    "at",
    "away",
    "back",
    "be",
    "became",
    "because",
    "been",
    "before",
    "being",
    "below",
    "between",
    "big",
    "both",
    "but",
    "by",
    "came",
    "can",
    "cannot",
    "come",
    "could",
    "day",
    "days",
    "did",
    "different",
    "do",
    "does",
    "doing",
    "done",
    "down",
    "each",
    "either",
    "end",
    "enough",
    "even",
    "ever",
    "every",
    "everything",
    "far",
    "few",
    "find",
    "first",
    "for",
    "found",
    "from",
    "get",
    "give",
    "go",
    "going",
    "good",
    "got",
    "great",
    "had",
    "has",
    "have",
    "having",
    "he",
    "her",
    "here",
    "hers",
    "him",
    "his",
    "how",
    "i",
    "if",
    "in",
    "into",
    "is",
    "it",
    "its",
    "itself",
    "just",
    "keep",
    "kind",
    "knew",
    "know",
    "large",
    "last",
    "left",
    "less",
    "let",
    "like",
    "little",
    "long",
    "look",
    "looked",
    "made",
    "make",
    "many",
    "may",
    "maybe",
    "me",
    "mean",
    "might",
    "mine",
    "more",
    "most",
    "much",
    "must",
    "my",
    "myself",
    "need",
    "never",
    "new",
    "next",
    "no",
    "none",
    "not",
    "nothing",
    "now",
    "of",
    "off",
    "often",
    "old",
    "on",
    "once",
    "one",
    "only",
    "or",
    "other",
    "our",
    "out",
    "over",
    "own",
    "part",
    "people",
    "place",
    "put",
    "quite",
    "rather",
    "really",
    "right",
    "said",
    "same",
    "saw",
    "say",
    "see",
    "seem",
    "seemed",
    "she",
    "should",
    "since",
    "small",
    "so",
    "some",
    "something",
    "soon",
    "still",
    "such",
    "take",
    "tell",
    "than",
    "that",
    "the",
    "their",
    "them",
    "then",
    "there",
    "these",
    "they",
    "thing",
    "things",
    "think",
    "this",
    "those",
    "thought",
    "three",
    "through",
    "time",
    "to",
    "today",
    "together",
    "told",
    "too",
    "took",
    "toward",
    "two",
    "under",
    "until",
    "up",
    "upon",
    "us",
    "use",
    "used",
    "very",
    "want",
    "was",
    "way",
    "we",
    "well",
    "went",
    "were",
    "what",
    "when",
    "where",
    "which",
    "while",
    "who",
    "whole",
    "why",
    "will",
    "with",
    "without",
    "word",
    "words",
    "work",
    "world",
    "would",
    "year",
    "years",
    "yes",
    "yet",
    "you",
    "your",
    "yours",
];

/// Longest run of output words that may be merged into one vocabulary term.
const MAX_WINDOW: usize = 3;
/// Words shorter than this are never phonetically corrected.
const MIN_PHONETIC_CHARS: usize = 3;

/// Apply vocabulary corrections to `text`. `vocabulary` order is priority
/// order (earlier terms win ties). Returns the text unchanged when the
/// vocabulary or text is empty.
pub fn correct(text: &str, vocabulary: &[String]) -> String {
    correct_counted(text, vocabulary).0
}

/// Like [`correct`], also returning how many corrections were applied so
/// callers can log a count without logging transcript content.
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
        match longest_match(&tokens, i, &lookup) {
            Some((consumed, term)) => {
                out.push_str(term);
                out.push_str(tokens[i + consumed - 1].suffix);
                corrections += 1;
                i += consumed;
            }
            None => {
                out.push_str(tok.core);
                out.push_str(tok.suffix);
                i += 1;
            }
        }
    }
    out.push_str(trailing_ws);
    (out, corrections)
}

/// One vocabulary term: its original text and its spoken form lowered and
/// joined without separators ("UserController" -> "usercontroller").
struct Term<'a> {
    text: &'a str,
    joined: String,
}

/// Per-call lookup over the vocabulary. Built fresh every call: a few
/// thousand identifiers hash in microseconds, and the merged vocabulary can
/// change between sessions, so caching across calls isn't worth the staleness
/// risk.
struct Lookup<'a> {
    terms: Vec<Term<'a>>,
    /// Case-sensitive term texts; matching output words are frozen.
    exact: HashSet<&'a str>,
    /// Lowercased term text -> term index (first entry wins) for casing repair.
    casing: HashMap<String, usize>,
    /// Concatenated phonetic key -> candidate term indices, so per-word work
    /// stays proportional to the bucket, not the whole vocabulary.
    buckets: HashMap<String, Vec<usize>>,
}

impl<'a> Lookup<'a> {
    fn build(vocabulary: &'a [String]) -> Self {
        let mut lookup = Self {
            terms: Vec::with_capacity(vocabulary.len()),
            exact: HashSet::new(),
            casing: HashMap::new(),
            buckets: HashMap::new(),
        };
        for raw in vocabulary {
            let text = raw.trim();
            if text.is_empty() {
                continue;
            }
            let words = spoken_words(text);
            let key: String = words.iter().map(|w| phonetic_key(w)).collect();
            let idx = lookup.terms.len();
            lookup.terms.push(Term {
                text,
                joined: words.concat(),
            });
            lookup.exact.insert(text);
            lookup.casing.entry(text.to_lowercase()).or_insert(idx);
            if !key.is_empty() {
                lookup.buckets.entry(key).or_default().push(idx);
            }
        }
        lookup
    }
}

/// Best correction starting at token `i`: the longest matching window wins,
/// then a single-token casing or phonetic repair. Returns (tokens consumed,
/// replacement term).
fn longest_match<'a>(tokens: &[Token], i: usize, lookup: &Lookup<'a>) -> Option<(usize, &'a str)> {
    // Exact case-sensitive vocabulary hits are frozen: never rewritten or
    // merged into a window. This is also what makes the pass idempotent.
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
    match_single(&tokens[i], lookup).map(|term| (1, term))
}

/// Match a run of output words against a term spoken as several words
/// ("git hub" -> "GitHub"). Interior punctuation breaks the window, and
/// every word must individually pass the phonetic gates.
fn match_window<'a>(window: &[Token], lookup: &Lookup<'a>) -> Option<&'a str> {
    let last = window.len() - 1;
    let mut key = String::new();
    for (j, tok) in window.iter().enumerate() {
        if (j > 0 && !tok.prefix.is_empty()) || (j < last && !tok.suffix.is_empty()) {
            return None;
        }
        if !phonetic_eligible(tok.core, lookup) {
            return None;
        }
        key.push_str(&phonetic_key(tok.core));
    }
    if key.is_empty() {
        return None;
    }
    let joined = window
        .iter()
        .flat_map(|t| t.core.chars())
        .collect::<String>()
        .to_lowercase();
    let span = span_text(window);
    best_candidate(&key, &joined, &span, lookup)
}

/// Single-token repair: casing first (aggressive), then guarded phonetics.
fn match_single<'a>(tok: &Token, lookup: &Lookup<'a>) -> Option<&'a str> {
    if tok.core.is_empty() {
        return None;
    }
    let lc = tok.core.to_lowercase();
    if let Some(&idx) = lookup.casing.get(&lc) {
        // Case-insensitive hit with different casing (exact matches were
        // frozen earlier): rewrite to the vocabulary casing.
        return Some(lookup.terms[idx].text);
    }
    if !phonetic_eligible(tok.core, lookup) {
        return None;
    }
    let key = phonetic_key(tok.core);
    if key.is_empty() {
        return None;
    }
    best_candidate(&key, &lc, tok.core, lookup)
}

/// Whether a word may be phonetically corrected: long enough, not a common
/// English word, and not itself an exact vocabulary term.
fn phonetic_eligible(core: &str, lookup: &Lookup) -> bool {
    if core.chars().count() < MIN_PHONETIC_CHARS || lookup.exact.contains(core) {
        return false;
    }
    STOPLIST
        .binary_search(&core.to_lowercase().as_str())
        .is_err()
}

/// Closest candidate in the phonetic bucket for `key` that passes the
/// distance guard; ties break toward the earlier vocabulary entry (user
/// terms come first in the merged list). `span` is the original text of the
/// window so a verbatim term is never counted as a correction.
fn best_candidate<'a>(key: &str, joined: &str, span: &str, lookup: &Lookup<'a>) -> Option<&'a str> {
    let word_len = joined.chars().count();
    let limit = distance_threshold(word_len);
    let mut best: Option<(usize, &'a str)> = None;
    for &idx in lookup.buckets.get(key)? {
        let term = &lookup.terms[idx];
        if term.text == span || term.joined.chars().count().abs_diff(word_len) > limit {
            continue;
        }
        let dist = levenshtein(joined, &term.joined);
        if dist <= limit && best.is_none_or(|(d, _)| dist < d) {
            best = Some((dist, term.text));
        }
    }
    best.map(|(_, text)| text)
}

/// Edit-distance guard scaled to word length (chars): short words allow one
/// edit, medium two, long identifiers three.
fn distance_threshold(chars: usize) -> usize {
    match chars {
        0..=5 => 1,
        6..=10 => 2,
        _ => 3,
    }
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

/// Refined Soundex key (see module doc): consonant classes with the first
/// letter coded by class, vowels separating runs of equal codes, `h`/`w`
/// transparent, variable length. Digits and non-ASCII alphanumerics code as
/// themselves; the rare class-digit/literal-digit bucket collision is
/// resolved by the distance guard. Vowel-only words yield an empty key and
/// never match phonetically.
fn phonetic_key(word: &str) -> String {
    let mut key = String::new();
    let mut last: Option<char> = None;
    for ch in word.chars().flat_map(char::to_lowercase) {
        let code = match ch {
            'b' | 'f' | 'p' | 'v' => '1',
            'c' | 'g' | 'j' | 'k' | 'q' | 's' | 'x' | 'z' => '2',
            'd' | 't' => '3',
            'l' => '4',
            'm' | 'n' => '5',
            'r' => '6',
            'a' | 'e' | 'i' | 'o' | 'u' | 'y' => {
                last = None;
                continue;
            }
            'h' | 'w' => continue,
            _ if ch.is_alphanumeric() => ch,
            _ => {
                last = None;
                continue;
            }
        };
        if last != Some(code) {
            key.push(code);
        }
        last = Some(code);
    }
    key
}

/// Standard two-row Levenshtein over chars.
fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() {
        return b.len();
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr = vec![0usize; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        curr[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            curr[j + 1] = (prev[j] + cost).min(prev[j + 1] + 1).min(curr[j] + 1);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vocab(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn stoplist_is_sorted_and_deduped() {
        assert!(
            STOPLIST.windows(2).all(|w| w[0] < w[1]),
            "STOPLIST must stay alphabetized for binary_search"
        );
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
    fn stoplist_word_still_casing_repairs_when_in_vocab() {
        // "will" is stoplisted (no phonetic repair) but a genuine vocab entry
        // still gets its casing.
        let v = vocab(&["Will"]);
        assert_eq!(correct("i asked will to help", &v), "i asked Will to help");
    }

    #[test]
    fn phonetic_single_word_accepted() {
        let v = vocab(&["Kubernetes", "Postgres"]);
        // Same key (216532), distance 1 <= 2 for a 10-char word.
        assert_eq!(
            correct("we deployed kubernetis", &v),
            "we deployed Kubernetes"
        );
        // Same key (123262: trailing s merges), distance 1.
        assert_eq!(
            correct("dump it from postgrez", &v),
            "dump it from Postgres"
        );
    }

    #[test]
    fn phonetic_rejected_too_short_word() {
        // "ct" would match "Cat" (key 23, distance 1) but is under 3 chars.
        let v = vocab(&["Cat"]);
        assert_eq!(correct("the ct scanner", &v), "the ct scanner");
    }

    #[test]
    fn phonetic_rejected_stoplist_word() {
        // "would" and "Wold" share key 43 at distance 1, but "would" is a
        // common English word and must never be phonetically rewritten.
        let v = vocab(&["Wold"]);
        assert_eq!(correct("i would go", &v), "i would go");
    }

    #[test]
    fn phonetic_rejected_distance_too_far() {
        // Same key as Kubernetes but 4 edits away; limit for 11 chars is 3.
        let v = vocab(&["Kubernetes"]);
        assert_eq!(correct("try coopernetes now", &v), "try coopernetes now");
    }

    #[test]
    fn phonetic_rejected_key_mismatch() {
        // "ruse" is one edit from "Rust" but keys differ (62 vs 623): both
        // conditions are required.
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
            "we met Müller at the Kubernetes talk"
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
            "i pushed the UserController to GitHub and deployed PostgreSQL on Kubernetes."
        );
        assert_eq!(count, 4);
    }

    #[test]
    fn no_corrections_counts_zero() {
        let v = vocab(&["GitHub"]);
        let (out, count) = correct_counted("nothing relevant here", &v);
        assert_eq!(out, "nothing relevant here");
        assert_eq!(count, 0);
    }
}
