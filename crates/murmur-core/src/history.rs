//! Persistent transcription history: a capped, newest-first log of delivered
//! phrases, stored as JSON next to the config file so it survives restarts.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// Hard cap on stored entries. Older phrases are dropped past this; the log is
/// a convenience, not an archive.
const MAX_ENTRIES: usize = 500;

const MS_PER_DAY: u64 = 86_400_000;

/// Per-application usage tally.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AppUsage {
    pub app: String,
    pub phrases: usize,
    pub words: usize,
}

/// On-device usage stats derived entirely from the local history log.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct UsageStats {
    pub total_phrases: usize,
    pub total_words: usize,
    pub words_this_week: usize,
    /// Consecutive days with at least one phrase, ending today (or yesterday if
    /// nothing yet today, so the streak isn't "lost" before the day is over).
    pub day_streak: u32,
    /// Top apps by phrase count (max 5).
    pub top_apps: Vec<AppUsage>,
    /// Phrases per weekday, index 0 = Sunday.
    pub by_weekday: [usize; 7],
    /// Words per calendar day for the last 21 days, oldest first (last entry =
    /// today). Drives the analytics sparkline and streak grid.
    pub daily_words: Vec<usize>,
}

/// One delivered phrase.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HistoryEntry {
    pub text: String,
    /// Unix epoch milliseconds when the phrase was delivered.
    pub timestamp_ms: u64,
    /// Foreground application the text was delivered to, when known.
    #[serde(default)]
    pub app: Option<String>,
}

/// Newest-first, capped transcription log persisted to disk.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct History {
    entries: Vec<HistoryEntry>,
}

impl History {
    /// Default history file path (`<config dir>/murmur/history.json`).
    pub fn default_path() -> Result<PathBuf> {
        let dir = dirs::config_dir()
            .ok_or_else(|| anyhow::anyhow!("Could not determine config directory"))?
            .join("murmur");
        Ok(dir.join("history.json"))
    }

    /// Load from disk; on a read/parse error fall back to empty. A file that
    /// exists but fails to parse is backed up to `history.json.bak` first.
    pub fn load(path: &PathBuf) -> Self {
        let content = match std::fs::read_to_string(path) {
            Ok(content) => content,
            Err(_) => return Self::default(),
        };
        match serde_json::from_str(&content) {
            Ok(history) => history,
            Err(e) => {
                let backup = path.with_extension("json.bak");
                tracing::warn!(
                    "History at {} is unreadable ({}); backing it up to {} and starting fresh",
                    path.display(),
                    e,
                    backup.display()
                );
                let _ = std::fs::rename(path, &backup);
                Self::default()
            }
        }
    }

    /// Load without side effects: a missing or unparseable file yields empty
    /// and is never renamed or rewritten. For read-only consumers like the
    /// MCP server, whose recovery writes would race the owning app.
    pub fn load_readonly(path: &PathBuf) -> Self {
        let Ok(content) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        serde_json::from_str(&content).unwrap_or_else(|e| {
            tracing::warn!(
                "History at {} is unreadable ({}); reading as empty without modifying it",
                path.display(),
                e
            );
            Self::default()
        })
    }

    /// Prepend a new entry, dropping the oldest beyond the cap.
    pub fn add(&mut self, text: &str, app: Option<String>) {
        self.entries.insert(
            0,
            HistoryEntry {
                text: text.to_string(),
                timestamp_ms: now_ms(),
                app,
            },
        );
        self.entries.truncate(MAX_ENTRIES);
    }

    /// Up to `limit` most-recent entries whose text contains `query`
    /// (case-insensitive). An empty query matches everything.
    pub fn search(&self, query: &str, limit: usize) -> Vec<HistoryEntry> {
        let needle = query.trim().to_lowercase();
        self.entries
            .iter()
            .filter(|e| needle.is_empty() || e.text.to_lowercase().contains(&needle))
            .take(limit)
            .cloned()
            .collect()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Mine the history for distinctive technical terms the user has dictated
    /// more than once (camelCase, snake_case, or digit-bearing identifiers) that
    /// aren't already in `existing`, newest-weighted, capped at `max`. These can
    /// be added to the custom vocabulary so the decoder biases toward them.
    /// Plain capitalized words are skipped: without a stoplist they're mostly
    /// sentence-initial noise rather than proper nouns.
    pub fn learn_terms(&self, existing: &[String], max: usize) -> Vec<String> {
        if max == 0 {
            return Vec::new();
        }
        let have: std::collections::HashSet<String> =
            existing.iter().map(|w| w.to_lowercase()).collect();
        let mut counts: HashMap<&str, usize> = HashMap::new();
        for entry in &self.entries {
            for tok in entry
                .text
                .split(|c: char| !(c.is_alphanumeric() || c == '_'))
            {
                if is_distinctive_term(tok) && !have.contains(&tok.to_lowercase()) {
                    *counts.entry(tok).or_default() += 1;
                }
            }
        }
        let mut candidates: Vec<(&str, usize)> =
            counts.into_iter().filter(|(_, n)| *n >= 2).collect();
        // Most-repeated first, then alphabetical for a stable order.
        candidates.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        candidates
            .into_iter()
            .take(max)
            .map(|(t, _)| t.to_string())
            .collect()
    }

    /// Aggregate the stored history into usage stats. `now_ms` is passed in so
    /// the computation stays clock-free here. All UTC-day based, which can be
    /// off by a day from local time near midnight — fine for a usage summary.
    pub fn stats(&self, now_ms: u64) -> UsageStats {
        let week_ago = now_ms.saturating_sub(7 * MS_PER_DAY);
        let today = now_ms / MS_PER_DAY;

        let mut total_words = 0usize;
        let mut words_this_week = 0usize;
        let mut by_weekday = [0usize; 7];
        let mut active_days: BTreeSet<u64> = BTreeSet::new();
        let mut words_by_day: HashMap<u64, usize> = HashMap::new();
        let mut per_app: HashMap<&str, (usize, usize)> = HashMap::new();

        for e in &self.entries {
            let words = e.text.split_whitespace().count();
            total_words += words;
            if e.timestamp_ms >= week_ago {
                words_this_week += words;
            }
            let day = e.timestamp_ms / MS_PER_DAY;
            active_days.insert(day);
            *words_by_day.entry(day).or_default() += words;
            // 1970-01-01 was a Thursday → index 4 with Sunday = 0.
            by_weekday[(((day % 7) + 4) % 7) as usize] += 1;
            if let Some(app) = e.app.as_deref() {
                let slot = per_app.entry(app).or_default();
                slot.0 += 1;
                slot.1 += words;
            }
        }

        // Streak: count back from today (or yesterday if today is still empty).
        let mut day_streak = 0u32;
        let mut d = if active_days.contains(&today) {
            today
        } else {
            today.saturating_sub(1)
        };
        while active_days.contains(&d) {
            day_streak += 1;
            if d == 0 {
                break;
            }
            d -= 1;
        }

        let mut top_apps: Vec<AppUsage> = per_app
            .into_iter()
            .map(|(app, (phrases, words))| AppUsage {
                app: app.to_string(),
                phrases,
                words,
            })
            .collect();
        top_apps.sort_by(|a, b| b.phrases.cmp(&a.phrases).then(b.words.cmp(&a.words)));
        top_apps.truncate(5);

        const DAILY_WINDOW: u64 = 21;
        let daily_words: Vec<usize> = (0..DAILY_WINDOW)
            .map(|i| {
                let day = today.saturating_sub(DAILY_WINDOW - 1 - i);
                words_by_day.get(&day).copied().unwrap_or(0)
            })
            .collect();

        UsageStats {
            total_phrases: self.entries.len(),
            total_words,
            words_this_week,
            day_streak,
            top_apps,
            by_weekday,
            daily_words,
        }
    }

    /// Atomic save: write to a unique sibling tempfile, then rename.
    pub fn save(&self, path: &std::path::Path) -> Result<()> {
        let content = serde_json::to_string(self)?;
        crate::fsutil::atomic_write(path, content.as_bytes())?;
        Ok(())
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Whether a token looks like a technical identifier worth learning: 3–40 chars,
/// starting with a letter or `_`, and carrying an interior capital, a digit, or
/// an underscore (camelCase / PascalCase / snake_case / `oauth2`). Plain words
/// and lone capitalized words are excluded.
fn is_distinctive_term(tok: &str) -> bool {
    let len = tok.chars().count();
    if !(3..=40).contains(&len) {
        return false;
    }
    if !tok
        .chars()
        .next()
        .is_some_and(|c| c.is_alphabetic() || c == '_')
    {
        return false;
    }
    let interior_upper = tok.chars().skip(1).any(|c| c.is_uppercase());
    let has_digit = tok.chars().any(|c| c.is_ascii_digit());
    interior_upper || has_digit || tok.contains('_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_readonly_never_touches_a_corrupt_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("history.json");
        std::fs::write(&path, "{not json").expect("write");

        let history = History::load_readonly(&path);
        assert!(history.entries.is_empty());
        // Unlike load(), nothing on disk may change: no .bak rename.
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "{not json");
        assert!(!path.with_extension("json.bak").exists());
    }

    #[test]
    fn add_prepends_and_caps() {
        let mut h = History::default();
        for i in 0..(MAX_ENTRIES + 10) {
            h.add(&format!("phrase {i}"), None);
        }
        assert_eq!(h.entries.len(), MAX_ENTRIES);
        // Newest first.
        assert_eq!(h.entries[0].text, format!("phrase {}", MAX_ENTRIES + 9));
    }

    #[test]
    fn search_is_case_insensitive_substring() {
        let mut h = History::default();
        h.add("Deploy the server", None);
        h.add("Order more coffee", None);
        let hits = h.search("SERVER", 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].text, "Deploy the server");
        assert_eq!(h.search("", 10).len(), 2);
        assert_eq!(h.search("nonsense", 10).len(), 0);
    }

    #[test]
    fn stats_aggregate_words_apps_and_streak() {
        let day = MS_PER_DAY;
        let now = 100 * day + 5_000;
        let mut h = History::default();
        let push = |h: &mut History, text: &str, ts: u64, app: Option<&str>| {
            h.entries.push(HistoryEntry {
                text: text.to_string(),
                timestamp_ms: ts,
                app: app.map(str::to_string),
            });
        };
        push(&mut h, "hello world", now - 1_000, Some("warp.exe")); // today
        push(&mut h, "three little words", now - 2_000, Some("warp.exe")); // today
        push(&mut h, "one", now - day, Some("chrome.exe")); // yesterday
        push(&mut h, "old phrase here", now - 10 * day, None); // outside the week

        let s = h.stats(now);
        assert_eq!(s.total_phrases, 4);
        assert_eq!(s.total_words, 2 + 3 + 1 + 3);
        assert_eq!(s.words_this_week, 2 + 3 + 1);
        assert_eq!(s.day_streak, 2); // today + yesterday, broken before that
        assert_eq!(s.top_apps[0].app, "warp.exe");
        assert_eq!(s.top_apps[0].phrases, 2);
    }

    #[test]
    fn learn_terms_picks_repeated_identifiers_only() {
        let mut h = History::default();
        h.add("call useEffect and then useEffect again", None);
        h.add("the useEffect hook with oauth2 and oauth2", None);
        h.add("just some ordinary words here words", None); // plain words ignored
        let learned = h.learn_terms(&[], 10);
        assert!(learned.contains(&"useEffect".to_string()), "{learned:?}");
        assert!(learned.contains(&"oauth2".to_string()), "{learned:?}");
        // Plain repeated words ("words") are not learned.
        assert!(!learned.iter().any(|t| t == "words"), "{learned:?}");
        // Already-known terms are skipped.
        let none = h.learn_terms(&["useEffect".to_string(), "oauth2".to_string()], 10);
        assert!(none.is_empty(), "{none:?}");
    }
}
