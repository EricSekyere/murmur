//! Codebase-derived vocabulary: a lexical project indexer.
//!
//! Walks a project (gitignore-aware), extracts identifiers, and returns a
//! ranked, budget-capped subset to feed the STT engine's vocabulary so project
//! symbols (`calculateTotalRevenue`, `IndexerSettings`) transcribe correctly.
//!
//! Identifier extraction is AST-accurate via tree-sitter when the `treesitter`
//! feature is on (skipping comments, strings, and keywords), and falls back to
//! a regex scan otherwise. The biasing only helps Whisper; Parakeet exposes no
//! biasing API.

mod extract;
mod rank;
mod resolve;
#[cfg(feature = "treesitter")]
mod treesitter;

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Result, bail};
use ignore::WalkBuilder;

pub use rank::RankedSymbol;
pub use resolve::{FileMatch, resolve_file};

/// Source extensions scanned when [`IndexConfig::extensions`] is empty.
const DEFAULT_EXTENSIONS: &[&str] = &["rs", "ts", "tsx", "js", "jsx", "py", "go", "java"];

/// Tuning for a single index pass.
#[derive(Debug, Clone)]
pub struct IndexConfig {
    /// Hard cap on the number of symbols returned.
    pub max_symbols: usize,
    /// Budget for the joined glossary length (chars), to fit the prompt window.
    pub max_chars: usize,
    /// Extensions to scan; empty means [`DEFAULT_EXTENSIONS`].
    pub extensions: Vec<String>,
    /// Stop after scanning this many files (perf guard on huge trees).
    pub max_files: usize,
    /// Skip files larger than this (generated/minified blobs are noise).
    pub max_file_bytes: u64,
}

impl Default for IndexConfig {
    fn default() -> Self {
        Self {
            max_symbols: 64,
            max_chars: 320,
            extensions: Vec::new(),
            max_files: 5000,
            max_file_bytes: 1 << 20, // 1 MiB
        }
    }
}

/// Walk `root`, extract and rank identifiers, and return the budget-capped
/// symbol list ready for [`crate::stt::engine::SttEngine::set_vocabulary`].
pub fn index_project(root: &Path, cfg: &IndexConfig) -> Result<Vec<String>> {
    Ok(index_project_ranked(root, cfg)?
        .into_iter()
        .map(|s| s.text)
        .collect())
}

/// Like [`index_project`] but keeps the score/frequency, for the CLI and
/// debugging.
pub fn index_project_ranked(root: &Path, cfg: &IndexConfig) -> Result<Vec<RankedSymbol>> {
    if !root.exists() {
        bail!("project root does not exist: {}", root.display());
    }
    let exts = resolve_extensions(cfg);
    let now = SystemTime::now();
    let mut acc = rank::SymbolAccumulator::new();
    let mut scanned = 0usize;
    walk_into(root, &exts, cfg, now, &mut acc, &mut scanned);
    Ok(acc.select(cfg.max_symbols, cfg.max_chars))
}

/// Index several project roots into one shared TF-IDF budget and return the
/// combined symbol list. Missing roots are skipped, so a temporarily
/// unavailable folder doesn't fail the whole pass.
pub fn index_projects(roots: &[PathBuf], cfg: &IndexConfig) -> Result<Vec<String>> {
    let exts = resolve_extensions(cfg);
    let now = SystemTime::now();
    let mut acc = rank::SymbolAccumulator::new();
    let mut scanned = 0usize;
    for root in dedup_roots(roots) {
        if scanned >= cfg.max_files {
            break;
        }
        walk_into(&root, &exts, cfg, now, &mut acc, &mut scanned);
    }
    Ok(acc
        .select(cfg.max_symbols, cfg.max_chars)
        .into_iter()
        .map(|s| s.text)
        .collect())
}

/// Walk the project roots (gitignore-aware, like [`index_projects`]) and
/// return every file as a root-relative, forward-slash path for the spoken
/// file resolver ([`resolve_file`]). Deliberately not filtered by source
/// extension: "open the readme file" should resolve docs and configs too.
/// Deduped and stably sorted; missing roots are skipped.
pub fn index_project_files(roots: &[PathBuf], cfg: &IndexConfig) -> Result<Vec<String>> {
    let mut paths = Vec::new();
    let mut scanned = 0usize;
    for root in dedup_roots(roots) {
        if scanned >= cfg.max_files {
            break;
        }
        collect_relative_paths(&root, cfg, &mut paths, &mut scanned);
    }
    paths.sort_unstable();
    paths.dedup();
    Ok(paths)
}

/// Fold one root's files into `paths`, honoring the shared file budget.
fn collect_relative_paths(
    root: &Path,
    cfg: &IndexConfig,
    paths: &mut Vec<String>,
    scanned: &mut usize,
) {
    let walker = WalkBuilder::new(root)
        .hidden(true)
        .git_global(false)
        .require_git(false)
        .build();
    for entry in walker {
        if *scanned >= cfg.max_files {
            break;
        }
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let Ok(relative) = entry.path().strip_prefix(root) else {
            continue;
        };
        // Non-UTF-8 path components can't be spoken or typed; skip them.
        let Some(joined) = forward_slashed(relative) else {
            continue;
        };
        paths.push(joined);
        *scanned += 1;
    }
}

/// Join a relative path with `/` regardless of platform; `None` on non-UTF-8.
fn forward_slashed(path: &Path) -> Option<String> {
    let parts: Option<Vec<&str>> = path.components().map(|c| c.as_os_str().to_str()).collect();
    parts.map(|p| p.join("/"))
}

/// Canonicalize the roots and drop any that is the same as, or nested under,
/// another, so overlapping or duplicate roots (e.g. `repo` and `repo/src`)
/// don't get their shared files scanned twice — which would inflate the IDF and
/// term frequencies and skew the ranked vocabulary. Missing roots fail to
/// canonicalize and are dropped (index_projects is best-effort).
fn dedup_roots(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut canon: Vec<PathBuf> = roots
        .iter()
        .filter_map(|r| std::fs::canonicalize(r).ok())
        .collect();
    // Shortest first so an ancestor is always kept before its descendants.
    canon.sort_by_key(|p| p.components().count());
    let mut kept: Vec<PathBuf> = Vec::new();
    for root in canon {
        if !kept.iter().any(|k| root.starts_with(k)) {
            kept.push(root);
        }
    }
    kept
}

/// Walk one root (gitignore-aware) and fold its identifiers into `acc`,
/// honoring the shared `scanned`/`max_files` budget.
fn walk_into(
    root: &Path,
    exts: &[String],
    cfg: &IndexConfig,
    now: SystemTime,
    acc: &mut rank::SymbolAccumulator,
    scanned: &mut usize,
) {
    // require_git(false) applies .gitignore even outside a detected repo;
    // git_global(false) keeps the result independent of the user's machine.
    let walker = WalkBuilder::new(root)
        .hidden(true)
        .git_global(false)
        .require_git(false)
        .build();

    for entry in walker {
        if *scanned >= cfg.max_files {
            break;
        }
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        if !has_extension(entry.path(), exts) {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        if meta.len() > cfg.max_file_bytes {
            continue;
        }
        // Non-UTF-8 (binary) files just fail to read and are skipped.
        let Ok(content) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        let weight = rank::recency_weight(file_age_days(&meta, now));
        fold_identifiers(acc, entry.path(), &content, weight);
        *scanned += 1;
    }
}

/// Fold a file's identifiers into `acc`: AST-accurate via tree-sitter when the
/// `treesitter` feature supports the language, otherwise the lexical scan.
#[cfg_attr(not(feature = "treesitter"), allow(unused_variables))]
fn fold_identifiers(acc: &mut rank::SymbolAccumulator, path: &Path, content: &str, weight: f64) {
    #[cfg(feature = "treesitter")]
    if let Some(ext) = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_lowercase)
        && treesitter::supports(&ext)
        && let Some(symbols) = treesitter::extract_symbols(content, &ext)
    {
        acc.add_file(symbols.iter().map(String::as_str), weight);
        return;
    }
    acc.add_file(extract::extract_identifiers(content), weight);
}

/// Source extensions scanned by default. Exposed so a file watcher can decide
/// which changes are worth a re-index.
pub fn default_extensions() -> &'static [&'static str] {
    DEFAULT_EXTENSIONS
}

fn resolve_extensions(cfg: &IndexConfig) -> Vec<String> {
    if cfg.extensions.is_empty() {
        DEFAULT_EXTENSIONS.iter().map(|s| s.to_string()).collect()
    } else {
        cfg.extensions
            .iter()
            .map(|e| e.trim_start_matches('.').to_lowercase())
            .filter(|e| !e.is_empty())
            .collect()
    }
}

fn has_extension(path: &Path, exts: &[String]) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| exts.iter().any(|want| want.eq_ignore_ascii_case(ext)))
}

/// File age in days from its mtime; 0 when unavailable or in the future.
fn file_age_days(meta: &std::fs::Metadata, now: SystemTime) -> f64 {
    meta.modified()
        .ok()
        .and_then(|m| now.duration_since(m).ok())
        .map(|d| d.as_secs_f64() / 86_400.0)
        .unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(dir: &Path, name: &str, body: &str) {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, body).unwrap();
    }

    #[test]
    fn indexes_symbols_and_respects_gitignore() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(root, ".gitignore", "ignored.rs\n");
        write(root, "keep.rs", "fn keptVisibleSymbol() {}");
        write(root, "ignored.rs", "fn secretIgnoredSymbol() {}");
        write(root, "notes.md", "fn markdownSymbol() {}"); // wrong extension

        let symbols = index_project(root, &IndexConfig::default()).unwrap();
        assert!(symbols.iter().any(|s| s == "keptVisibleSymbol"));
        assert!(
            !symbols.iter().any(|s| s == "secretIgnoredSymbol"),
            "gitignored file must be excluded"
        );
        assert!(
            !symbols.iter().any(|s| s == "markdownSymbol"),
            "non-source extension must be excluded"
        );
    }

    #[test]
    fn missing_root_errors() {
        let err = index_project(Path::new("definitely/not/here"), &IndexConfig::default());
        assert!(err.is_err());
    }

    #[test]
    fn dedup_roots_drops_nested_and_duplicate_roots() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join("src")).unwrap();
        let sub = root.join("src");

        // A duplicate and a descendant both collapse to the single ancestor.
        let kept = dedup_roots(&[root.to_path_buf(), sub.clone(), root.to_path_buf()]);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0], fs::canonicalize(root).unwrap());

        // Two unrelated roots are both kept; a missing root is dropped.
        let other = tempfile::tempdir().unwrap();
        let kept = dedup_roots(&[
            root.to_path_buf(),
            other.path().to_path_buf(),
            root.join("does-not-exist"),
        ]);
        assert_eq!(kept.len(), 2);
    }

    #[test]
    fn file_index_returns_relative_slash_normalized_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(root, "src/nested/deep.rs", "fn deep() {}");
        write(root, "README.md", "# docs");

        let files = index_project_files(&[root.to_path_buf()], &IndexConfig::default()).unwrap();
        assert!(files.contains(&"src/nested/deep.rs".to_string()));
        // No extension filter: docs and configs are resolvable by voice too.
        assert!(files.contains(&"README.md".to_string()));
        assert!(
            files.iter().all(|f| !f.contains('\\')),
            "paths must be forward-slash normalized: {files:?}"
        );
    }

    #[test]
    fn file_index_respects_gitignore() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(root, ".gitignore", "ignored.rs\nsecret/\n");
        write(root, "keep.rs", "fn kept() {}");
        write(root, "ignored.rs", "fn hidden() {}");
        write(root, "secret/inner.rs", "fn hidden() {}");

        let files = index_project_files(&[root.to_path_buf()], &IndexConfig::default()).unwrap();
        assert!(files.contains(&"keep.rs".to_string()));
        assert!(!files.contains(&"ignored.rs".to_string()));
        assert!(
            !files.iter().any(|f| f.starts_with("secret/")),
            "gitignored directory must be excluded: {files:?}"
        );
    }

    #[test]
    fn file_index_dedups_nested_roots_and_honors_file_cap() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(root, "src/lib.rs", "fn a() {}");
        write(root, "src/util.rs", "fn b() {}");

        // A nested root collapses into its ancestor: each file listed once.
        let files = index_project_files(
            &[root.to_path_buf(), root.join("src")],
            &IndexConfig::default(),
        )
        .unwrap();
        assert_eq!(
            files,
            vec!["src/lib.rs".to_string(), "src/util.rs".to_string()]
        );

        let capped = index_project_files(
            &[root.to_path_buf()],
            &IndexConfig {
                max_files: 1,
                ..IndexConfig::default()
            },
        )
        .unwrap();
        assert_eq!(capped.len(), 1);
    }

    #[test]
    fn honors_symbol_budget() {
        let tmp = tempfile::tempdir().unwrap();
        let mut body = String::new();
        for i in 0..100 {
            body.push_str(&format!("fn handlerFunction{i}() {{}}\n"));
        }
        write(tmp.path(), "lib.rs", &body);
        let cfg = IndexConfig {
            max_symbols: 5,
            ..IndexConfig::default()
        };
        assert_eq!(index_project(tmp.path(), &cfg).unwrap().len(), 5);
    }
}
