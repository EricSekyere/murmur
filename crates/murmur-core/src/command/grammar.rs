//! Tier 1 deterministic template grammar: a hand-rolled hassil/Talon-style
//! matcher, no regex.
//!
//! Pattern syntax: literal words, alternatives `(open|launch)`, optional
//! groups `[the]`, free-text slots `{name}`, choice slots
//! `{app:browser|editor}`, and inclusive integer ranges `{level:0..100}`.
//! Matching is case-insensitive and whitespace-normalized, and a phrase
//! matches only when the entire normalized input is consumed, mirroring the
//! whole-phrase rule in [`crate::voice_commands`].

use std::collections::HashMap;

use thiserror::Error;

use crate::voice_commands::normalize;

/// A malformed pattern string, reported at compile time so a bad pattern can
/// never panic or silently misfire at match time.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum GrammarError {
    #[error("pattern contains no tokens")]
    EmptyPattern,

    #[error("unbalanced '{0}' in pattern")]
    Unbalanced(char),

    #[error("nested group in '{0}' (groups cannot contain other groups or slots)")]
    NestedGroup(String),

    #[error("empty alternative in '{0}'")]
    EmptyAlternative(String),

    #[error("invalid slot '{0}'")]
    InvalidSlot(String),

    #[error("invalid integer range '{spec}' in slot '{name}'")]
    InvalidRange { name: String, spec: String },

    #[error("duplicate slot name '{0}'")]
    DuplicateSlot(String),
}

/// A value captured by a slot during a successful match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlotValue {
    /// Free text from a `{name}` slot: one or more words, space-joined.
    Text(String),
    /// An in-range integer from a `{name:lo..hi}` slot.
    Number(i64),
    /// The chosen alternative from a `{name:a|b|c}` slot, normalized.
    Choice(String),
}

/// A command template as authored: an identifier plus its pattern text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandPattern {
    pub id: String,
    pub pattern: String,
}

impl CommandPattern {
    pub fn new(id: impl Into<String>, pattern: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            pattern: pattern.into(),
        }
    }

    /// Parse the pattern text into matchable token form.
    ///
    /// # Errors
    /// Returns [`GrammarError`] when the pattern is malformed (unbalanced
    /// delimiters, empty alternatives, bad ranges, duplicate slot names).
    pub fn compile(self) -> Result<CompiledPattern, GrammarError> {
        CompiledPattern::new(self)
    }
}

/// A [`CommandPattern`] parsed once into internal tokens, ready to match.
#[derive(Debug, Clone)]
pub struct CompiledPattern {
    source: CommandPattern,
    tokens: Vec<Token>,
}

impl CompiledPattern {
    /// Parse `source.pattern` into tokens; see [`CommandPattern::compile`].
    pub fn new(source: CommandPattern) -> Result<Self, GrammarError> {
        let tokens = parse_pattern(&source.pattern)?;
        Ok(Self { source, tokens })
    }

    pub fn id(&self) -> &str {
        &self.source.id
    }

    pub fn pattern(&self) -> &str {
        &self.source.pattern
    }
}

/// A successful match: which command fired and the captured slot values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    pub command_id: String,
    pub slots: HashMap<String, SlotValue>,
}

/// An ordered set of compiled patterns. Insertion order is priority order:
/// [`Grammar::match_phrase`] returns the first pattern that matches, so
/// register specific commands before catch-all ones.
#[derive(Debug, Clone, Default)]
pub struct Grammar {
    patterns: Vec<CompiledPattern>,
}

impl Grammar {
    pub fn new() -> Self {
        Self::default()
    }

    /// Compile and register a pattern.
    ///
    /// # Errors
    /// Returns [`GrammarError`] when the pattern is malformed; the grammar is
    /// left unchanged.
    pub fn add(
        &mut self,
        id: impl Into<String>,
        pattern: impl Into<String>,
    ) -> Result<(), GrammarError> {
        self.patterns
            .push(CommandPattern::new(id, pattern).compile()?);
        Ok(())
    }

    /// Match a spoken phrase against the registered patterns, first hit wins.
    /// The whole normalized input must be consumed; trailing words reject the
    /// match, so a command can never fire from inside a longer sentence.
    pub fn match_phrase(&self, input: &str) -> Option<Match> {
        let normalized = normalize(input);
        let words: Vec<&str> = normalized.split_whitespace().collect();
        if words.is_empty() {
            return None;
        }
        for compiled in &self.patterns {
            let mut slots = HashMap::new();
            if match_tokens(&compiled.tokens, &words, &mut slots) {
                // Privacy: log only the command id, never the phrase itself.
                tracing::debug!(command_id = %compiled.id(), "grammar matched phrase");
                return Some(Match {
                    command_id: compiled.source.id.clone(),
                    slots,
                });
            }
        }
        None
    }
}

/// One element of a compiled pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    /// A literal word, lowercased at compile time.
    Word(String),
    /// `(a|b)` or `[a|b]`: word-sequence branches, optionally skippable.
    Group {
        branches: Vec<Vec<String>>,
        optional: bool,
    },
    Slot {
        name: String,
        kind: SlotKind,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SlotKind {
    Text,
    /// Each option is a pre-split lowercased word sequence.
    Choice(Vec<Vec<String>>),
    /// Inclusive bounds.
    Range {
        min: i64,
        max: i64,
    },
}

const OPENERS: [char; 3] = ['(', '[', '{'];
const CLOSERS: [char; 3] = [')', ']', '}'];

fn parse_pattern(pattern: &str) -> Result<Vec<Token>, GrammarError> {
    let mut tokens = Vec::new();
    let mut slot_names: Vec<String> = Vec::new();
    let mut rest = pattern.trim_start();
    while let Some(first) = rest.chars().next() {
        let after = &rest[first.len_utf8()..];
        let tail = match first {
            '(' | '[' => {
                let (body, tail) = group_body(after, first)?;
                tokens.push(parse_group(body, first == '[')?);
                tail
            }
            '{' => {
                let (body, tail) = group_body(after, first)?;
                tokens.push(parse_slot(body, &mut slot_names)?);
                tail
            }
            c if CLOSERS.contains(&c) => return Err(GrammarError::Unbalanced(c)),
            _ => {
                let (word, tail) = take_word(rest);
                tokens.push(word);
                tail
            }
        };
        rest = tail.trim_start();
    }
    if tokens.is_empty() {
        return Err(GrammarError::EmptyPattern);
    }
    Ok(tokens)
}

/// Split `s` into the body before the delimiter matching `open` and the tail
/// after it, rejecting any nested or mismatched delimiter inside.
fn group_body(s: &str, open: char) -> Result<(&str, &str), GrammarError> {
    let close = match open {
        '(' => ')',
        '[' => ']',
        _ => '}',
    };
    for (i, c) in s.char_indices() {
        if c == close {
            return Ok((&s[..i], &s[i + c.len_utf8()..]));
        }
        if OPENERS.contains(&c) {
            return Err(GrammarError::NestedGroup(s.trim().to_string()));
        }
        if CLOSERS.contains(&c) {
            return Err(GrammarError::Unbalanced(c));
        }
    }
    Err(GrammarError::Unbalanced(open))
}

/// Read a literal word: everything up to whitespace or a group delimiter.
fn take_word(s: &str) -> (Token, &str) {
    let end = s
        .find(|c: char| c.is_whitespace() || OPENERS.contains(&c) || CLOSERS.contains(&c))
        .unwrap_or(s.len());
    (Token::Word(s[..end].to_lowercase()), &s[end..])
}

/// Parse `a|b c|d` group body into word-sequence branches.
fn parse_group(body: &str, optional: bool) -> Result<Token, GrammarError> {
    let branches = split_alternatives(body)
        .ok_or_else(|| GrammarError::EmptyAlternative(body.trim().to_string()))?;
    Ok(Token::Group { branches, optional })
}

/// Split on `|` into lowercased word sequences; `None` if any branch is empty.
fn split_alternatives(body: &str) -> Option<Vec<Vec<String>>> {
    body.split('|')
        .map(|part| {
            let words: Vec<String> = part.split_whitespace().map(str::to_lowercase).collect();
            (!words.is_empty()).then_some(words)
        })
        .collect()
}

/// Parse a `{...}` slot body: `name`, `name:a|b`, or `name:lo..hi`.
fn parse_slot(body: &str, seen: &mut Vec<String>) -> Result<Token, GrammarError> {
    let (name, spec) = match body.split_once(':') {
        Some((n, s)) => (n.trim(), Some(s.trim())),
        None => (body.trim(), None),
    };
    if name.is_empty() || name.contains(char::is_whitespace) {
        return Err(GrammarError::InvalidSlot(body.trim().to_string()));
    }
    if seen.iter().any(|s| s == name) {
        return Err(GrammarError::DuplicateSlot(name.to_string()));
    }
    seen.push(name.to_string());
    let kind = match spec {
        None => SlotKind::Text,
        Some(s) if s.contains("..") => parse_range(name, s)?,
        Some(s) => SlotKind::Choice(
            split_alternatives(s)
                .ok_or_else(|| GrammarError::InvalidSlot(format!("{name}:{s}")))?,
        ),
    };
    Ok(Token::Slot {
        name: name.to_string(),
        kind,
    })
}

fn parse_range(name: &str, spec: &str) -> Result<SlotKind, GrammarError> {
    let err = || GrammarError::InvalidRange {
        name: name.to_string(),
        spec: spec.to_string(),
    };
    let (lo, hi) = spec.split_once("..").ok_or_else(err)?;
    let min: i64 = lo.trim().parse().map_err(|_| err())?;
    let max: i64 = hi.trim().parse().map_err(|_| err())?;
    if min > max {
        return Err(err());
    }
    Ok(SlotKind::Range { min, max })
}

/// Backtracking matcher: succeeds only when every token and every input word
/// is consumed.
fn match_tokens(tokens: &[Token], words: &[&str], slots: &mut HashMap<String, SlotValue>) -> bool {
    let Some((first, rest)) = tokens.split_first() else {
        return words.is_empty();
    };
    match first {
        Token::Word(expected) => words.split_first().is_some_and(|(head, tail)| {
            *head == expected.as_str() && match_tokens(rest, tail, slots)
        }),
        Token::Group { branches, optional } => match_group(branches, *optional, rest, words, slots),
        Token::Slot { name, kind } => match_slot(name, kind, rest, words, slots),
    }
}

fn match_group(
    branches: &[Vec<String>],
    optional: bool,
    rest: &[Token],
    words: &[&str],
    slots: &mut HashMap<String, SlotValue>,
) -> bool {
    let branch_hit = branches.iter().any(|branch| {
        strip_words(words, branch).is_some_and(|tail| match_tokens(rest, tail, slots))
    });
    branch_hit || (optional && match_tokens(rest, words, slots))
}

fn match_slot(
    name: &str,
    kind: &SlotKind,
    rest: &[Token],
    words: &[&str],
    slots: &mut HashMap<String, SlotValue>,
) -> bool {
    match kind {
        SlotKind::Text => (1..=words.len()).any(|take| {
            let value = SlotValue::Text(words[..take].join(" "));
            capture(name, value, rest, &words[take..], slots)
        }),
        SlotKind::Choice(options) => options.iter().any(|option| {
            strip_words(words, option).is_some_and(|tail| {
                capture(name, SlotValue::Choice(option.join(" ")), rest, tail, slots)
            })
        }),
        SlotKind::Range { min, max } => words.split_first().is_some_and(|(head, tail)| {
            head.parse::<i64>()
                .ok()
                .filter(|n| (*min..=*max).contains(n))
                .is_some_and(|n| capture(name, SlotValue::Number(n), rest, tail, slots))
        }),
    }
}

/// Record a slot value, try the rest of the pattern, and roll the value back
/// on failure so sibling branches see a clean slot map.
fn capture(
    name: &str,
    value: SlotValue,
    rest: &[Token],
    words: &[&str],
    slots: &mut HashMap<String, SlotValue>,
) -> bool {
    slots.insert(name.to_string(), value);
    let matched = match_tokens(rest, words, slots);
    if !matched {
        slots.remove(name);
    }
    matched
}

/// If `words` starts with the `prefix` word sequence, return the remainder.
fn strip_words<'a, 'w>(words: &'a [&'w str], prefix: &[String]) -> Option<&'a [&'w str]> {
    if words.len() < prefix.len() {
        return None;
    }
    let (head, tail) = words.split_at(prefix.len());
    head.iter()
        .zip(prefix)
        .all(|(w, p)| *w == p.as_str())
        .then_some(tail)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn built(patterns: &[(&str, &str)]) -> Grammar {
        let mut grammar = Grammar::new();
        for (id, pattern) in patterns {
            grammar.add(*id, *pattern).expect("pattern should compile");
        }
        grammar
    }

    fn matched(grammar: &Grammar, phrase: &str) -> Match {
        grammar
            .match_phrase(phrase)
            .unwrap_or_else(|| panic!("phrase {phrase:?} should match"))
    }

    #[test]
    fn literal_match_and_non_match() {
        let g = built(&[("open_browser", "open the browser")]);
        assert_eq!(matched(&g, "open the browser").command_id, "open_browser");
        assert!(g.match_phrase("close the browser").is_none());
        assert!(g.match_phrase("open browser").is_none());
    }

    #[test]
    fn case_whitespace_and_trailing_punctuation_tolerated() {
        let g = built(&[("open_browser", "open the browser")]);
        assert!(g.match_phrase("  Open   THE Browser. ").is_some());
        assert!(g.match_phrase("OPEN THE BROWSER?").is_some());
    }

    #[test]
    fn alternatives_match_each_branch_only() {
        let g = built(&[("open", "(open|launch|start) firefox")]);
        for phrase in ["open firefox", "launch firefox", "start firefox"] {
            assert_eq!(matched(&g, phrase).command_id, "open");
        }
        assert!(g.match_phrase("close firefox").is_none());
    }

    #[test]
    fn optional_group_present_and_absent() {
        let g = built(&[("open", "open [the] browser")]);
        assert!(g.match_phrase("open the browser").is_some());
        assert!(g.match_phrase("open browser").is_some());
        assert!(g.match_phrase("open the the browser").is_none());
    }

    #[test]
    fn free_text_slot_captures_multi_word_text() {
        let g = built(&[("open_file", "open file {path}")]);
        let m = matched(&g, "open file src slash main dot rs");
        assert_eq!(
            m.slots.get("path"),
            Some(&SlotValue::Text("src slash main dot rs".into()))
        );
        // A text slot needs at least one word.
        assert!(g.match_phrase("open file").is_none());
    }

    #[test]
    fn text_slot_in_the_middle_backtracks() {
        let g = built(&[("remind", "remind me to {task} at {hour:0..23}")]);
        let m = matched(&g, "remind me to feed the cat at 9");
        assert_eq!(
            m.slots.get("task"),
            Some(&SlotValue::Text("feed the cat".into()))
        );
        assert_eq!(m.slots.get("hour"), Some(&SlotValue::Number(9)));
    }

    #[test]
    fn choice_slot_returns_chosen_variant() {
        let g = built(&[("focus", "switch to [the] {app:browser|editor|terminal}")]);
        let m = matched(&g, "switch to the editor");
        assert_eq!(
            m.slots.get("app"),
            Some(&SlotValue::Choice("editor".into()))
        );
        assert!(g.match_phrase("switch to the calculator").is_none());
    }

    #[test]
    fn choice_slot_supports_multi_word_options() {
        let g = built(&[("open", "open {app:visual studio|terminal}")]);
        let m = matched(&g, "open Visual Studio");
        assert_eq!(
            m.slots.get("app"),
            Some(&SlotValue::Choice("visual studio".into()))
        );
    }

    #[test]
    fn integer_range_slot_accepts_only_in_range_numbers() {
        let g = built(&[("volume", "set volume to {level:0..100}")]);
        assert_eq!(
            matched(&g, "set volume to 40").slots.get("level"),
            Some(&SlotValue::Number(40))
        );
        assert_eq!(
            matched(&g, "set volume to 0").slots.get("level"),
            Some(&SlotValue::Number(0))
        );
        assert_eq!(
            matched(&g, "set volume to 100").slots.get("level"),
            Some(&SlotValue::Number(100))
        );
        assert!(g.match_phrase("set volume to 101").is_none());
        assert!(g.match_phrase("set volume to -1").is_none());
        assert!(g.match_phrase("set volume to loud").is_none());
    }

    #[test]
    fn whole_input_must_be_consumed() {
        let g = built(&[("open", "open the browser")]);
        assert!(g.match_phrase("open the browser now").is_none());
        assert!(g.match_phrase("please open the browser").is_none());
    }

    #[test]
    fn first_added_pattern_wins() {
        let g = built(&[
            ("specific", "open the browser"),
            ("generic", "open the {thing}"),
        ]);
        assert_eq!(matched(&g, "open the browser").command_id, "specific");
        assert_eq!(matched(&g, "open the terminal").command_id, "generic");
    }

    #[test]
    fn empty_input_never_matches() {
        let g = built(&[("open", "open [the] browser")]);
        for phrase in ["", "   ", ".", "?!"] {
            assert!(
                g.match_phrase(phrase).is_none(),
                "{phrase:?} must not match"
            );
        }
    }

    #[test]
    fn combined_pattern_with_all_features() {
        let g = built(&[(
            "volume",
            "(set|change) [the] {app:browser|music player} volume to {level:0..100}",
        )]);
        let m = matched(&g, "Change the music player volume to 85.");
        assert_eq!(
            m.slots.get("app"),
            Some(&SlotValue::Choice("music player".into()))
        );
        assert_eq!(m.slots.get("level"), Some(&SlotValue::Number(85)));
        assert_eq!(m.slots.len(), 2);
    }

    #[test]
    fn malformed_patterns_return_errors_not_panics() {
        let mut g = Grammar::new();
        assert!(matches!(
            g.add("a", "(open|launch the browser"),
            Err(GrammarError::Unbalanced('('))
        ));
        assert!(matches!(
            g.add("b", "(a||b)"),
            Err(GrammarError::EmptyAlternative(_))
        ));
        assert!(matches!(
            g.add("c", "open [the browser"),
            Err(GrammarError::Unbalanced('['))
        ));
        assert!(matches!(
            g.add("d", "set {x:5..1}"),
            Err(GrammarError::InvalidRange { .. })
        ));
        assert!(matches!(
            g.add("e", "set {x:a..b}"),
            Err(GrammarError::InvalidRange { .. })
        ));
        assert!(matches!(
            g.add("f", "set {} volume"),
            Err(GrammarError::InvalidSlot(_))
        ));
        assert!(matches!(g.add("g", "   "), Err(GrammarError::EmptyPattern)));
        assert!(matches!(
            g.add("h", "open ((a|b))"),
            Err(GrammarError::NestedGroup(_))
        ));
        assert!(matches!(
            g.add("i", "close ) it"),
            Err(GrammarError::Unbalanced(')'))
        ));
        assert!(matches!(
            g.add("j", "{x} and {x}"),
            Err(GrammarError::DuplicateSlot(_))
        ));
        // Failed adds leave the grammar unchanged.
        assert!(g.match_phrase("open the browser").is_none());
    }

    #[test]
    fn compiled_pattern_exposes_source() {
        let compiled = CommandPattern::new("open", "open [the] browser")
            .compile()
            .expect("compiles");
        assert_eq!(compiled.id(), "open");
        assert_eq!(compiled.pattern(), "open [the] browser");
    }
}
