//! Spoken Conventional Commit lines: "commit feat scope core add x" is
//! delivered as `feat(core): add x`.
//!
//! Pure text transform — Murmur only types the formatted line, it never runs
//! git. A phrase must start with "commit" followed by a valid Conventional
//! Commit type; anything else returns `None` and stays ordinary dictation,
//! so prose like "commit the changes to the repo" is untouched.

/// Valid Conventional Commit types. Requiring one right after "commit" is the
/// primary protection against false triggers in ordinary prose.
const TYPES: &[&str] = &[
    "feat", "fix", "docs", "style", "refactor", "perf", "test", "build", "ci", "chore", "revert",
];

/// Format `commit <type> [scope <scope>] [breaking] <description…>` as a
/// Conventional Commit line, or `None` when the phrase is not a commit
/// command and should be delivered unchanged.
pub fn format_commit(phrase: &str) -> Option<String> {
    // Same trim and trailing-punctuation handling as
    // `voice_commands::normalize`, but original casing is kept so the
    // description is delivered as spoken; keywords match case-insensitively.
    let trimmed = phrase
        .trim()
        .trim_end_matches(['.', '!', '?', ','])
        .trim_end();
    let mut tokens = trimmed.split_whitespace().peekable();

    tokens.next().filter(|t| t.eq_ignore_ascii_case("commit"))?;
    let commit_type = tokens
        .next()
        .map(str::to_lowercase)
        .filter(|t| TYPES.contains(&t.as_str()))?;

    let scope = tokens
        .next_if(|t| t.eq_ignore_ascii_case("scope"))
        .and_then(|_| tokens.next())
        .map(str::to_lowercase);
    let breaking = tokens
        .next_if(|t| t.eq_ignore_ascii_case("breaking"))
        .is_some();

    let remainder = tokens.collect::<Vec<_>>().join(" ");
    let description = remainder.trim_end_matches('.').trim_end();
    if description.is_empty() {
        // Never emit a bare "feat: ".
        return None;
    }
    let description = lowercase_first(description);

    let bang = if breaking { "!" } else { "" };
    Some(match scope {
        Some(scope) => format!("{commit_type}({scope}){bang}: {description}"),
        None => format!("{commit_type}{bang}: {description}"),
    })
}

/// Lowercase only the first character, per Conventional Commit description
/// style, leaving the rest as spoken (e.g. "Add JSON parser" -> "add JSON parser").
fn lowercase_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().chain(chars).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_type_is_accepted() {
        for ty in TYPES {
            assert_eq!(
                format_commit(&format!("commit {ty} do the thing")),
                Some(format!("{ty}: do the thing")),
            );
        }
    }

    #[test]
    fn scope_keyword_takes_one_token() {
        assert_eq!(
            format_commit("commit feat scope core add the vocabulary metric"),
            Some("feat(core): add the vocabulary metric".to_string()),
        );
    }

    #[test]
    fn no_scope_without_keyword() {
        assert_eq!(
            format_commit("commit fix handle the null case"),
            Some("fix: handle the null case".to_string()),
        );
    }

    #[test]
    fn breaking_without_scope() {
        assert_eq!(
            format_commit("commit feat breaking drop the old api"),
            Some("feat!: drop the old api".to_string()),
        );
    }

    #[test]
    fn breaking_after_scope() {
        assert_eq!(
            format_commit("commit feat scope core breaking drop the old api"),
            Some("feat(core)!: drop the old api".to_string()),
        );
    }

    #[test]
    fn invalid_type_stays_plain_text() {
        assert_eq!(format_commit("commit the changes to the repo"), None);
        assert_eq!(format_commit("commit all files now"), None);
    }

    #[test]
    fn non_commit_phrase_stays_plain_text() {
        assert_eq!(format_commit("hello world"), None);
        assert_eq!(format_commit("please commit feat something"), None);
    }

    #[test]
    fn empty_description_is_rejected() {
        assert_eq!(format_commit("commit"), None);
        assert_eq!(format_commit("commit feat"), None);
        assert_eq!(format_commit("commit feat scope core"), None);
        assert_eq!(format_commit("commit feat scope"), None);
        assert_eq!(format_commit("commit feat breaking"), None);
        assert_eq!(format_commit(""), None);
    }

    #[test]
    fn first_char_is_lowercased_rest_preserved() {
        assert_eq!(
            format_commit("commit feat Add JSON support"),
            Some("feat: add JSON support".to_string()),
        );
    }

    #[test]
    fn trailing_period_is_stripped() {
        assert_eq!(
            format_commit("Commit feat add the parser."),
            Some("feat: add the parser".to_string()),
        );
        assert_eq!(
            format_commit("commit fix handle nulls?"),
            Some("fix: handle nulls".to_string()),
        );
    }

    #[test]
    fn keywords_match_case_insensitively() {
        assert_eq!(
            format_commit("Commit FEAT SCOPE Core add x"),
            Some("feat(core): add x".to_string()),
        );
    }

    #[test]
    fn surrounding_whitespace_is_ignored() {
        assert_eq!(
            format_commit("  commit chore   tidy the imports  "),
            Some("chore: tidy the imports".to_string()),
        );
    }
}
