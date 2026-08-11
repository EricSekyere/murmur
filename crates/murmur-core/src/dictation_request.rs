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

/// Default trigger file path (`<config base>/murmur/dictation-request.json`).
pub fn default_path() -> Result<PathBuf> {
    let base = crate::fsutil::config_base_dir()
        .ok_or_else(|| anyhow::anyhow!("Could not determine config directory"))?;
    Ok(default_path_in(&base))
}

/// The trigger file path under an explicit config base directory.
pub fn default_path_in(config_base: &Path) -> PathBuf {
    config_base.join("murmur").join("dictation-request.json")
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
    // Read raw bytes and clear before any decode: a UTF-8 failure inside
    // read_to_string would return early and leave the file in place, so the
    // poller would re-read and re-fail on it every tick forever. Our own
    // writer renames a tempfile into place and cannot leave partial bytes, so
    // the vector is a foreign or hand-edited file, not a torn write.
    let bytes = std::fs::read(path).ok()?;
    clear(path);
    match serde_json::from_slice(&bytes) {
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

/// Delete the trigger only if it is still the one stamped `requested_ms`.
///
/// A request must clean up after itself without disarming a different
/// request: the trigger path is shared, so two clients (or two editors) can
/// each have one outstanding, and a blind delete would cancel the other's.
/// A trigger already consumed by the app is gone, which is a no-op here.
pub fn clear_if_stamped(path: &Path, requested_ms: u64) {
    let Ok(bytes) = std::fs::read(path) else {
        return;
    };
    match serde_json::from_slice::<DictationRequest>(&bytes) {
        Ok(req) if req.requested_ms == requested_ms => clear(path),
        // Someone else's trigger, or an unreadable one that take() will
        // consume and discard on its next poll. Either way, not ours to delete.
        _ => {}
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
    fn take_deletes_a_non_utf8_trigger_and_returns_none() {
        // The MCP server writes this file from another process; a killed or
        // truncated write can leave bytes that are not valid UTF-8. Decoding
        // before the delete would leave the file in place and re-fail on
        // every poll tick forever.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("dictation-request.json");
        std::fs::write(&path, [0xFF, 0x00, 0xFE]).expect("write");

        assert!(take(&path).is_none());
        assert!(!path.exists());
    }

    #[test]
    fn clear_ignores_a_missing_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        clear(&dir.path().join("absent.json"));
    }

    #[test]
    fn clear_if_stamped_only_removes_its_own_trigger() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("dictation-request.json");
        write(
            &path,
            &DictationRequest {
                requested_ms: 2_000,
                prompt: None,
            },
        )
        .expect("write");

        // A different request's cleanup must not disarm this one: the trigger
        // path is shared, so two clients can each have one outstanding.
        clear_if_stamped(&path, 1_000);
        assert!(path.exists(), "another request's trigger must survive");

        clear_if_stamped(&path, 2_000);
        assert!(!path.exists(), "its own trigger must be retired");
    }

    #[test]
    fn clear_if_stamped_tolerates_missing_and_unreadable_triggers() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("dictation-request.json");
        // Already consumed by the app: nothing to do, and no panic.
        clear_if_stamped(&path, 1_000);

        // Non-UTF-8 bytes are not ours to judge; take() consumes them.
        std::fs::write(&path, [0xFF, 0x00]).expect("write");
        clear_if_stamped(&path, 1_000);
        assert!(path.exists());
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
