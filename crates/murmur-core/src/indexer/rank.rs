//! Scoring and budget-capped selection of extracted identifiers.
//!
//! The hard part of the feature: Whisper's prompt window is small (~224
//! tokens, shared with a style hint and rolling context), so the vocabulary
//! must be a ranked, capped subset, not the whole symbol table. Stuffing in
//! plain English words wastes that budget and degrades general transcription.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

/// An identifier with its accumulated score, returned ranked for inspection.
#[derive(Debug, Clone)]
pub struct RankedSymbol {
    pub text: String,
    pub score: f64,
    pub freq: u32,
}

/// Identifiers shorter than this are noise, not worth biasing.
const MIN_LEN: usize = 3;
/// Extra weight for identifiers the STT engine is most likely to mangle:
/// those with interior caps, underscores, or digits.
const DISTINCTIVE_FACTOR: f64 = 1.5;
/// Recency half-life: a file edited this many days ago contributes about half
/// the recency bonus of one edited today.
const HALF_LIFE_DAYS: f64 = 7.0;

/// Per-file recency multiplier in `[1.0, 2.0]`: a just-edited file counts ~2x,
/// an ancient one ~1x. mtime is the only "what am I working on" signal we have.
pub fn recency_weight(age_days: f64) -> f64 {
    1.0 + (-age_days.max(0.0) / HALF_LIFE_DAYS).exp()
}

/// Accumulates identifier statistics across files, then selects the
/// budget-capped top symbols by TF-IDF.
///
/// Pure frequency surfaces ubiquitous tokens (`the`, `std`, `Result`) that the
/// STT engine already handles, not the distinctive project symbols we want. So
/// scoring weights each term by **inverse document frequency** — a term in
/// nearly every file scores ~0 — and uses **sublinear term frequency** so a few
/// very common words can't dominate. That floats concentrated, distinctive
/// identifiers (`calculateTotalRevenue`) to the top.
#[derive(Default)]
pub struct SymbolAccumulator {
    /// Keyed by lowercased identifier for case-insensitive dedup.
    entries: HashMap<String, Entry>,
    /// Files folded in so far (the document count for IDF).
    total_files: usize,
}

struct Entry {
    display: String,
    /// Recency- and distinctiveness-weighted occurrence sum (term frequency).
    tf: f64,
    /// Raw occurrence count, for display.
    freq: u32,
    /// Distinct files the term appeared in (document frequency).
    doc_count: u32,
}

impl SymbolAccumulator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Fold one file's identifiers in. Each occurrence adds to term frequency
    /// (weighted by recency and distinctiveness); the term's document count
    /// rises at most once per file.
    pub fn add_file<'a, I: Iterator<Item = &'a str>>(
        &mut self,
        identifiers: I,
        recency_weight: f64,
    ) {
        self.total_files += 1;
        let mut seen_in_file: HashSet<String> = HashSet::new();
        for ident in identifiers {
            if !is_candidate(ident) {
                continue;
            }
            let key = ident.to_lowercase();
            let first_here = seen_in_file.insert(key.clone());
            let entry = self.entries.entry(key).or_insert_with(|| Entry {
                display: ident.to_string(),
                tf: 0.0,
                freq: 0,
                doc_count: 0,
            });
            entry.tf += recency_weight * distinctiveness(ident);
            entry.freq += 1;
            if first_here {
                entry.doc_count += 1;
            }
        }
    }

    /// Rank by TF-IDF and return entries until the count or joined-char budget
    /// is hit, whichever comes first.
    pub fn select(self, max_symbols: usize, max_chars: usize) -> Vec<RankedSymbol> {
        let n = self.total_files.max(1) as f64;
        // With one document there is no IDF signal, so fall back to TF only.
        let single_doc = self.total_files <= 1;
        let mut ranked: Vec<RankedSymbol> = self
            .entries
            .into_values()
            .map(|e| {
                let idf = if single_doc {
                    1.0
                } else {
                    (n / e.doc_count.max(1) as f64).ln().max(0.0)
                };
                RankedSymbol {
                    text: e.display,
                    score: (1.0 + e.tf.ln()) * idf,
                    freq: e.freq,
                }
            })
            // Drop ubiquitous terms (idf ~ 0): they bias toward generic output.
            .filter(|s| s.score > 0.0)
            .collect();
        // Score desc; ties broken by text for deterministic output.
        ranked.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.text.cmp(&b.text))
        });

        let mut out = Vec::new();
        let mut chars = 0usize;
        for sym in ranked {
            if out.len() >= max_symbols {
                break;
            }
            // Account for the ", " joiner between entries.
            let added = sym.text.chars().count() + if out.is_empty() { 0 } else { 2 };
            if chars + added > max_chars {
                break;
            }
            chars += added;
            out.push(sym);
        }
        out
    }
}

/// Whether an identifier is worth keeping: long enough, not a language keyword,
/// and not a bare common-English word (those only when all-lowercase, since a
/// distinctive casing like `Config` or `MAX` signals a real symbol).
fn is_candidate(ident: &str) -> bool {
    if ident.chars().count() < MIN_LEN {
        return false;
    }
    if KEYWORDS.contains(ident) {
        return false;
    }
    if is_all_lowercase(ident) && COMMON_WORDS.contains(ident) {
        return false;
    }
    true
}

fn is_all_lowercase(ident: &str) -> bool {
    !ident.chars().any(|c| c.is_ascii_uppercase())
}

/// `DISTINCTIVE_FACTOR` for identifiers with interior caps, underscores, or
/// digits (the strings STT mangles), otherwise `1.0`.
fn distinctiveness(ident: &str) -> f64 {
    let interior_upper = ident.chars().skip(1).any(|c| c.is_ascii_uppercase());
    let has_underscore = ident.contains('_');
    let has_digit = ident.chars().any(|c| c.is_ascii_digit());
    if interior_upper || has_underscore || has_digit {
        DISTINCTIVE_FACTOR
    } else {
        1.0
    }
}

/// Language keywords across the supported file types; never useful as hot words.
static KEYWORDS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    [
        // Rust
        "let",
        "mut",
        "pub",
        "mod",
        "use",
        "for",
        "while",
        "loop",
        "match",
        "impl",
        "trait",
        "struct",
        "enum",
        "const",
        "static",
        "return",
        "async",
        "await",
        "move",
        "ref",
        "dyn",
        "where",
        "self",
        "super",
        "crate",
        "type",
        "unsafe",
        "extern",
        "fn",
        "as",
        "box",
        // JS / TS
        "function",
        "var",
        "class",
        "export",
        "import",
        "default",
        "extends",
        "interface",
        "typeof",
        "instanceof",
        "void",
        "null",
        "undefined",
        "new",
        "this",
        "yield",
        // Python
        "def",
        "from",
        "lambda",
        "pass",
        "with",
        "global",
        "nonlocal",
        "elif",
        "else",
        "try",
        "except",
        "finally",
        "raise",
        "none",
        "true",
        "false",
        "and",
        "not",
        "is",
        "in",
        "or",
        // Go
        "func",
        "package",
        "map",
        "chan",
        "range",
        "defer",
        "select",
        "case",
        "fallthrough",
        "goto",
        "go",
        // Java
        "public",
        "private",
        "protected",
        "final",
        "implements",
        "throws",
        "throw",
        "catch",
        // shared control
        "if",
        "then",
        "end",
        "do",
    ]
    .into_iter()
    .collect()
});

/// Bare lowercase English/programming words that waste prompt budget and bias
/// the decoder toward generic output. Dropped only when all-lowercase.
static COMMON_WORDS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    [
        "count", "user", "data", "value", "name", "type", "result", "index", "error", "item",
        "list", "text", "line", "file", "path", "time", "date", "size", "code", "page", "view",
        "mode", "state", "input", "output", "number", "string", "object", "array", "field",
        "label", "title", "body", "head", "main", "test", "temp", "total", "status", "message",
        "content", "config", "option", "params", "args", "key", "val", "obj", "str", "num", "len",
        "idx", "tmp", "ctx", "req", "res", "err", "msg", "init",
    ]
    .into_iter()
    .collect()
});

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(ranked: &[RankedSymbol]) -> Vec<String> {
        ranked.iter().map(|s| s.text.clone()).collect()
    }

    #[test]
    fn recency_weight_decays_monotonically() {
        let today = recency_weight(0.0);
        let week = recency_weight(7.0);
        let old = recency_weight(365.0);
        assert!((today - 2.0).abs() < 1e-9, "fresh file ~2x");
        assert!(today > week && week > old);
        assert!((old - 1.0).abs() < 0.05, "old file approaches 1x");
        assert_eq!(recency_weight(-5.0), today, "future mtime clamped to now");
    }

    #[test]
    fn distinctive_outranks_plain_at_equal_frequency() {
        let mut acc = SymbolAccumulator::new();
        // Same occurrence count and recency; "renderWidget" is distinctive
        // (interior caps), "rendering" is plain lowercase.
        acc.add_file(["renderWidget", "rendering"].into_iter(), 1.0);
        let ranked = acc.select(10, 1000);
        let widget = ranked
            .iter()
            .position(|s| s.text == "renderWidget")
            .unwrap();
        let plain = ranked.iter().position(|s| s.text == "rendering").unwrap();
        assert!(widget < plain, "distinctive identifier should rank higher");
    }

    #[test]
    fn drops_keywords_short_and_common_words() {
        let mut acc = SymbolAccumulator::new();
        acc.add_file(
            [
                "return",
                "fn",
                "ab",
                "count",
                "data",
                "Config",
                "renderWidget",
            ]
            .into_iter(),
            1.0,
        );
        let got = texts(&acc.select(10, 1000));
        assert!(got.contains(&"renderWidget".to_string()));
        assert!(
            got.contains(&"Config".to_string()),
            "distinctive casing kept"
        );
        for dropped in ["return", "fn", "ab", "count", "data"] {
            assert!(!got.contains(&dropped.to_string()), "{dropped} should drop");
        }
    }

    #[test]
    fn idf_suppresses_ubiquitous_terms() {
        let mut acc = SymbolAccumulator::new();
        // "common" appears in every file; "RareSymbol" only in one.
        for _ in 0..20 {
            acc.add_file(["common", "common", "filler"].into_iter(), 1.0);
        }
        acc.add_file(["RareSymbol", "RareSymbol", "RareSymbol"].into_iter(), 1.0);
        let ranked = acc.select(10, 10_000);
        let rare = ranked
            .iter()
            .position(|s| s.text == "RareSymbol")
            .expect("rare distinctive symbol should appear");
        // "common" is in nearly every file, so its IDF ~ 0; it ranks far below
        // the concentrated symbol, or is dropped entirely.
        if let Some(common) = ranked.iter().position(|s| s.text == "common") {
            assert!(
                rare < common,
                "ubiquitous term must rank below the rare one"
            );
        }
    }

    #[test]
    fn caps_by_symbol_count() {
        let mut acc = SymbolAccumulator::new();
        for i in 0..50 {
            acc.add_file([format!("symbolName{i}")].iter().map(|s| s.as_str()), 1.0);
        }
        assert_eq!(acc.select(10, 10_000).len(), 10);
    }

    #[test]
    fn caps_by_char_budget() {
        let mut acc = SymbolAccumulator::new();
        for i in 0..50 {
            acc.add_file([format!("ident{i:03}")].iter().map(|s| s.as_str()), 1.0);
        }
        // Each entry is 8 chars + 2 joiner; budget 30 fits 3 (8 + 10 + 10 = 28).
        let ranked = acc.select(100, 30);
        let joined_len: usize = ranked.iter().map(|s| s.text.chars().count()).sum::<usize>()
            + ranked.len().saturating_sub(1) * 2;
        assert!(
            joined_len <= 30,
            "joined length {joined_len} exceeds budget"
        );
        assert!(ranked.len() < 50);
    }
}
