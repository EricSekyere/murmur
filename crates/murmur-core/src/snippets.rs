//! Parameterized voice snippets: triggers with `{name}` slots whose spoken
//! capture is substituted into the expansion, optionally recased
//! (`{name:pascal}`).
//!
//! Static snippets (no `{` in the trigger) keep the exact semantics of
//! [`crate::voice_commands::match_snippet`], byte for byte: their expansions
//! are delivered verbatim, with no escape interpretation. Only a trigger that
//! contains a slot opts its expansion into `\n` / `\t` interpretation; this
//! asymmetry is deliberate so already-shipped configs never change behavior.
//!
//! A bad snippet is inert, never destructive, never fatal: every problem is
//! caught at compile time, the snippet is skipped (or, for an unknown
//! transform only, kept with the capture inserted verbatim), and a warning is
//! returned for the UI to surface.

use crate::command::{Grammar, SlotValue};
use crate::stt::postprocess::{apply_case, capitalize};
use crate::voice_commands::{self, Snippet, normalize};

/// A trigger may capture at most this many text slots; more would make the
/// backtracking matcher and the spoken phrase itself unwieldy.
pub const MAX_TRIGGER_SLOTS: usize = 3;

/// Phrases longer than this skip parameterized matching, bounding the
/// backtracking search. Static snippets are unaffected.
pub const MAX_PARAM_PHRASE_WORDS: usize = 32;

/// Cap on the expanded output, applied AFTER substitution so a long capture
/// repeated across several slots cannot balloon the delivery. Also the
/// per-snippet expansion cap that settings clamping enforces on load.
pub const MAX_EXPANSION_CHARS: usize = 2_000;

/// Casing applied to a slot capture in the expansion, as `{name:transform}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transform {
    Pascal,
    Camel,
    Snake,
    Kebab,
    /// SCREAMING_SNAKE, matching the spoken "upper" casing formatter.
    Upper,
    Lower,
    Title,
}

impl Transform {
    fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "pascal" => Self::Pascal,
            "camel" => Self::Camel,
            "snake" => Self::Snake,
            "kebab" => Self::Kebab,
            "upper" => Self::Upper,
            "lower" => Self::Lower,
            "title" => Self::Title,
            _ => return None,
        })
    }

    /// Recase a captured value. Must never panic: the capture can contain
    /// digits, symbols (`=>`), or unicode when developer-mode postprocessing
    /// ran first.
    pub fn apply(&self, input: &str) -> String {
        let words: Vec<String> = input.split_whitespace().map(str::to_string).collect();
        match self {
            Self::Pascal => apply_case("pascal", &words),
            Self::Camel => apply_case("camel", &words),
            Self::Snake => apply_case("snake", &words),
            Self::Kebab => apply_case("kebab", &words),
            Self::Upper => apply_case("upper", &words),
            Self::Lower => input.to_lowercase(),
            Self::Title => words
                .iter()
                .map(|w| capitalize(w))
                .collect::<Vec<_>>()
                .join(" "),
        }
    }
}

/// One piece of a parsed expansion template.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Segment {
    Literal(String),
    Slot {
        name: String,
        transform: Option<Transform>,
    },
}

/// The user's snippets compiled for matching: statics keep the exact-scan
/// path, parameterized triggers become grammar patterns whose synthetic id is
/// their template index.
#[derive(Debug, Clone, Default)]
pub struct CompiledSnippets {
    statics: Vec<Snippet>,
    grammar: Grammar,
    templates: Vec<Vec<Segment>>,
}

impl CompiledSnippets {
    /// True when the phrase would expand; used by the "literally" escape.
    pub fn matches(&self, phrase: &str) -> bool {
        self.expand(phrase).is_some()
    }

    /// Expand a whole spoken phrase: exact static match first (unchanged
    /// semantics), then parameterized patterns in config order, first hit
    /// wins. `None` means the phrase is ordinary dictation.
    pub fn expand(&self, phrase: &str) -> Option<String> {
        if let Some(expansion) = voice_commands::match_snippet(phrase, &self.statics) {
            return Some(expansion.to_string());
        }
        if self.templates.is_empty() {
            return None;
        }
        let word_count = normalize(phrase).split_whitespace().count();
        if word_count == 0 || word_count > MAX_PARAM_PHRASE_WORDS {
            return None;
        }
        let matched = self.grammar.match_phrase(phrase)?;
        let index: usize = matched.command_id.parse().ok()?;
        let segments = self.templates.get(index)?;
        let mut out = String::new();
        for segment in segments {
            match segment {
                Segment::Literal(text) => out.push_str(text.as_str()),
                Segment::Slot { name, transform } => {
                    let value = slot_text(matched.slots.get(name.as_str()));
                    match transform {
                        Some(t) => out.push_str(&t.apply(&value)),
                        None => out.push_str(&value),
                    }
                }
            }
        }
        clamp_chars(&mut out, MAX_EXPANSION_CHARS);
        // Privacy: log only the snippet index, never the capture or phrase.
        tracing::debug!(snippet_index = index, "parameterized snippet expanded");
        Some(out)
    }
}

/// Compile the configured snippets, tolerating per-snippet failures: a bad
/// snippet is skipped with a warning and never fails startup. The returned
/// warnings include the existing shadowed/duplicate checks.
pub fn compile(snippets: &[Snippet]) -> (CompiledSnippets, Vec<String>) {
    let mut warnings = voice_commands::snippet_warnings(snippets);
    let mut compiled = CompiledSnippets::default();
    for snippet in snippets {
        if !snippet.trigger.contains('{') {
            compiled.statics.push(snippet.clone());
            continue;
        }
        if let Err(reason) = add_parameterized(&mut compiled, snippet, &mut warnings) {
            warnings.push(format!(
                "Snippet \"{}\" is disabled: {}.",
                snippet.trigger.trim(),
                reason
            ));
        }
    }
    (compiled, warnings)
}

/// Compile one brace-containing trigger and its expansion template; on error
/// the snippet is left out entirely.
fn add_parameterized(
    compiled: &mut CompiledSnippets,
    snippet: &Snippet,
    warnings: &mut Vec<String>,
) -> Result<(), String> {
    let slot_names = trigger_slot_names(&snippet.trigger)?;
    if slot_names.len() > MAX_TRIGGER_SLOTS {
        return Err(format!(
            "a trigger may have at most {MAX_TRIGGER_SLOTS} slots"
        ));
    }
    if !trigger_has_literal_word(&snippet.trigger) {
        return Err(
            "a trigger needs at least one literal word next to its slots, \
            or it would swallow every phrase"
                .to_string(),
        );
    }
    let segments = parse_expansion(snippet, &slot_names, warnings)?;
    let id = compiled.templates.len().to_string();
    compiled
        .grammar
        .add(id, &snippet.trigger)
        .map_err(|e| e.to_string())?;
    compiled.templates.push(segments);
    Ok(())
}

/// Slot names as they appear in the trigger, in order. Mirrors the grammar's
/// own slot parsing (name before an optional `:` spec) but runs first so the
/// expansion can be validated against them.
fn trigger_slot_names(trigger: &str) -> Result<Vec<String>, String> {
    let mut names = Vec::new();
    let mut rest = trigger;
    while let Some(open) = rest.find('{') {
        let after = &rest[open + 1..];
        let close = after
            .find('}')
            .ok_or_else(|| "unbalanced '{' in trigger".to_string())?;
        let body = &after[..close];
        let name = body.split_once(':').map_or(body, |(n, _)| n).trim();
        if name.is_empty() {
            return Err(format!("invalid slot '{{{body}}}'"));
        }
        names.push(name.to_string());
        rest = &after[close + 1..];
    }
    Ok(names)
}

/// A trigger consisting only of slots (and punctuation) would match any
/// phrase at all; require at least one literal word outside the braces.
fn trigger_has_literal_word(trigger: &str) -> bool {
    let mut outside = String::new();
    let mut rest = trigger;
    while let Some(open) = rest.find('{') {
        outside.push_str(&rest[..open]);
        match rest[open + 1..].find('}') {
            Some(close) => rest = &rest[open + 1 + close + 1..],
            None => return false,
        }
    }
    outside.push_str(rest);
    outside.chars().any(char::is_alphanumeric)
}

/// Parse an expansion into literal and slot segments. `{{` / `}}` are literal
/// braces, handled during the scan so `{{name}}` never mis-parses as a slot.
/// `\n` and `\t` are interpreted here, on the parameterized path only. An
/// unknown transform degrades to the verbatim capture with a warning; an
/// unknown slot name disables the snippet.
fn parse_expansion(
    snippet: &Snippet,
    slot_names: &[String],
    warnings: &mut Vec<String>,
) -> Result<Vec<Segment>, String> {
    let mut segments = Vec::new();
    let mut literal = String::new();
    let mut chars = snippet.expansion.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' if chars.peek() == Some(&'{') => {
                chars.next();
                literal.push('{');
            }
            '}' if chars.peek() == Some(&'}') => {
                chars.next();
                literal.push('}');
            }
            '{' => {
                let mut body = String::new();
                loop {
                    match chars.next() {
                        Some('}') => break,
                        Some(ch) => body.push(ch),
                        None => return Err("unbalanced '{' in expansion".to_string()),
                    }
                }
                if !literal.is_empty() {
                    segments.push(Segment::Literal(std::mem::take(&mut literal)));
                }
                segments.push(parse_slot_reference(&body, snippet, slot_names, warnings)?);
            }
            '\\' => literal.push_str(match chars.next() {
                Some('n') => "\n",
                Some('t') => "\t",
                Some('\\') => "\\",
                Some(other) => {
                    literal.push('\\');
                    literal.push(other);
                    continue;
                }
                None => "\\",
            }),
            other => literal.push(other),
        }
    }
    if !literal.is_empty() {
        segments.push(Segment::Literal(literal));
    }
    Ok(segments)
}

/// Resolve one `{name}` / `{name:transform}` reference in the expansion.
fn parse_slot_reference(
    body: &str,
    snippet: &Snippet,
    slot_names: &[String],
    warnings: &mut Vec<String>,
) -> Result<Segment, String> {
    let (name, spec) = match body.split_once(':') {
        Some((n, s)) => (n.trim(), Some(s.trim())),
        None => (body.trim(), None),
    };
    if !slot_names.iter().any(|s| s == name) {
        return Err(format!(
            "the expansion references '{{{name}}}' but the trigger has no such slot"
        ));
    }
    let transform = match spec {
        None => None,
        Some(s) => match Transform::parse(s) {
            Some(t) => Some(t),
            None => {
                warnings.push(format!(
                    "Snippet \"{}\": unknown transform '{s}'; the capture is inserted as spoken.",
                    snippet.trigger.trim()
                ));
                None
            }
        },
    };
    Ok(Segment::Slot {
        name: name.to_string(),
        transform,
    })
}

/// A captured slot value as substitution text.
fn slot_text(value: Option<&SlotValue>) -> String {
    match value {
        Some(SlotValue::Text(s)) => s.clone(),
        Some(SlotValue::Choice(s)) => s.clone(),
        Some(SlotValue::Number(n)) => n.to_string(),
        // Unreachable in practice: a matched pattern captured all its slots.
        None => String::new(),
    }
}

/// Truncate to at most `max` characters, on a char boundary.
fn clamp_chars(s: &mut String, max: usize) {
    if let Some((cut, _)) = s.char_indices().nth(max) {
        s.truncate(cut);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snip(trigger: &str, expansion: &str) -> Snippet {
        Snippet {
            trigger: trigger.to_string(),
            expansion: expansion.to_string(),
        }
    }

    fn compiled(entries: &[(&str, &str)]) -> CompiledSnippets {
        let snippets: Vec<Snippet> = entries.iter().map(|(t, e)| snip(t, e)).collect();
        let (compiled, warnings) = compile(&snippets);
        assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
        compiled
    }

    // High-risk behavior 1: static snippets must not change behavior.

    #[test]
    fn static_snippet_matches_exactly_as_before() {
        let c = compiled(&[("my email", "user@example.com")]);
        assert_eq!(c.expand("My email."), Some("user@example.com".to_string()));
        assert_eq!(c.expand("send my email now"), None);
    }

    #[test]
    fn static_expansion_keeps_backslash_sequences_verbatim() {
        // Escape interpretation is opted into by a slot in the trigger; a
        // shipped static snippet stays byte-identical.
        let c = compiled(&[("sig", "line one\\nline two\\ttabbed")]);
        assert_eq!(
            c.expand("sig"),
            Some("line one\\nline two\\ttabbed".to_string())
        );
    }

    #[test]
    fn static_expansion_keeps_braces_verbatim() {
        let c = compiled(&[("block", "fn main() {}")]);
        assert_eq!(c.expand("block"), Some("fn main() {}".to_string()));
    }

    #[test]
    fn parameterized_expansion_interprets_escapes() {
        let c = compiled(&[("new test {name}", "#[test]\\nfn {name:snake}() {{\\n}}")]);
        assert_eq!(
            c.expand("new test user login"),
            Some("#[test]\nfn user_login() {\n}".to_string())
        );
    }

    // High-risk behavior 2: silence never fires a snippet on either path.

    #[test]
    fn silence_never_fires_static_or_parameterized() {
        let snippets = vec![
            snip("...", "my-secret-signature"),
            snip("", "boom"),
            snip("run {task}", "run: {task}"),
        ];
        let (c, _) = compile(&snippets);
        for phrase in [".", "?", "!!", "  ,  ", "", "..."] {
            assert_eq!(c.expand(phrase), None, "phrase {phrase:?} must not fire");
        }
    }

    #[test]
    fn slot_only_trigger_is_refused() {
        let (c, warnings) = compile(&[snip("{x}", "swallowed: {x}")]);
        assert_eq!(c.expand("any words at all"), None);
        assert!(warnings.iter().any(|w| w.contains("disabled")));
    }

    #[test]
    fn punctuation_plus_slot_trigger_is_refused() {
        let (c, warnings) = compile(&[snip("... {x}", "{x}")]);
        assert_eq!(c.expand("some phrase"), None);
        assert_eq!(warnings.len(), 1);
    }

    // High-risk behavior 3: precedence.

    #[test]
    fn exact_static_beats_parameterized() {
        let c = compiled(&[
            ("deploy {env}", "deploy to {env}"),
            ("deploy staging", "the exact one"),
        ]);
        assert_eq!(
            c.expand("deploy staging"),
            Some("the exact one".to_string())
        );
        assert_eq!(
            c.expand("deploy production"),
            Some("deploy to production".to_string())
        );
    }

    #[test]
    fn parameterized_config_order_first_match_wins() {
        let c = compiled(&[
            ("make {kind} widget", "specific: {kind}"),
            ("make {thing}", "general: {thing}"),
        ]);
        assert_eq!(
            c.expand("make blue widget"),
            Some("specific: blue".to_string())
        );
        assert_eq!(c.expand("make coffee"), Some("general: coffee".to_string()));
    }

    // Transforms.

    #[test]
    fn all_transforms_on_one_input() {
        let c = compiled(&[(
            "case {name}",
            "{name:pascal}|{name:camel}|{name:snake}|{name:kebab}|{name:upper}|{name:lower}|{name:title}|{name}",
        )]);
        assert_eq!(
            c.expand("case user profile card"),
            Some(
                "UserProfileCard|userProfileCard|user_profile_card|user-profile-card|\
                 USER_PROFILE_CARD|user profile card|User Profile Card|user profile card"
                    .to_string()
            )
        );
    }

    #[test]
    fn transforms_tolerate_digits_symbols_and_unicode() {
        for input in ["v2 api", "=> arrow", "café menu", "useState", "42"] {
            for t in [
                Transform::Pascal,
                Transform::Camel,
                Transform::Snake,
                Transform::Kebab,
                Transform::Upper,
                Transform::Lower,
                Transform::Title,
            ] {
                let out = t.apply(input);
                assert!(!out.is_empty(), "{t:?} on {input:?} produced empty");
            }
        }
        assert_eq!(Transform::Pascal.apply("café menu"), "CaféMenu");
        assert_eq!(Transform::Snake.apply("v2 api"), "v2_api");
    }

    #[test]
    fn repeated_slot_with_different_transforms() {
        let c = compiled(&[(
            "create react component {name}",
            "export function {name:pascal}() {{\\n  return null;\\n}}",
        )]);
        assert_eq!(
            c.expand("create react component user profile"),
            Some("export function UserProfile() {\n  return null;\n}".to_string())
        );
        let c = compiled(&[(
            "import {module}",
            "import {module:camel} from './{module:kebab}';",
        )]);
        assert_eq!(
            c.expand("import date utils"),
            Some("import dateUtils from './date-utils';".to_string())
        );
    }

    #[test]
    fn multiple_slots_capture_with_backtracking() {
        let c = compiled(&[("map {key} to {value}", "{key:snake}: {value:snake}")]);
        assert_eq!(
            c.expand("map user name to display label"),
            Some("user_name: display_label".to_string())
        );
    }

    // Guardrails.

    #[test]
    fn more_than_three_slots_is_refused() {
        let (c, warnings) = compile(&[snip("mix {a} {b} {c} {d}", "{a}{b}{c}{d}")]);
        assert_eq!(c.expand("mix one two three four"), None);
        assert!(warnings.iter().any(|w| w.contains("disabled")));
    }

    #[test]
    fn phrases_over_word_limit_skip_parameterized_matching() {
        let c = compiled(&[("run {task}", "run: {task}")]);
        let long = format!("run {}", vec!["word"; MAX_PARAM_PHRASE_WORDS].join(" "));
        assert_eq!(c.expand(&long), None);
        // At the limit it still matches.
        let ok = format!("run {}", vec!["word"; MAX_PARAM_PHRASE_WORDS - 1].join(" "));
        assert!(c.expand(&ok).is_some());
    }

    #[test]
    fn expanded_output_is_clamped_after_substitution() {
        let big_literal = "x".repeat(900);
        let c = compiled(&[(
            "echo {text}",
            &format!("{big_literal}{{text}}{big_literal}{{text}}{big_literal}"),
        )]);
        let capture = vec!["word"; 30].join(" ");
        let out = c.expand(&format!("echo {capture}")).unwrap();
        assert_eq!(out.chars().count(), MAX_EXPANSION_CHARS);
    }

    // Failure tolerance.

    #[test]
    fn unknown_expansion_slot_disables_snippet_with_warning() {
        let (c, warnings) = compile(&[snip("greet {name}", "hello {nam}")]);
        assert_eq!(c.expand("greet world"), None);
        assert!(warnings.iter().any(|w| w.contains("disabled")));
    }

    #[test]
    fn unknown_transform_falls_back_to_verbatim_and_stays_active() {
        let (c, warnings) = compile(&[snip("greet {name}", "hello {name:sponge}")]);
        assert_eq!(c.expand("greet world"), Some("hello world".to_string()));
        assert!(warnings.iter().any(|w| w.contains("unknown transform")));
    }

    #[test]
    fn malformed_trigger_or_expansion_is_inert_not_fatal() {
        let snippets = vec![
            snip("open {", "broken"),
            snip("pair {x} and {x}", "{x}"),
            snip("dangling {x}", "oops {"),
            snip("my email", "still@works.com"),
        ];
        let (c, warnings) = compile(&snippets);
        assert_eq!(warnings.len(), 3);
        assert_eq!(c.expand("my email"), Some("still@works.com".to_string()));
        assert_eq!(c.expand("open sesame"), None);
    }

    #[test]
    fn literal_double_braces_in_parameterized_expansion() {
        // {{name}} is a literal "{name}", not a slot reference.
        let c = compiled(&[("wrap {name}", "{{name}} is {name}")]);
        assert_eq!(
            c.expand("wrap this thing"),
            Some("{name} is this thing".to_string())
        );
    }

    #[test]
    fn existing_shadow_and_duplicate_warnings_are_preserved() {
        let snippets = vec![
            snip("scratch that", "x"),
            snip("sig", "a"),
            snip("Sig", "b"),
        ];
        let (_, warnings) = compile(&snippets);
        assert!(warnings.iter().any(|w| w.contains("built-in command")));
        assert!(warnings.iter().any(|w| w.contains("Duplicate")));
    }

    #[test]
    fn capture_is_never_a_snippet_trigger_recursively() {
        // Expansion output is final text; a capture equal to another trigger
        // must not re-expand.
        let c = compiled(&[("say {word}", "{word}!"), ("ping", "pong")]);
        assert_eq!(c.expand("say ping"), Some("ping!".to_string()));
    }
}
