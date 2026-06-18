//! Lexical identifier extraction from source text.
//!
//! A regex pass, not a parser: it captures every identifier-shaped token,
//! including ones in comments and strings. Ranking and the stoplist downstream
//! filter the noise. Tree-sitter (Phase 2) would scope this to real symbols.

use std::sync::LazyLock;

use regex::Regex;

/// Programming identifiers: a leading letter or underscore, then letters,
/// digits, or underscores. Captures the whole token (`calculateTotalRevenue`,
/// `MAX_RETRIES`, `user_id`) since that is the unit the STT engine mangles.
static IDENTIFIER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[A-Za-z_][A-Za-z0-9_]*").expect("valid identifier regex"));

/// Yield every identifier in `source`, in order, borrowing from the input.
pub fn extract_identifiers(source: &str) -> impl Iterator<Item = &str> {
    IDENTIFIER.find_iter(source).map(|m| m.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_whole_identifiers() {
        let src = "let calculateTotalRevenue = MAX_RETRIES + user_id;";
        let got: Vec<&str> = extract_identifiers(src).collect();
        assert_eq!(
            got,
            vec!["let", "calculateTotalRevenue", "MAX_RETRIES", "user_id"]
        );
    }

    #[test]
    fn skips_punctuation_and_leading_digits() {
        // A digit-led token yields only its trailing identifier part.
        let got: Vec<&str> = extract_identifiers("foo.bar(42, x9, 3abc)").collect();
        assert_eq!(got, vec!["foo", "bar", "x9", "abc"]);
    }

    #[test]
    fn extracts_from_comments_and_strings() {
        // Lexical pass is intentionally unscoped; downstream filtering handles it.
        let got: Vec<&str> =
            extract_identifiers("// noteHere\nlet s = \"insideString\";").collect();
        assert!(got.contains(&"noteHere"));
        assert!(got.contains(&"insideString"));
    }
}
