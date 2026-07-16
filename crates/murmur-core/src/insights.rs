//! Persistent per-day dictation aggregate: one row per active day, kept
//! alongside the history log. Unlike history it survives the 500-entry cap,
//! so personal records (and a future calendar heatmap) can cover all time.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::history::History;

// history.rs keeps its own copy; a duplicated one-line const beats coupling
// the two modules for it.
const MS_PER_DAY: u64 = 86_400_000;

/// Hard cap on stored rows (~2 years of daily activity). Oldest rows are
/// dropped past this so the file never grows unbounded.
const MAX_DAYS: usize = 730;

/// One active day's totals. Unix day index = timestamp_ms / MS_PER_DAY (UTC),
/// consistent with `history::stats`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct DailyStat {
    #[serde(default)]
    day: u64,
    #[serde(default)]
    words: usize,
    #[serde(default)]
    phrases: usize,
    /// Filler occurrences (see [`crate::filler`]) — a count only; which
    /// fillers matched is never kept.
    #[serde(default)]
    filler_count: usize,
}

/// All-time personal records derived from the per-day aggregate.
// TODO(follow-up): best-ever WPM lands once a session-finalize backend hook exists.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Records {
    /// Most words dictated in a single day (0 when no data).
    pub best_day_words: usize,
    /// Unix day index of that best day (0 when no data).
    pub best_day: u64,
    /// Longest run of consecutive active days.
    pub longest_streak: u32,
    /// Weekday with the most total words, 0 = Sunday (0 when no data).
    pub most_active_weekday: u8,
    /// Active days covered by the aggregate; drives the "records reflect data
    /// from here forward" UI note.
    pub tracked_days: usize,
}

/// One active day's word total, for the activity heatmap.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DayActivity {
    pub day: u64,
    pub words: usize,
}

/// Per-day totals persisted to disk, sorted by `day` ascending with one row
/// per active day.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Insights {
    days: Vec<DailyStat>,
}

impl Insights {
    /// Default aggregate file path (`<config dir>/murmur/insights.json`).
    pub fn default_path() -> Result<PathBuf> {
        let dir = dirs::config_dir()
            .ok_or_else(|| anyhow::anyhow!("Could not determine config directory"))?
            .join("murmur");
        Ok(dir.join("insights.json"))
    }

    /// Load from disk; on a read/parse error fall back to empty. A file that
    /// exists but fails to parse is backed up to `insights.json.bak` first.
    pub fn load(path: &Path) -> Self {
        let content = match std::fs::read_to_string(path) {
            Ok(content) => content,
            Err(_) => return Self::default(),
        };
        match serde_json::from_str(&content) {
            Ok(insights) => insights,
            Err(e) => {
                let backup = path.with_extension("json.bak");
                tracing::warn!(
                    "Insights at {} are unreadable ({}); backing them up to {} and starting fresh",
                    path.display(),
                    e,
                    backup.display()
                );
                let _ = std::fs::rename(path, &backup);
                Self::default()
            }
        }
    }

    /// Atomic save: write to a unique sibling tempfile, then rename.
    pub fn save(&self, path: &Path) -> Result<()> {
        let content = serde_json::to_string(self)?;
        crate::fsutil::atomic_write(path, content.as_bytes())?;
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.days.is_empty()
    }

    /// Fold one delivered phrase into its day's row, inserting the row if the
    /// day is new. Past `MAX_DAYS` rows the oldest days are dropped.
    pub fn record(&mut self, text: &str, timestamp_ms: u64) {
        let day = timestamp_ms / MS_PER_DAY;
        let idx = match self.days.binary_search_by_key(&day, |d| d.day) {
            Ok(idx) => idx,
            Err(idx) => {
                self.days.insert(
                    idx,
                    DailyStat {
                        day,
                        words: 0,
                        phrases: 0,
                        filler_count: 0,
                    },
                );
                idx
            }
        };
        let stat = &mut self.days[idx];
        stat.words += text.split_whitespace().count();
        stat.phrases += 1;
        stat.filler_count += crate::filler::count_fillers(text);
        if self.days.len() > MAX_DAYS {
            let excess = self.days.len() - MAX_DAYS;
            self.days.drain(..excess);
        }
    }

    /// One-time seeding from the (capped) history log, so a fresh aggregate
    /// starts with the recent past instead of empty. The caller guards this
    /// with [`Self::is_empty`].
    pub fn backfill_from_history(&mut self, history: &History) {
        for entry in history.entries() {
            self.record(&entry.text, entry.timestamp_ms);
        }
    }

    /// Per-day word totals for the activity heatmap, sorted by `day`
    /// ascending. Only active days are returned; the frontend fills the gaps.
    pub fn daily_activity(&self) -> Vec<DayActivity> {
        self.days
            .iter()
            .map(|stat| DayActivity {
                day: stat.day,
                words: stat.words,
            })
            .collect()
    }

    /// Derive the personal records from the stored rows.
    pub fn records(&self) -> Records {
        let mut best_day_words = 0usize;
        let mut best_day = 0u64;
        let mut words_by_weekday = [0usize; 7];
        let mut longest_streak = 0u32;
        let mut run = 0u32;
        let mut prev_day: Option<u64> = None;

        for stat in &self.days {
            if stat.words > best_day_words {
                best_day_words = stat.words;
                best_day = stat.day;
            }
            // 1970-01-01 was a Thursday → index 4 with Sunday = 0.
            words_by_weekday[(((stat.day % 7) + 4) % 7) as usize] += stat.words;
            run = match prev_day {
                Some(prev) if stat.day == prev + 1 => run + 1,
                _ => 1,
            };
            longest_streak = longest_streak.max(run);
            prev_day = Some(stat.day);
        }

        // Earliest weekday wins ties, so an empty aggregate yields Sunday (0).
        let most_active_weekday = words_by_weekday
            .iter()
            .enumerate()
            .max_by_key(|&(idx, &words)| (words, std::cmp::Reverse(idx)))
            .map(|(idx, _)| idx as u8)
            .unwrap_or(0);

        Records {
            best_day_words,
            best_day,
            longest_streak,
            most_active_weekday,
            tracked_days: self.days.len(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_accumulates_into_the_right_day() {
        let mut insights = Insights::default();
        insights.record("um hello you know world", 100 * MS_PER_DAY + 5_000);

        assert_eq!(insights.days.len(), 1);
        let stat = &insights.days[0];
        assert_eq!(stat.day, 100);
        assert_eq!(stat.words, 5);
        assert_eq!(stat.phrases, 1);
        assert_eq!(stat.filler_count, 2); // "um" + "you know"
    }

    #[test]
    fn same_day_phrases_merge_into_one_row() {
        let mut insights = Insights::default();
        insights.record("hello world", 100 * MS_PER_DAY + 5_000);
        insights.record("three little words", 100 * MS_PER_DAY + 9_000);

        assert_eq!(insights.days.len(), 1);
        assert_eq!(insights.days[0].words, 5);
        assert_eq!(insights.days[0].phrases, 2);
    }

    #[test]
    fn different_days_make_separate_sorted_rows() {
        let mut insights = Insights::default();
        insights.record("later day", 101 * MS_PER_DAY + 5_000);
        insights.record("earlier day", 100 * MS_PER_DAY + 5_000);

        assert_eq!(insights.days.len(), 2);
        assert_eq!(insights.days[0].day, 100);
        assert_eq!(insights.days[1].day, 101);
    }

    #[test]
    fn records_compute_best_day_streak_weekday_and_tracked_days() {
        let mut insights = Insights::default();
        // Days 100–102 are consecutive; the gap before 105 breaks the run.
        insights.record("one", 100 * MS_PER_DAY + 5_000);
        insights.record("two words", 101 * MS_PER_DAY + 5_000);
        insights.record("a b c d e", 102 * MS_PER_DAY + 5_000);
        insights.record("after the gap", 105 * MS_PER_DAY + 5_000);

        let r = insights.records();
        assert_eq!(r.best_day_words, 5);
        assert_eq!(r.best_day, 102);
        assert_eq!(r.longest_streak, 3);
        // Day 102: (((102 % 7) + 4) % 7) = 1 → Monday carried the most words.
        assert_eq!(r.most_active_weekday, 1);
        assert_eq!(r.tracked_days, 4);
    }

    #[test]
    fn daily_activity_returns_sorted_word_totals_per_day() {
        let mut insights = Insights::default();
        insights.record("after the gap", 105 * MS_PER_DAY + 5_000);
        insights.record("hello world", 100 * MS_PER_DAY + 5_000);
        insights.record("three little words", 100 * MS_PER_DAY + 9_000);

        assert_eq!(
            insights.daily_activity(),
            vec![
                DayActivity { day: 100, words: 5 },
                DayActivity { day: 105, words: 3 },
            ]
        );
    }

    #[test]
    fn empty_insights_yield_empty_daily_activity() {
        assert!(Insights::default().daily_activity().is_empty());
    }

    #[test]
    fn empty_insights_yield_all_zero_records() {
        let r = Insights::default().records();
        assert_eq!(
            r,
            Records {
                best_day_words: 0,
                best_day: 0,
                longest_streak: 0,
                most_active_weekday: 0,
                tracked_days: 0,
            }
        );
    }

    #[test]
    fn cap_drops_the_oldest_days() {
        let mut insights = Insights::default();
        for day in 0..(MAX_DAYS as u64 + 10) {
            insights.record("hello", day * MS_PER_DAY + 5_000);
        }
        assert_eq!(insights.days.len(), MAX_DAYS);
        assert_eq!(insights.days[0].day, 10); // days 0–9 dropped
        assert_eq!(insights.days[MAX_DAYS - 1].day, MAX_DAYS as u64 + 9);
    }

    #[test]
    fn backfill_replays_history_entries_per_day() {
        // A single entry keeps the assertion independent of the wall-clock
        // timestamp `History::add` stamps (no cross-midnight ambiguity).
        let mut history = History::default();
        history.add("hello little world", None);

        let mut insights = Insights::default();
        insights.backfill_from_history(&history);

        let r = insights.records();
        assert_eq!(r.best_day_words, 3);
        assert_eq!(r.tracked_days, 1);
    }

    #[test]
    fn load_recovers_a_corrupt_file_by_backing_it_up() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("insights.json");
        std::fs::write(&path, "{not json").expect("write");

        let insights = Insights::load(&path);
        assert!(insights.is_empty());
        assert!(path.with_extension("json.bak").exists());
        assert!(!path.exists());
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("insights.json");
        let mut insights = Insights::default();
        insights.record("hello world", 100 * MS_PER_DAY + 5_000);
        insights.save(&path).expect("save");

        let loaded = Insights::load(&path);
        assert_eq!(loaded.records(), insights.records());
    }
}
