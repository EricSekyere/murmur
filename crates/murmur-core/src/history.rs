//! Persistent transcription history: a capped, newest-first log of delivered
//! phrases, stored as JSON next to the config file so it survives restarts.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// Hard cap on stored entries. Older phrases are dropped past this; the log is
/// a convenience, not an archive.
const MAX_ENTRIES: usize = 500;

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

    /// Load from disk, falling back to an empty log on any read/parse error so
    /// a corrupt file never blocks startup.
    pub fn load(path: &PathBuf) -> Self {
        match std::fs::read_to_string(path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
            Err(_) => Self::default(),
        }
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

    /// Atomic save: write to a sibling tempfile, then rename.
    pub fn save(&self, path: &PathBuf) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_string(self)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, &content)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
