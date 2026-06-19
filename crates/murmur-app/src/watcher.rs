//! File-system watcher that re-indexes the codebase vocabulary when a project's
//! source files change. Debounced so a burst of saves (or a build) triggers a
//! single re-index, and filtered to source files outside churny build dirs.

use std::path::{Path, PathBuf};
use std::time::Duration;

use notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_mini::{DebounceEventResult, Debouncer, new_debouncer};
use tauri::Manager;

use crate::state::AppState;

/// Quiet period after the last change before re-indexing.
const DEBOUNCE: Duration = Duration::from_secs(2);

/// Directories whose churn should never trigger a re-index.
const IGNORED_DIRS: &[&str] = &[
    ".git",
    "target",
    "node_modules",
    "dist",
    "build",
    ".venv",
    "__pycache__",
    ".next",
];

/// Re-point the watcher at the current project roots, or stop it when the
/// indexer is disabled or has no roots. Replaces any existing watcher.
pub(crate) fn rewatch(app: &tauri::AppHandle) {
    let state = app.state::<AppState>();
    let (enabled, roots) = {
        let settings = state.settings.lock().unwrap_or_else(|e| e.into_inner());
        (
            settings.indexer.enabled,
            settings.indexer.project_roots.clone(),
        )
    };

    let mut guard = state
        .codebase_watcher
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if !enabled || roots.is_empty() {
        *guard = None; // drop -> stop watching
        return;
    }

    match build_watcher(app.clone(), &roots) {
        Ok(debouncer) => *guard = Some(debouncer),
        Err(e) => {
            tracing::warn!("Codebase watcher failed to start: {e}");
            *guard = None;
        }
    }
}

fn build_watcher(
    app: tauri::AppHandle,
    roots: &[PathBuf],
) -> Result<Debouncer<RecommendedWatcher>, notify::Error> {
    let mut debouncer = new_debouncer(DEBOUNCE, move |res: DebounceEventResult| {
        if let Ok(events) = res
            && events.iter().any(|e| is_relevant(&e.path))
        {
            tracing::debug!("Codebase change detected; re-indexing");
            crate::spawn_project_index(app.clone());
        }
    })?;
    for root in roots {
        if let Err(e) = debouncer.watcher().watch(root, RecursiveMode::Recursive) {
            tracing::warn!("Could not watch {}: {e}", root.display());
        }
    }
    Ok(debouncer)
}

/// Whether a changed path is a source file worth re-indexing for.
fn is_relevant(path: &Path) -> bool {
    if path.components().any(|c| {
        c.as_os_str()
            .to_str()
            .is_some_and(|s| IGNORED_DIRS.contains(&s))
    }) {
        return false;
    }
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return false;
    };
    let ext = ext.to_lowercase();
    murmur_core::indexer::default_extensions().contains(&ext.as_str())
}
