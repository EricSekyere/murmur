//! Detection and polling logic for the `wait_for_next_dictation` tool.
//!
//! The MCP server and the Murmur app are separate processes sharing the
//! on-disk history log: the app appends each delivered dictation, the server
//! only reads. "Wait for the next dictation" is therefore implemented by
//! capturing a baseline (the newest entry's timestamp when the wait begins)
//! and polling for an entry strictly newer than it. Nothing here starts a
//! recording; the app owns capture via the user's hotkey.
//!
//! The pure comparison ([`newest_since`]) and the poll loop
//! ([`wait_for_new_entry`], generic over its loader and sleeper) are kept
//! separate from real timing so tests never sleep on wall-clock time.

use murmur_core::history::HistoryEntry;
use std::future::Future;
use std::time::Duration;

pub(crate) const DEFAULT_TIMEOUT_SECS: u64 = 30;
pub(crate) const MAX_TIMEOUT_SECS: u64 = 300;

const POLL_INTERVAL_MS: u64 = 500;
pub(crate) const POLL_INTERVAL: Duration = Duration::from_millis(POLL_INTERVAL_MS);

/// Clamp a requested timeout to `[1, MAX_TIMEOUT_SECS]`, defaulting when absent.
pub(crate) fn clamp_timeout(requested: Option<u64>) -> u64 {
    requested
        .unwrap_or(DEFAULT_TIMEOUT_SECS)
        .clamp(1, MAX_TIMEOUT_SECS)
}

/// Number of sleeps that cover `timeout_secs` at [`POLL_INTERVAL`]. The loop
/// checks once more than it sleeps, so total wall time is about the timeout.
pub(crate) fn polls_for(timeout_secs: u64) -> u64 {
    timeout_secs.saturating_mul(1000) / POLL_INTERVAL_MS
}

/// The newest entry strictly newer than the baseline, if any. A `None`
/// baseline means the history was empty when the wait began, so any entry is
/// new. Selection is by timestamp rather than position so it holds even if
/// the slice ordering ever changes.
pub(crate) fn newest_since(
    entries: &[HistoryEntry],
    baseline_ms: Option<u64>,
) -> Option<&HistoryEntry> {
    entries
        .iter()
        .filter(|e| baseline_ms.is_none_or(|ts| e.timestamp_ms > ts))
        .max_by_key(|e| e.timestamp_ms)
}

/// Poll `load` until it yields an entry newer than `baseline_ms`, sleeping via
/// `sleep` between checks, for at most `max_polls` sleeps. Returns `None` on
/// timeout. The sleeper is injected so tests can pass a ready future instead
/// of real time.
pub(crate) async fn wait_for_new_entry<L, S, Fut>(
    baseline_ms: Option<u64>,
    max_polls: u64,
    mut load: L,
    mut sleep: S,
) -> Option<HistoryEntry>
where
    L: FnMut() -> Vec<HistoryEntry>,
    S: FnMut() -> Fut,
    Fut: Future<Output = ()>,
{
    for poll in 0..=max_polls {
        let entries = load();
        if let Some(entry) = newest_since(&entries, baseline_ms) {
            return Some(entry.clone());
        }
        if poll < max_polls {
            sleep().await;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(text: &str, timestamp_ms: u64) -> HistoryEntry {
        HistoryEntry {
            text: text.into(),
            timestamp_ms,
            app: None,
        }
    }

    #[test]
    fn no_new_entry_returns_none() {
        let entries = vec![entry("old", 100)];
        assert_eq!(newest_since(&entries, Some(100)), None);
        assert_eq!(newest_since(&entries, Some(200)), None);
        assert_eq!(newest_since(&[], Some(100)), None);
        assert_eq!(newest_since(&[], None), None);
    }

    #[test]
    fn a_newer_entry_is_returned() {
        let entries = vec![entry("new", 200), entry("old", 100)];
        assert_eq!(newest_since(&entries, Some(100)), Some(&entries[0]));
    }

    #[test]
    fn empty_baseline_treats_any_entry_as_new() {
        let entries = vec![entry("first", 100)];
        assert_eq!(newest_since(&entries, None), Some(&entries[0]));
    }

    #[test]
    fn multiple_new_entries_returns_the_newest() {
        let entries = vec![entry("newest", 300), entry("mid", 250), entry("old", 100)];
        assert_eq!(newest_since(&entries, Some(100)), Some(&entries[0]));
    }

    #[test]
    fn timeout_clamps_to_sane_range() {
        assert_eq!(clamp_timeout(None), DEFAULT_TIMEOUT_SECS);
        assert_eq!(clamp_timeout(Some(0)), 1);
        assert_eq!(clamp_timeout(Some(5)), 5);
        assert_eq!(clamp_timeout(Some(1_000)), MAX_TIMEOUT_SECS);
    }

    #[test]
    fn poll_budget_covers_the_timeout() {
        assert_eq!(polls_for(30), 60);
        assert_eq!(polls_for(1), 2);
    }

    #[tokio::test]
    async fn times_out_after_the_poll_budget_without_real_sleeps() {
        let mut loads = 0u64;
        let result = wait_for_new_entry(
            Some(100),
            4,
            || {
                loads += 1;
                vec![entry("stale", 100)]
            },
            || std::future::ready(()),
        )
        .await;
        assert_eq!(result, None);
        // One check per sleep plus the initial check: the loop is bounded.
        assert_eq!(loads, 5);
    }

    #[tokio::test]
    async fn returns_the_entry_once_it_appears_mid_wait() {
        let mut loads = 0u64;
        let result = wait_for_new_entry(
            Some(100),
            10,
            || {
                loads += 1;
                if loads >= 3 {
                    vec![entry("answer", 200), entry("stale", 100)]
                } else {
                    vec![entry("stale", 100)]
                }
            },
            || std::future::ready(()),
        )
        .await;
        assert_eq!(result, Some(entry("answer", 200)));
        assert_eq!(loads, 3);
    }
}
