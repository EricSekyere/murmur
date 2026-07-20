//! Persistent meeting transcripts: one JSON file per meeting under
//! `<config dir>/murmur/meetings/<started_ms>.json`, plus a pure Markdown
//! export.
//!
//! Meetings are user data, not a rolling convenience log: the dictation
//! history's clear/opt-out flows must NEVER touch this directory. The only
//! deletion path is the explicit per-meeting [`delete_in`] the UI exposes.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use super::SpeakerSegment;
use super::assembly::{self, TranscriptSegment};

/// Bumped when the record layout changes incompatibly; readers can branch on
/// it. `#[serde(default)]` everywhere keeps additive changes version-free:
/// v2 added `speakers` + `summary`, and v1 files still load with both empty.
pub const SCHEMA_VERSION: u32 = 2;

/// One recorded meeting: when it started, how long it ran, its timestamped
/// transcript, and (v2) optional speaker labels and an on-demand summary.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MeetingRecord {
    #[serde(default)]
    pub schema_version: u32,
    /// Unix epoch milliseconds when recording started. Doubles as the
    /// meeting's id and its filename stem.
    #[serde(default)]
    pub started_ms: u64,
    /// Total recorded duration in seconds.
    #[serde(default)]
    pub duration_secs: f32,
    /// Transcript segments with recording-relative timestamps.
    #[serde(default)]
    pub segments: Vec<TranscriptSegment>,
    /// Whole-meeting diarization spans. Empty means no speaker labels (v1
    /// record, model absent at recording time, or diarization failed).
    #[serde(default)]
    pub speakers: Vec<SpeakerSegment>,
    /// On-demand local-LLM summary; `None` until the user requests one.
    #[serde(default)]
    pub summary: Option<String>,
}

impl MeetingRecord {
    /// A fresh, empty record for a meeting that just started.
    pub fn new(started_ms: u64) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            started_ms,
            duration_secs: 0.0,
            segments: Vec::new(),
            speakers: Vec::new(),
            summary: None,
        }
    }

    /// Default meetings directory (`<config dir>/murmur/meetings`).
    pub fn default_dir() -> Result<PathBuf> {
        let dir = dirs::config_dir()
            .ok_or_else(|| anyhow::anyhow!("Could not determine config directory"))?
            .join("murmur")
            .join("meetings");
        Ok(dir)
    }

    /// The record file path for meeting `id` inside `dir`.
    pub fn path_in(dir: &Path, id: u64) -> PathBuf {
        dir.join(format!("{id}.json"))
    }

    /// Atomically save this record into `dir`, returning the file path.
    /// Called after every transcribed chunk, so a crash mid-meeting loses at
    /// most the chunk in flight.
    pub fn save_in(&self, dir: &Path) -> Result<PathBuf> {
        let path = Self::path_in(dir, self.started_ms);
        let content = serde_json::to_string(self).context("serializing meeting record")?;
        crate::fsutil::atomic_write(&path, content.as_bytes())
            .with_context(|| format!("writing meeting record {}", path.display()))?;
        Ok(path)
    }

    /// Load one record from `path`.
    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("reading meeting record {}", path.display()))?;
        serde_json::from_str(&content)
            .with_context(|| format!("parsing meeting record {}", path.display()))
    }

    /// All records in `dir`, newest first. A missing directory yields an
    /// empty list; an unreadable or corrupt entry is skipped with a warning
    /// (never deleted — it is user data) so one bad file can't hide the rest.
    pub fn list(dir: &Path) -> Vec<Self> {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut records: Vec<Self> = entries
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
            .filter_map(|path| match Self::load(&path) {
                Ok(record) => Some(record),
                Err(e) => {
                    tracing::warn!(?path, "Skipping unreadable meeting record: {e:#}");
                    None
                }
            })
            .collect();
        records.sort_by(|a, b| b.started_ms.cmp(&a.started_ms));
        records
    }

    /// Delete meeting `id`'s record file (and any exported Markdown sibling)
    /// from `dir`. The only deletion path — see the module docs.
    pub fn delete_in(dir: &Path, id: u64) -> Result<()> {
        std::fs::remove_file(Self::path_in(dir, id))
            .with_context(|| format!("deleting meeting record {id}"))?;
        // The export is derived data; best-effort removal alongside.
        let _ = std::fs::remove_file(dir.join(format!("{id}.md")));
        Ok(())
    }
}

/// Render a record as Markdown: a date-stamped title, an optional
/// `## Summary` section, then the transcript — speaker-labeled
/// `Speaker N: text` blocks when diarization ran, else the v1
/// `[mm:ss - mm:ss] text` lines. Pure and deterministic (the date is UTC —
/// std exposes no timezone) so the format is exact-tested.
pub fn export_markdown(record: &MeetingRecord) -> String {
    let (y, mo, d, h, mi) = utc_datetime(record.started_ms);
    let mut out = format!("# Meeting — {y:04}-{mo:02}-{d:02} {h:02}:{mi:02} UTC\n\n");
    if let Some(summary) = record.summary.as_deref().filter(|s| !s.trim().is_empty()) {
        out.push_str("## Summary\n\n");
        out.push_str(summary.trim());
        out.push_str("\n\n");
    }
    if record.speakers.is_empty() {
        for seg in &record.segments {
            out.push_str(&format!(
                "[{} - {}] {}\n",
                format_mm_ss(seg.start_secs),
                format_mm_ss(seg.end_secs),
                seg.text.trim()
            ));
        }
    } else {
        let blocks = assembly::label_transcript(&record.segments, &record.speakers);
        let body = assembly::format_transcript(&blocks);
        if !body.is_empty() {
            out.push_str(&body);
            out.push('\n');
        }
    }
    out
}

/// Seconds → zero-padded `mm:ss` (minutes uncapped: a 90-minute meeting reads
/// `90:00`, which stays sortable and unambiguous).
fn format_mm_ss(secs: f32) -> String {
    let total = secs.max(0.0) as u64;
    format!("{:02}:{:02}", total / 60, total % 60)
}

/// Epoch milliseconds → (year, month, day, hour, minute) in UTC, via the
/// standard civil-from-days algorithm (no chrono dependency).
fn utc_datetime(epoch_ms: u64) -> (i64, u32, u32, u32, u32) {
    let secs = (epoch_ms / 1000) as i64;
    let days = secs.div_euclid(86_400);
    let secs_of_day = secs.rem_euclid(86_400);

    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);

    (
        year,
        month,
        day,
        (secs_of_day / 3600) as u32,
        (secs_of_day % 3600 / 60) as u32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_record() -> MeetingRecord {
        MeetingRecord {
            schema_version: SCHEMA_VERSION,
            started_ms: 1_768_924_800_000, // 2026-01-20 16:00:00 UTC
            duration_secs: 83.5,
            segments: vec![
                TranscriptSegment::new(0.0, 19.4, "Morning, shall we start?"),
                TranscriptSegment::new(19.4, 41.0, "Yes, the build is green."),
                TranscriptSegment::new(61.0, 83.5, "Wrapping up."),
            ],
            speakers: Vec::new(),
            summary: None,
        }
    }

    fn sample_speakers() -> Vec<SpeakerSegment> {
        vec![
            SpeakerSegment {
                start_secs: 0.0,
                end_secs: 45.0,
                speaker: 0,
            },
            SpeakerSegment {
                start_secs: 45.0,
                end_secs: 83.5,
                speaker: 1,
            },
        ]
    }

    #[test]
    fn save_load_round_trip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let record = sample_record();
        let path = record.save_in(dir.path()).expect("save");
        assert_eq!(path, MeetingRecord::path_in(dir.path(), record.started_ms));

        let loaded = MeetingRecord::load(&path).expect("load");
        assert_eq!(loaded.schema_version, SCHEMA_VERSION);
        assert_eq!(loaded.started_ms, record.started_ms);
        assert_eq!(loaded.duration_secs, record.duration_secs);
        assert_eq!(loaded.segments, record.segments);
        assert!(loaded.speakers.is_empty());
        assert_eq!(loaded.summary, None);
    }

    #[test]
    fn v2_round_trip_keeps_speakers_and_summary() {
        let dir = tempfile::tempdir().expect("tempdir");
        let record = MeetingRecord {
            speakers: sample_speakers(),
            summary: Some("Two people agreed the build is green.".to_string()),
            ..sample_record()
        };
        let path = record.save_in(dir.path()).expect("save");

        let loaded = MeetingRecord::load(&path).expect("load");
        assert_eq!(loaded.speakers, record.speakers);
        assert_eq!(loaded.summary, record.summary);
    }

    #[test]
    fn v1_record_json_loads_with_empty_speakers_and_no_summary() {
        // Exactly the shape Phase 1 wrote: schema_version 1, no speakers or
        // summary keys at all. Serde defaults must fill them in.
        let dir = tempfile::tempdir().expect("tempdir");
        let v1 = r#"{
            "schema_version": 1,
            "started_ms": 1768924800000,
            "duration_secs": 40.0,
            "segments": [
                {"start_secs": 0.0, "end_secs": 19.4, "text": "Morning."},
                {"start_secs": 19.4, "end_secs": 40.0, "text": "Build is green."}
            ]
        }"#;
        let path = dir.path().join("1768924800000.json");
        std::fs::write(&path, v1).expect("write v1 record");

        let loaded = MeetingRecord::load(&path).expect("load v1");
        assert_eq!(loaded.schema_version, 1);
        assert_eq!(loaded.segments.len(), 2);
        assert!(loaded.speakers.is_empty());
        assert_eq!(loaded.summary, None);
        // And the v1 record still exports exactly like Phase 1 did.
        assert!(export_markdown(&loaded).contains("[00:00 - 00:19] Morning.\n"));
    }

    #[test]
    fn schema_version_is_present_on_disk() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = sample_record().save_in(dir.path()).expect("save");
        let raw = std::fs::read_to_string(path).expect("read");
        assert!(raw.contains("\"schema_version\":2"), "{raw}");
    }

    #[test]
    fn list_is_newest_first_and_skips_corrupt_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        for started_ms in [1_000_u64, 3_000, 2_000] {
            MeetingRecord::new(started_ms)
                .save_in(dir.path())
                .expect("save");
        }
        std::fs::write(dir.path().join("999.json"), "{not json").expect("write corrupt");
        std::fs::write(dir.path().join("notes.txt"), "ignored").expect("write stray");

        let listed = MeetingRecord::list(dir.path());
        let ids: Vec<u64> = listed.iter().map(|r| r.started_ms).collect();
        assert_eq!(ids, vec![3_000, 2_000, 1_000]);
        // Skip means skip: the corrupt file must still be on disk untouched.
        assert!(dir.path().join("999.json").exists());
    }

    #[test]
    fn list_of_missing_dir_is_empty() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(MeetingRecord::list(&dir.path().join("nope")).is_empty());
    }

    #[test]
    fn delete_removes_record_and_export() {
        let dir = tempfile::tempdir().expect("tempdir");
        let record = MeetingRecord::new(42);
        record.save_in(dir.path()).expect("save");
        std::fs::write(dir.path().join("42.md"), "export").expect("write export");

        MeetingRecord::delete_in(dir.path(), 42).expect("delete");
        assert!(!MeetingRecord::path_in(dir.path(), 42).exists());
        assert!(!dir.path().join("42.md").exists());
        // Deleting a missing meeting errors rather than silently succeeding.
        assert!(MeetingRecord::delete_in(dir.path(), 42).is_err());
    }

    #[test]
    fn export_markdown_exact_format() {
        let markdown = export_markdown(&sample_record());
        assert_eq!(
            markdown,
            "# Meeting — 2026-01-20 16:00 UTC\n\n\
             [00:00 - 00:19] Morning, shall we start?\n\
             [00:19 - 00:41] Yes, the build is green.\n\
             [01:01 - 01:23] Wrapping up.\n"
        );
    }

    #[test]
    fn export_markdown_with_speakers_renders_labeled_blocks() {
        let record = MeetingRecord {
            speakers: sample_speakers(),
            ..sample_record()
        };
        assert_eq!(
            export_markdown(&record),
            "# Meeting — 2026-01-20 16:00 UTC\n\n\
             Speaker 1: Morning, shall we start? Yes, the build is green.\n\
             Speaker 2: Wrapping up.\n"
        );
    }

    #[test]
    fn export_markdown_with_summary_renders_a_summary_section_on_top() {
        let record = MeetingRecord {
            summary: Some("Build status reviewed; all green.".to_string()),
            ..sample_record()
        };
        let markdown = export_markdown(&record);
        assert_eq!(
            markdown,
            "# Meeting — 2026-01-20 16:00 UTC\n\n\
             ## Summary\n\n\
             Build status reviewed; all green.\n\n\
             [00:00 - 00:19] Morning, shall we start?\n\
             [00:19 - 00:41] Yes, the build is green.\n\
             [01:01 - 01:23] Wrapping up.\n"
        );
        // A whitespace-only summary is treated as absent.
        let blank = MeetingRecord {
            summary: Some("   ".to_string()),
            ..sample_record()
        };
        assert_eq!(export_markdown(&blank), export_markdown(&sample_record()));
    }

    #[test]
    fn export_markdown_with_summary_and_speakers_combines_both() {
        let record = MeetingRecord {
            speakers: sample_speakers(),
            summary: Some("All green.".to_string()),
            ..sample_record()
        };
        assert_eq!(
            export_markdown(&record),
            "# Meeting — 2026-01-20 16:00 UTC\n\n\
             ## Summary\n\n\
             All green.\n\n\
             Speaker 1: Morning, shall we start? Yes, the build is green.\n\
             Speaker 2: Wrapping up.\n"
        );
    }

    #[test]
    fn export_markdown_of_empty_record_is_title_only() {
        let record = MeetingRecord {
            started_ms: 0,
            ..MeetingRecord::new(0)
        };
        assert_eq!(
            export_markdown(&record),
            "# Meeting — 1970-01-01 00:00 UTC\n\n"
        );
    }

    #[test]
    fn mm_ss_formats_and_never_caps_minutes() {
        assert_eq!(format_mm_ss(0.0), "00:00");
        assert_eq!(format_mm_ss(59.9), "00:59");
        assert_eq!(format_mm_ss(61.0), "01:01");
        assert_eq!(format_mm_ss(5_403.0), "90:03");
        assert_eq!(format_mm_ss(-3.0), "00:00");
    }
}
