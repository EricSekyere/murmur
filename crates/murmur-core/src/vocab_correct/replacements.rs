//! Explicit corrections approved by the user; never inferred from history.

use serde::{Deserialize, Serialize};

use super::tokenize;

pub const MAX_RULES: usize = 100;
const MAX_CHARS: usize = 160;
const MAX_WORDS: usize = 8;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Replacement {
    #[serde(default)]
    pub heard: String,
    #[serde(default)]
    pub replacement: String,
}

fn key(rule: &Replacement) -> String {
    rule.heard
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn valid(rule: &Replacement) -> bool {
    let words: Vec<_> = rule.heard.split_whitespace().collect();
    !words.is_empty()
        && words.len() <= MAX_WORDS
        && rule.heard.chars().count() <= MAX_CHARS
        && !rule.heard.contains(['\r', '\n'])
        && words.iter().all(|word| {
            word.starts_with(char::is_alphanumeric) && word.ends_with(char::is_alphanumeric)
        })
        && !rule.replacement.trim().is_empty()
        && rule.replacement.chars().count() <= MAX_CHARS
        && !rule.replacement.chars().any(char::is_control)
}

/// Validate a user edit without silently changing what they approved.
pub fn validate(rules: &[Replacement]) -> Result<(), String> {
    if rules.len() > MAX_RULES {
        return Err(format!("At most {MAX_RULES} correction pairs are allowed."));
    }
    let mut seen = std::collections::HashSet::new();
    for (index, rule) in rules.iter().enumerate() {
        if !valid(rule) {
            return Err(format!(
                "Correction {} needs 1-8 whole heard words and a replacement, each at most 160 characters, without line breaks.",
                index + 1
            ));
        }
        if !seen.insert(key(rule)) {
            return Err(format!(
                "Correction {} repeats an earlier heard phrase.",
                index + 1
            ));
        }
    }
    Ok(())
}

/// Recover hand-edited settings without inventing or truncating a correction.
pub fn sanitize(rules: &mut Vec<Replacement>) {
    let mut seen = std::collections::HashSet::new();
    rules.retain(|rule| valid(rule) && seen.insert(key(rule)));
    rules.truncate(MAX_RULES);
}

/// Replace whole phrases once. Longest matches win; generated text is not rescanned.
pub fn apply(text: &str, rules: &[Replacement]) -> String {
    if rules.is_empty() {
        return text.to_string();
    }
    let mut patterns: Vec<_> = rules
        .iter()
        .filter(|rule| valid(rule))
        .map(|rule| {
            (
                key(rule)
                    .split_whitespace()
                    .map(str::to_string)
                    .collect::<Vec<_>>(),
                rule.replacement.trim(),
            )
        })
        .collect();
    patterns.sort_by_key(|(words, _)| std::cmp::Reverse(words.len()));
    let (tokens, trailing) = tokenize(text);
    let mut output = String::with_capacity(text.len());
    let mut i = 0;
    while i < tokens.len() {
        let first = &tokens[i];
        output.push_str(first.ws);
        output.push_str(first.prefix);
        let matched = patterns.iter().find(|(words, _)| {
            let Some(window) = tokens.get(i..i + words.len()) else {
                return false;
            };
            window
                .iter()
                .zip(words)
                .enumerate()
                .all(|(j, (token, word))| {
                    token.core.to_lowercase() == *word
                        && (j == 0 || (token.prefix.is_empty() && !token.ws.contains(['\r', '\n'])))
                        && (j + 1 == words.len() || token.suffix.is_empty())
                })
        });
        if let Some((words, replacement)) = matched {
            output.push_str(replacement);
            output.push_str(tokens[i + words.len() - 1].suffix);
            i += words.len();
        } else {
            output.push_str(first.core);
            output.push_str(first.suffix);
            i += 1;
        }
    }
    output.push_str(trailing);
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair(heard: &str, replacement: &str) -> Replacement {
        Replacement {
            heard: heard.into(),
            replacement: replacement.into(),
        }
    }

    #[test]
    fn exact_phrases_preserve_boundaries_and_do_not_cascade() {
        let rules = [pair("super base", "Supabase"), pair("Supabase", "database")];
        assert_eq!(
            apply("  (SUPER  BASE), super bases; super\nbase. ", &rules),
            "  (Supabase), super bases; super\nbase. "
        );
        assert_eq!(apply("super, base", &rules), "super, base");
    }

    #[test]
    fn longer_context_wins_without_changing_unmatched_words() {
        let rules = [
            pair("fresh", "Fresha"),
            pair("a fresh approach", "a fresh approach"),
        ];
        assert_eq!(
            apply("a fresh approach with fresh", &rules),
            "a fresh approach with Fresha"
        );
        assert_eq!(
            apply("CAFÉ freshen", &[pair("café", "Café")]),
            "Café freshen"
        );
    }

    #[test]
    fn bad_rules_and_ambiguous_duplicates_are_rejected() {
        for rules in [
            vec![pair("", "x")],
            vec![pair("a", " ")],
            vec![pair("a", "line\nbreak")],
            vec![pair("a", "b"), pair(" A ", "c")],
        ] {
            assert!(validate(&rules).is_err());
        }
        let mut rules = vec![pair("", "x"), pair(" A ", "b"), pair("a", "c")];
        sanitize(&mut rules);
        assert_eq!(rules, vec![pair(" A ", "b")]);
    }
}
