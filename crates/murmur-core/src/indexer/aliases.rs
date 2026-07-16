//! Spoken path aliases (PRD §5.5 Layer 2): rewrite spoken phrases into path
//! segments ("package json" -> `package.json`) before a query hits
//! [`super::resolve_file`]. Pure and deterministic; phonetic matching
//! (Soundex/Metaphone, also PRD §5.5) is deliberately deferred.

use std::collections::HashMap;

use crate::config::settings::PathAlias;

/// Built-in spoken-form → path-segment defaults, always active. A user alias
/// with the same spoken form overrides its builtin.
const BUILTIN_ALIASES: &[(&str, &str)] = &[
    ("source", "src"),
    ("package json", "package.json"),
    ("dot env", ".env"),
    ("read me", "README"),
    ("node modules", "node_modules"),
    ("git ignore", ".gitignore"),
    ("cargo toml", "Cargo.toml"),
];

/// Replace whole-word spoken phrases in `query` with their path segments,
/// case-insensitively. Longer spoken forms win over shorter ones ("package
/// json" beats any "json" alias) and each query word is consumed at most
/// once. Word boundaries are strict: "sourced" never matches "source".
/// Unmatched words pass through unchanged, so the output feeds
/// [`super::resolve_file`] directly (its tokenizer splits `/` and `.`).
pub fn apply_aliases(query: &str, user_aliases: &[PathAlias]) -> String {
    let table = alias_table(user_aliases);
    let words: Vec<&str> = query.split_whitespace().collect();
    let lowered: Vec<String> = words.iter().map(|w| w.to_lowercase()).collect();
    // Per-word replacement: None = untouched; a matched phrase's first word
    // carries the path and the rest collapse to empty.
    let mut replaced: Vec<Option<&str>> = vec![None; words.len()];
    for (spoken, path) in &table {
        let phrase: Vec<&str> = spoken.split_whitespace().collect();
        let mut start = 0;
        while start + phrase.len() <= words.len() {
            let window = start..start + phrase.len();
            let free = replaced[window.clone()].iter().all(Option::is_none);
            let hit = lowered[window.clone()]
                .iter()
                .map(String::as_str)
                .eq(phrase.iter().copied());
            if free && hit {
                replaced[start] = Some(path);
                for slot in &mut replaced[start + 1..window.end] {
                    *slot = Some("");
                }
                start += phrase.len();
            } else {
                start += 1;
            }
        }
    }
    words
        .iter()
        .zip(&replaced)
        .filter_map(|(word, r)| match r {
            None => Some(*word),
            Some(path) if !path.is_empty() => Some(*path),
            Some(_) => None,
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The merged builtin + user table, longest spoken form (in words) first so a
/// multi-word alias claims its words before any single-word alias can; ties
/// break lexicographically for determinism.
fn alias_table(user_aliases: &[PathAlias]) -> Vec<(String, String)> {
    let mut merged: HashMap<String, String> = BUILTIN_ALIASES
        .iter()
        .map(|(spoken, path)| (spoken.to_string(), path.to_string()))
        .collect();
    for alias in user_aliases {
        let spoken = alias.spoken.trim().to_lowercase();
        let path = alias.path.trim();
        if !spoken.is_empty() && !path.is_empty() {
            merged.insert(spoken, path.to_string());
        }
    }
    let mut table: Vec<(String, String)> = merged.into_iter().collect();
    let word_count = |s: &str| s.split_whitespace().count();
    table.sort_by(|a, b| {
        word_count(&b.0)
            .cmp(&word_count(&a.0))
            .then_with(|| a.0.cmp(&b.0))
    });
    table
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indexer::{directories, resolve_file};

    fn alias(spoken: &str, path: &str) -> PathAlias {
        PathAlias {
            spoken: spoken.into(),
            path: path.into(),
        }
    }

    #[test]
    fn builtins_apply_with_an_empty_user_list() {
        assert_eq!(
            apply_aliases("open the package json file", &[]),
            "open the package.json file"
        );
        assert_eq!(apply_aliases("the source folder", &[]), "the src folder");
        assert_eq!(apply_aliases("dot env", &[]), ".env");
    }

    #[test]
    fn multi_word_alias_wins_over_single_word() {
        // A greedy single-word "json" alias must not break "package json" apart.
        let user = [alias("json", "config.json")];
        assert_eq!(
            apply_aliases("open the package json file", &user),
            "open the package.json file"
        );
        // The single-word alias still applies where the longer one doesn't.
        assert_eq!(
            apply_aliases("the json file", &user),
            "the config.json file"
        );
    }

    #[test]
    fn matching_is_case_insensitive() {
        assert_eq!(apply_aliases("open Package JSON", &[]), "open package.json");
        let user = [alias("Docs", "documentation")];
        assert_eq!(
            apply_aliases("the DOCS folder", &user),
            "the documentation folder"
        );
    }

    #[test]
    fn word_boundaries_are_strict() {
        // "sourced" must not match the "source" alias.
        assert_eq!(apply_aliases("sourced files", &[]), "sourced files");
        assert_eq!(apply_aliases("resource file", &[]), "resource file");
    }

    #[test]
    fn user_alias_overrides_builtin_with_same_spoken_form() {
        let user = [alias("source", "lib")];
        assert_eq!(apply_aliases("the source folder", &user), "the lib folder");
    }

    #[test]
    fn each_word_is_consumed_at_most_once() {
        // Two overlapping matches cannot share the middle word.
        let user = [alias("alpha beta", "a/b"), alias("beta gamma", "b/g")];
        assert_eq!(apply_aliases("alpha beta gamma", &user), "a/b gamma");
    }

    #[test]
    fn empty_and_blank_queries_stay_empty() {
        assert_eq!(apply_aliases("", &[]), "");
        assert_eq!(apply_aliases("   ", &[]), "");
    }

    #[test]
    fn aliased_query_resolves_a_directory_end_to_end() {
        let files: Vec<String> = [
            "src/components/Header.tsx",
            "src/components/Footer.tsx",
            "src/user.ts",
            "tests/api/user_controller.test.ts",
            "README.md",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let dirs = directories(&files);
        let query = apply_aliases("go to the source components folder", &[]);
        let ranked = resolve_file(&query, &dirs);
        assert_eq!(ranked[0].path, "src/components");
    }

    #[test]
    fn dot_env_alias_resolves_the_env_file() {
        let files: Vec<String> = [".env", "src/main.rs", "docs/env-setup.md"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let query = apply_aliases("open the dot env file", &[]);
        let ranked = resolve_file(&query, &files);
        assert_eq!(ranked[0].path, ".env");
    }
}
