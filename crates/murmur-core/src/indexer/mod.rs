//! Codebase-derived vocabulary: a lexical project indexer.
//!
//! Walks a project (gitignore-aware), extracts identifiers, and returns a
//! ranked, budget-capped subset to feed the STT engine's vocabulary so project
//! symbols (`calculateTotalRevenue`, `IndexerSettings`) transcribe correctly.
//!
//! This is the cheap MVP: a regex + frequency scan, no AST. Tree-sitter is a
//! later accuracy upgrade. The biasing only helps Whisper; Parakeet exposes no
//! biasing API.

mod extract;
mod rank;

use std::path::Path;
use std::time::SystemTime;

use anyhow::{Result, bail};
use ignore::WalkBuilder;

pub use rank::RankedSymbol;

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

    // require_git(false) applies .gitignore even outside a detected repo;
    // git_global(false) keeps the result independent of the user's machine.
    let walker = WalkBuilder::new(root)
        .hidden(true)
        .git_global(false)
        .require_git(false)
        .build();

    for entry in walker {
        if scanned >= cfg.max_files {
            break;
        }
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        if !has_extension(entry.path(), &exts) {
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
        acc.add_file(extract::extract_identifiers(&content), weight);
        scanned += 1;
    }

    Ok(acc.select(cfg.max_symbols, cfg.max_chars))
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
