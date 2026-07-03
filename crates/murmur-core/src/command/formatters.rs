//! Talon-style spoken identifier formatters for command mode.
//!
//! Complements `stt::postprocess::apply_casing_formatters`, which rewrites
//! casing keywords found inline in free dictation (bounded by stop words).
//! Here a command-mode caller already holds a whole spoken phrase such as
//! "snake hello world" and wants everything after the style word formatted.
//! Reuses `capitalize` from postprocess; the split/join logic is new because
//! postprocess's formatter is private, string-keyed, lacks the Constant and
//! Dot styles, and its inline-scan stop-word semantics do not apply to a
//! whole-phrase command.

use crate::stt::postprocess::capitalize;

/// Identifier casing styles for spoken formatting commands.
/// `Constant` is SCREAMING_SNAKE_CASE.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaseStyle {
    Snake,
    Camel,
    Pascal,
    Kebab,
    Constant,
    Dot,
}

/// Format spoken words as an identifier in the given style.
///
/// Tokens split on whitespace, common identifier separators (`_`, `-`, `.`,
/// `/`), and lower-to-upper camel boundaries, so already-formatted input like
/// "helloWorld" or "MAX_SIZE" re-formats cleanly. Empty input yields "".
pub fn format_identifier(words: &str, style: CaseStyle) -> String {
    let tokens = split_tokens(words);
    if tokens.is_empty() {
        return String::new();
    }
    match style {
        CaseStyle::Snake => tokens.join("_"),
        CaseStyle::Kebab => tokens.join("-"),
        CaseStyle::Dot => tokens.join("."),
        CaseStyle::Constant => tokens
            .iter()
            .map(|t| t.to_uppercase())
            .collect::<Vec<_>>()
            .join("_"),
        CaseStyle::Camel => {
            let mut out = tokens[0].clone();
            for token in &tokens[1..] {
                out.push_str(&capitalize(token));
            }
            out
        }
        CaseStyle::Pascal => tokens.iter().map(|t| capitalize(t)).collect(),
    }
}

/// Parse a spoken case command: a leading style word followed by the words to
/// format. Recognizes "snake", "camel", "pascal", "kebab", "constant",
/// "screaming" (alias for Constant, with an optional trailing "snake"), and
/// "dot". An optional "case" after the style word is swallowed, so
/// "camel case get user" works. Returns `None` when the phrase does not start
/// with a style word or nothing follows it.
pub fn parse_case_command(phrase: &str) -> Option<(CaseStyle, String)> {
    let mut words = phrase.split_whitespace().peekable();
    let first = words.next()?;
    let style = style_word(first)?;
    if first.eq_ignore_ascii_case("screaming") {
        words.next_if(|w| w.eq_ignore_ascii_case("snake"));
    }
    words.next_if(|w| w.eq_ignore_ascii_case("case"));
    let rest: Vec<&str> = words.collect();
    if rest.is_empty() {
        return None;
    }
    // Privacy: log the style only, never the dictated words.
    tracing::debug!(?style, "parsed spoken case command");
    Some((style, rest.join(" ")))
}

fn style_word(word: &str) -> Option<CaseStyle> {
    match word.to_ascii_lowercase().as_str() {
        "snake" => Some(CaseStyle::Snake),
        "camel" => Some(CaseStyle::Camel),
        "pascal" => Some(CaseStyle::Pascal),
        "kebab" => Some(CaseStyle::Kebab),
        "constant" | "screaming" => Some(CaseStyle::Constant),
        "dot" => Some(CaseStyle::Dot),
        _ => None,
    }
}

/// Lowercased word tokens: split on whitespace and identifier separators,
/// then on camel boundaries within each fragment.
fn split_tokens(input: &str) -> Vec<String> {
    input
        .split(|c: char| c.is_whitespace() || matches!(c, '_' | '-' | '.' | '/'))
        .filter(|fragment| !fragment.is_empty())
        .flat_map(split_camel)
        .map(|token| token.to_lowercase())
        .collect()
}

/// Split a fragment at lower/digit to upper transitions ("getUser" splits to
/// "get" + "User"). All-caps runs like "MAX" stay intact.
fn split_camel(fragment: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut prev_lower = false;
    for c in fragment.chars() {
        if c.is_uppercase() && prev_lower && !current.is_empty() {
            parts.push(std::mem::take(&mut current));
        }
        current.push(c);
        prev_lower = c.is_lowercase() || c.is_ascii_digit();
    }
    if !current.is_empty() {
        parts.push(current);
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_styles_on_three_words() {
        let cases = [
            (CaseStyle::Snake, "hello_world_foo"),
            (CaseStyle::Camel, "helloWorldFoo"),
            (CaseStyle::Pascal, "HelloWorldFoo"),
            (CaseStyle::Kebab, "hello-world-foo"),
            (CaseStyle::Constant, "HELLO_WORLD_FOO"),
            (CaseStyle::Dot, "hello.world.foo"),
        ];
        for (style, expected) in cases {
            assert_eq!(format_identifier("hello world foo", style), expected);
        }
    }

    #[test]
    fn single_word_per_style() {
        assert_eq!(format_identifier("hello", CaseStyle::Snake), "hello");
        assert_eq!(format_identifier("hello", CaseStyle::Camel), "hello");
        assert_eq!(format_identifier("hello", CaseStyle::Pascal), "Hello");
        assert_eq!(format_identifier("hello", CaseStyle::Constant), "HELLO");
    }

    #[test]
    fn empty_and_whitespace_input() {
        assert_eq!(format_identifier("", CaseStyle::Camel), "");
        assert_eq!(format_identifier("   ", CaseStyle::Snake), "");
        assert_eq!(
            format_identifier("  hello   world  ", CaseStyle::Kebab),
            "hello-world"
        );
    }

    #[test]
    fn reformats_already_symbol_input() {
        assert_eq!(
            format_identifier("hello_world", CaseStyle::Camel),
            "helloWorld"
        );
        assert_eq!(
            format_identifier("helloWorld", CaseStyle::Snake),
            "hello_world"
        );
        assert_eq!(
            format_identifier("get-user.name", CaseStyle::Pascal),
            "GetUserName"
        );
        assert_eq!(format_identifier("MAX_SIZE", CaseStyle::Kebab), "max-size");
    }

    #[test]
    fn parse_snake_command() {
        assert_eq!(
            parse_case_command("snake hello world"),
            Some((CaseStyle::Snake, "hello world".to_string()))
        );
    }

    #[test]
    fn parse_camel_case_swallows_case_word() {
        assert_eq!(
            parse_case_command("camel case get user"),
            Some((CaseStyle::Camel, "get user".to_string()))
        );
    }

    #[test]
    fn parse_constant_and_screaming_aliases() {
        assert_eq!(
            parse_case_command("constant max size"),
            Some((CaseStyle::Constant, "max size".to_string()))
        );
        assert_eq!(
            parse_case_command("screaming max size"),
            Some((CaseStyle::Constant, "max size".to_string()))
        );
        assert_eq!(
            parse_case_command("screaming snake case max size"),
            Some((CaseStyle::Constant, "max size".to_string()))
        );
    }

    #[test]
    fn parse_dot_command() {
        assert_eq!(
            parse_case_command("dot config value"),
            Some((CaseStyle::Dot, "config value".to_string()))
        );
    }

    #[test]
    fn parse_rejects_non_commands_and_bare_style_words() {
        assert_eq!(parse_case_command("hello world"), None);
        assert_eq!(parse_case_command(""), None);
        assert_eq!(parse_case_command("snake"), None);
        assert_eq!(parse_case_command("  camel case  "), None);
    }

    #[test]
    fn parse_then_format_round_trip() {
        let parsed = parse_case_command("pascal user service");
        let (style, words) = parsed.expect("should parse");
        assert_eq!(format_identifier(&words, style), "UserService");
    }
}
