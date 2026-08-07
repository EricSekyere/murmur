//! Spoken file resolution: rank indexed project paths against a spoken query
//! ("the user controller test file") by token overlap, then decide whether
//! the winner is clear enough to act on ([`decide`]). Pure and deterministic;
//! no I/O. The Tier-2 semantic fallback lives in the app layer when it ships
//! (viable-features.md §1.1).

/// One candidate path with its resolver score (higher is better).
#[derive(Debug, Clone, PartialEq)]
pub struct FileMatch {
    pub path: String,
    pub score: f64,
}

/// What the ranked matches settle on: nothing, one clear winner, or a
/// near-tie the user must pick from.
#[derive(Debug, Clone, PartialEq)]
pub enum Resolution {
    /// No indexed path overlapped the query.
    None,
    /// The top match leads its runner-up by at least the ambiguity gap.
    Decisive(FileMatch),
    /// Every match within the gap of the top score, best first (2 to
    /// [`MAX_PICKER_CANDIDATES`] entries).
    Ambiguous(Vec<FileMatch>),
}

/// Runner-up gap below which acting on the top match would be a guess.
/// Scoring moves in steps of at least 0.5 (a stem hit; a whole token or the
/// exact-stem bonus is 1.0), while same-named files in different directories
/// differ only by the 0.01-per-token path penalty, i.e. a few hundredths.
/// Strictly-less-than keeps a clean half-point differentiator decisive.
const AMBIGUITY_GAP: f64 = 0.5;

/// Most near-tied candidates worth showing in a picker.
const MAX_PICKER_CANDIDATES: usize = 5;

/// Decide what to do with matches ranked by [`resolve_file`] (best first):
/// act on a decisive winner, or surface the near-tied set for the user.
pub fn decide(ranked: Vec<FileMatch>) -> Resolution {
    let mut iter = ranked.into_iter();
    let Some(top) = iter.next() else {
        return Resolution::None;
    };
    // Near-tie is measured against the top score, not pairwise, so a chain
    // of small gaps cannot drag in a candidate far below the winner.
    let runners_up: Vec<FileMatch> = iter
        .take(MAX_PICKER_CANDIDATES - 1)
        .take_while(|m| top.score - m.score < AMBIGUITY_GAP)
        .collect();
    if runners_up.is_empty() {
        return Resolution::Decisive(top);
    }
    let mut candidates = vec![top];
    candidates.extend(runners_up);
    Resolution::Ambiguous(candidates)
}

/// Spoken words that carry no signal about which file is meant.
const FILLER_WORDS: &[&str] = &["the", "a", "an", "file", "files", "open", "go", "to"];

/// Rank `files` against a spoken `query`, best match first. Scoring is plain
/// token overlap, with filename hits worth more than directory hits, a bonus
/// for naming the file stem exactly, and a light penalty on longer paths so
/// `src/user.ts` beats `vendor/x/y/z/user.ts` on equal overlap. Paths with no
/// overlapping token are omitted; ties break lexicographically for stability.
pub fn resolve_file(query: &str, files: &[String]) -> Vec<FileMatch> {
    let query_tokens = query_tokens(query);
    if query_tokens.is_empty() {
        return Vec::new();
    }
    let mut matches: Vec<FileMatch> = files
        .iter()
        .filter_map(|path| {
            score_path(&query_tokens, path).map(|score| FileMatch {
                path: path.clone(),
                score,
            })
        })
        .collect();
    matches.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.path.cmp(&b.path))
    });
    matches
}

/// Score one path against the query tokens; `None` when nothing overlaps.
fn score_path(query_tokens: &[String], path: &str) -> Option<f64> {
    let path_tokens = tokenize(path);
    let stem_tokens = tokenize(file_stem(path));
    let overlap = query_tokens
        .iter()
        .filter(|t| path_tokens.contains(t))
        .count();
    if overlap == 0 {
        return None;
    }
    let stem_hits = query_tokens
        .iter()
        .filter(|t| stem_tokens.contains(t))
        .count();
    let exact_stem = stem_hits == query_tokens.len()
        && !stem_tokens.is_empty()
        && stem_tokens.iter().all(|t| query_tokens.contains(t));
    let mut score = overlap as f64 + 0.5 * stem_hits as f64 - 0.01 * path_tokens.len() as f64;
    if exact_stem {
        score += 1.0;
    }
    Some(score)
}

/// The file name without its final extension: `a/b/user.test.ts` -> `user.test`.
/// Dot-prefixed names (`.env`, `.github`) keep the whole name: an empty stem
/// would cost them the stem bonus and let any path with the same token outrank
/// the exact hit.
fn file_stem(path: &str) -> &str {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.rsplit_once('.')
        .filter(|(stem, _)| !stem.is_empty())
        .map_or(name, |(stem, _)| stem)
}

/// Every distinct ancestor directory of `files` (relative, forward-slash, no
/// leading `./`, no empty root entry), sorted and deduped. Pure derivation:
/// the file index stays the single source of truth, so directory resolution
/// (`resolve_file` over this list) never needs its own file-system walk.
pub fn directories(files: &[String]) -> Vec<String> {
    let mut dirs = std::collections::BTreeSet::new();
    for file in files {
        for (i, ch) in file.char_indices() {
            if ch == '/' && i > 0 {
                dirs.insert(file[..i].to_string());
            }
        }
    }
    dirs.into_iter().collect()
}

/// Query words minus fillers, singularized alongside path tokens so
/// "tests"/"specs" bias toward `test`/`spec` paths.
fn query_tokens(query: &str) -> Vec<String> {
    tokenize(query)
        .into_iter()
        .filter(|t| !FILLER_WORDS.contains(&t.as_str()))
        .collect()
}

/// Lowercase tokens split on path separators, `_`, `-`, `.`, whitespace, and
/// camelCase boundaries, so `UserController.test.ts` and the spoken words
/// "user controller test" produce the same tokens.
fn tokenize(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut prev_lower_or_digit = false;
    for ch in text.chars() {
        if !ch.is_alphanumeric() {
            push_token(&mut tokens, &mut current);
            prev_lower_or_digit = false;
            continue;
        }
        if ch.is_uppercase() && prev_lower_or_digit {
            push_token(&mut tokens, &mut current);
        }
        current.extend(ch.to_lowercase());
        prev_lower_or_digit = ch.is_lowercase() || ch.is_numeric();
    }
    push_token(&mut tokens, &mut current);
    tokens
}

fn push_token(tokens: &mut Vec<String>, current: &mut String) {
    if current.is_empty() {
        return;
    }
    let token = std::mem::take(current);
    // Fold the common spoken plurals onto the singular so "tests" finds
    // `tests/` directories and `.test.ts` suffixes alike.
    tokens.push(match token.as_str() {
        "tests" => "test".to_string(),
        "specs" => "spec".to_string(),
        _ => token,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(paths: &[&str]) -> Vec<String> {
        paths.iter().map(|p| p.to_string()).collect()
    }

    #[test]
    fn test_file_query_outranks_bare_source_file() {
        let files = files(&[
            "src/user.ts",
            "tests/api/user_controller.test.ts",
            "src/controllers/user_controller.ts",
        ]);
        let ranked = resolve_file("user controller test", &files);
        assert_eq!(ranked[0].path, "tests/api/user_controller.test.ts");
        assert!(ranked[0].score > ranked[1].score);
    }

    #[test]
    fn spoken_words_match_camel_case_filenames() {
        let files = files(&["src/UserController.ts", "src/order.ts"]);
        let ranked = resolve_file("open the user controller file", &files);
        assert_eq!(ranked[0].path, "src/UserController.ts");
        assert_eq!(ranked.len(), 1, "no overlap must be omitted");
    }

    #[test]
    fn no_overlap_yields_empty() {
        let files = files(&["src/user.ts", "docs/readme.md"]);
        assert!(resolve_file("quantum flux capacitor", &files).is_empty());
        // A query of nothing but filler words resolves nothing.
        assert!(resolve_file("open the file", &files).is_empty());
        assert!(resolve_file("", &files).is_empty());
    }

    #[test]
    fn filler_words_do_not_change_the_winner() {
        let files = files(&["src/billing/invoice.rs", "src/billing/mod.rs"]);
        let plain = resolve_file("invoice", &files);
        let wordy = resolve_file("open the invoice file", &files);
        assert_eq!(plain[0].path, wordy[0].path);
        assert_eq!(plain[0].path, "src/billing/invoice.rs");
    }

    #[test]
    fn tests_plural_biases_toward_test_paths() {
        let files = files(&["tests/user.test.ts", "src/user.ts"]);
        let ranked = resolve_file("user tests", &files);
        assert_eq!(ranked[0].path, "tests/user.test.ts");
    }

    #[test]
    fn shorter_path_wins_on_equal_overlap() {
        let files = files(&["vendor/deep/nested/pile/user.ts", "src/user.ts"]);
        let ranked = resolve_file("user", &files);
        assert_eq!(ranked[0].path, "src/user.ts");
    }

    #[test]
    fn exact_stem_hit_beats_partial_stem() {
        let files = files(&["src/user_profile.ts", "src/user.ts"]);
        let ranked = resolve_file("user", &files);
        assert_eq!(ranked[0].path, "src/user.ts");
    }

    #[test]
    fn ordering_is_deterministic_on_score_ties() {
        let files = files(&["b/user.ts", "a/user.ts"]);
        let ranked = resolve_file("user", &files);
        assert_eq!(ranked[0].path, "a/user.ts");
    }

    #[test]
    fn empty_file_list_yields_empty() {
        assert!(resolve_file("user controller", &[]).is_empty());
    }

    #[test]
    fn dotfile_exact_hit_outranks_paths_sharing_its_token() {
        let files = files(&[".env", "docs/env-setup.md"]);
        let ranked = resolve_file("env", &files);
        assert_eq!(ranked[0].path, ".env");
    }

    #[test]
    fn directories_yield_all_ancestors_sorted_and_deduped() {
        let files = files(&[
            "src/components/Header.tsx",
            "src/components/Footer.tsx",
            "src/user.ts",
            "tests/api/user_controller.test.ts",
            "README.md",
        ]);
        assert_eq!(
            directories(&files),
            vec![
                "src".to_string(),
                "src/components".to_string(),
                "tests".to_string(),
                "tests/api".to_string(),
            ]
        );
    }

    #[test]
    fn directories_have_no_empty_or_dot_entries() {
        let files = files(&["top.rs", "/rooted/file.rs", "a/b/c.rs"]);
        let dirs = directories(&files);
        assert!(!dirs.iter().any(|d| d.is_empty() || d == "."));
        assert_eq!(
            dirs,
            vec!["/rooted".to_string(), "a".to_string(), "a/b".to_string(),]
        );
    }

    #[test]
    fn directory_resolution_ranks_the_named_leaf_first() {
        let files = files(&[
            "src/components/Header.tsx",
            "src/user.ts",
            "vendor/components/x.ts",
        ]);
        let dirs = directories(&files);
        let ranked = resolve_file("src components", &dirs);
        assert_eq!(ranked[0].path, "src/components");
        assert!(ranked[0].score > ranked[1].score);
    }

    #[test]
    fn directories_of_empty_index_is_empty() {
        assert!(directories(&[]).is_empty());
    }

    fn fm(path: &str, score: f64) -> FileMatch {
        FileMatch {
            path: path.to_string(),
            score,
        }
    }

    fn ambiguous_paths(resolution: Resolution) -> Vec<String> {
        match resolution {
            Resolution::Ambiguous(candidates) => candidates.into_iter().map(|c| c.path).collect(),
            other => panic!("expected Ambiguous, got {other:?}"),
        }
    }

    #[test]
    fn same_name_in_two_directories_is_ambiguous() {
        // The archetypal near-tie: identical filenames whose scores differ
        // only by the per-token path penalty (a few hundredths).
        let files = files(&["src/api/user.ts", "src/web/user.ts", "docs/guide.md"]);
        let ranked = resolve_file("open the user file", &files);
        assert_eq!(
            ambiguous_paths(decide(ranked)),
            vec!["src/api/user.ts".to_string(), "src/web/user.ts".to_string()]
        );
    }

    #[test]
    fn same_name_at_different_depths_is_still_ambiguous() {
        let files = files(&["src/user.ts", "vendor/lib/user.ts"]);
        let ranked = resolve_file("user", &files);
        assert_eq!(
            ambiguous_paths(decide(ranked)),
            vec!["src/user.ts".to_string(), "vendor/lib/user.ts".to_string()]
        );
    }

    #[test]
    fn exact_stem_bonus_is_decisive() {
        // "user" hits user.ts exactly; user_profile.ts misses the exact-stem
        // bonus, putting the gap over a full point.
        let files = files(&["src/user.ts", "src/user_profile.ts"]);
        let ranked = resolve_file("user", &files);
        match decide(ranked) {
            Resolution::Decisive(best) => assert_eq!(best.path, "src/user.ts"),
            other => panic!("expected Decisive, got {other:?}"),
        }
    }

    #[test]
    fn extra_matched_token_is_decisive() {
        let files = files(&["src/user_controller.ts", "src/user.ts"]);
        let ranked = resolve_file("user controller", &files);
        match decide(ranked) {
            Resolution::Decisive(best) => assert_eq!(best.path, "src/user_controller.ts"),
            other => panic!("expected Decisive, got {other:?}"),
        }
    }

    #[test]
    fn single_match_is_decisive_and_no_match_is_none() {
        let files = files(&["src/invoice.rs", "docs/readme.md"]);
        match decide(resolve_file("invoice", &files)) {
            Resolution::Decisive(best) => assert_eq!(best.path, "src/invoice.rs"),
            other => panic!("expected Decisive, got {other:?}"),
        }
        assert_eq!(
            decide(resolve_file("quantum flux", &files)),
            Resolution::None
        );
        assert_eq!(decide(Vec::new()), Resolution::None);
    }

    #[test]
    fn gap_of_exactly_half_a_point_is_decisive() {
        // The threshold is strictly-less-than: a clean stem-hit
        // differentiator (0.5) must not open the picker.
        let ranked = vec![fm("a/x.rs", 2.0), fm("b/y.rs", 1.5)];
        assert_eq!(decide(ranked), Resolution::Decisive(fm("a/x.rs", 2.0)));
    }

    #[test]
    fn gap_just_under_half_a_point_is_ambiguous() {
        let ranked = vec![fm("a/x.rs", 2.0), fm("b/y.rs", 1.51)];
        assert_eq!(
            ambiguous_paths(decide(ranked)),
            vec!["a/x.rs".to_string(), "b/y.rs".to_string()]
        );
    }

    #[test]
    fn ambiguity_is_anchored_to_the_top_score_not_chained() {
        // 2.7 is within the gap of 3.0; 2.4 is within the gap of 2.7 but not
        // of the top score, so it must not ride along.
        let ranked = vec![fm("a", 3.0), fm("b", 2.7), fm("c", 2.4)];
        assert_eq!(
            ambiguous_paths(decide(ranked)),
            vec!["a".to_string(), "b".to_string()]
        );
    }

    #[test]
    fn ambiguous_set_is_capped_at_five() {
        let files = files(&[
            "a/user.ts",
            "b/user.ts",
            "c/user.ts",
            "d/user.ts",
            "e/user.ts",
            "f/user.ts",
            "g/user.ts",
        ]);
        let ranked = resolve_file("user", &files);
        assert_eq!(
            ambiguous_paths(decide(ranked)),
            vec![
                "a/user.ts".to_string(),
                "b/user.ts".to_string(),
                "c/user.ts".to_string(),
                "d/user.ts".to_string(),
                "e/user.ts".to_string(),
            ]
        );
    }
}
