//! Cross-process dictation trigger: the MCP server's `request_dictation` tool
//! writes a small request file, and the running app polls for it and starts a
//! recording session. Both processes depend on this module, so the file path
//! and schema can never disagree. The spoken result never travels through this
//! file — it comes back via the shared history log.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Requests older than this are ignored, so a trigger left behind by a
/// crashed or offline app never auto-starts recording on a later launch.
pub const MAX_AGE_MS: u64 = 300_000;

/// One request to start a dictation session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DictationRequest {
    /// Unix epoch milliseconds when the request was made.
    #[serde(default)]
    pub requested_ms: u64,
    /// Optional short question the agent wants shown to the user.
    #[serde(default)]
    pub prompt: Option<String>,
}

/// Default trigger file path (`<config dir>/murmur/dictation-request.json`).
pub fn default_path() -> Result<PathBuf> {
    let dir = dirs::config_dir()
        .ok_or_else(|| anyhow::anyhow!("Could not determine config directory"))?
        .join("murmur");
    Ok(dir.join("dictation-request.json"))
}

/// Write the trigger atomically (tempfile + rename), so the polling app can
/// never observe a partially written request.
pub fn write(path: &Path, req: &DictationRequest) -> Result<()> {
    let content = serde_json::to_string(req)?;
    crate::fsutil::atomic_write(path, content.as_bytes())?;
    Ok(())
}

/// Read and consume the trigger: parse, then delete (consume-once). A missing
/// or unreadable file yields `None`; an unparseable file is still deleted so a
/// corrupt trigger cannot wedge the poller.
pub fn take(path: &Path) -> Option<DictationRequest> {
    let content = std::fs::read_to_string(path).ok()?;
    let parsed = serde_json::from_str(&content);
    clear(path);
    match parsed {
        Ok(req) => Some(req),
        Err(e) => {
            tracing::warn!(
                "Dictation trigger at {} is unparseable ({}); discarded",
                path.display(),
                e
            );
            None
        }
    }
}

/// Best-effort delete for startup cleanup and abandoned requests. A missing
/// file is the normal case, not an error.
pub fn clear(path: &Path) {
    if let Err(e) = std::fs::remove_file(path)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(
            "Failed to remove dictation trigger at {}: {}",
            path.display(),
            e
        );
    }
}

/// Whether the request is recent enough to act on (see [`MAX_AGE_MS`]).
pub fn is_fresh(req: &DictationRequest, now_ms: u64) -> bool {
    now_ms.saturating_sub(req.requested_ms) <= MAX_AGE_MS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_then_take_round_trips_and_deletes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("dictation-request.json");
        let req = DictationRequest {
            requested_ms: 1_234,
            prompt: Some("which branch?".to_string()),
        };
        write(&path, &req).expect("write");

        let taken = take(&path).expect("trigger must be present");
        assert_eq!(taken.requested_ms, 1_234);
        assert_eq!(taken.prompt.as_deref(), Some("which branch?"));
        // Consume-once: the file is gone and a second take yields nothing.
        assert!(!path.exists());
        assert!(take(&path).is_none());
    }

    #[test]
    fn take_on_a_missing_path_is_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(take(&dir.path().join("absent.json")).is_none());
    }

    #[test]
    fn take_deletes_a_corrupt_trigger_and_returns_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("dictation-request.json");
        std::fs::write(&path, "{not json").expect("write");

        assert!(take(&path).is_none());
        // The corrupt file must not linger and re-trip every poll.
        assert!(!path.exists());
    }

    #[test]
    fn clear_ignores_a_missing_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        clear(&dir.path().join("absent.json"));
    }

    #[test]
    fn is_fresh_bounds_the_request_age() {
        let now = 10 * MAX_AGE_MS;
        let at_limit = DictationRequest {
            requested_ms: now - MAX_AGE_MS,
            prompt: None,
        };
        assert!(is_fresh(&at_limit, now));
        let just_over = DictationRequest {
            requested_ms: now - MAX_AGE_MS - 1,
            prompt: None,
        };
        assert!(!is_fresh(&just_over, now));
        // A future timestamp (clock skew between processes) counts as fresh.
        let future = DictationRequest {
            requested_ms: now + 1_000,
            prompt: None,
        };
        assert!(is_fresh(&future, now));
    }
}
