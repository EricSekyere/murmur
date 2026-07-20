//! Raw-audio spool for whole-meeting diarization.
//!
//! Sortformer needs the FULL meeting in one buffer for consistent speaker
//! indices, but the meeting worker must never hold an unbounded recording in
//! RAM. So while a meeting records (and only when diarization is actually
//! possible), each mixed 16 kHz mono chunk is appended to
//! `<meetings dir>/<started_ms>.audio.tmp` as raw little-endian `f32` — no
//! container, no new dependency — and read back exactly once at stop.
//!
//! PRIVACY-CRITICAL: a spool file is raw meeting audio on disk, a deliberate,
//! bounded exception to "never store audio". Every exit path must delete it
//! (the app layer owns those paths), and [`sweep`] removes crash leftovers at
//! startup. Nothing here may ever log audio content.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Suffix of every spool file; [`sweep`] matches on it exactly so record
/// (`.json`) and export (`.md`) files can never be swept.
const SPOOL_SUFFIX: &str = ".audio.tmp";

/// The spool file path for meeting `id` inside `dir`.
pub fn spool_path(dir: &Path, id: u64) -> PathBuf {
    dir.join(format!("{id}{SPOOL_SUFFIX}"))
}

/// Incremental spool writer: buffered appends, flushed per chunk so a crash
/// loses at most the chunk in flight (the record file gets the same care).
pub struct SpoolWriter {
    path: PathBuf,
    writer: std::io::BufWriter<std::fs::File>,
}

impl SpoolWriter {
    /// Create (truncating) the spool for meeting `id` in `dir`.
    pub fn create(dir: &Path, id: u64) -> Result<Self> {
        std::fs::create_dir_all(dir).context("create meetings dir for audio spool")?;
        let path = spool_path(dir, id);
        let file = std::fs::File::create(&path)
            .with_context(|| format!("create meeting audio spool {}", path.display()))?;
        Ok(Self {
            path,
            writer: std::io::BufWriter::new(file),
        })
    }

    /// Append one mixed chunk as little-endian `f32` and flush it to disk.
    pub fn append(&mut self, samples: &[f32]) -> Result<()> {
        for sample in samples {
            self.writer
                .write_all(&sample.to_le_bytes())
                .context("write meeting audio spool")?;
        }
        self.writer.flush().context("flush meeting audio spool")
    }

    /// The spool's file path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Close the writer (best-effort final flush) and hand back the path for
    /// the read-back step.
    pub fn finish(mut self) -> PathBuf {
        if let Err(e) = self.writer.flush() {
            tracing::warn!("Final meeting audio spool flush failed: {e}");
        }
        self.path
    }

    /// Consume the writer and delete its file.
    pub fn delete(self) {
        let path = self.finish();
        remove(&path);
    }
}

/// Best-effort spool deletion; a failure on a still-existing file is logged
/// (the startup [`sweep`] is the backstop).
pub fn remove(path: &Path) {
    if let Err(e) = std::fs::remove_file(path)
        && path.exists()
    {
        tracing::warn!(?path, "Failed to delete meeting audio spool: {e}");
    }
}

/// Read a whole spool back as samples. Deliberately one `Vec`: diarization
/// needs the full meeting in a single buffer, and the transient peak
/// (~230 MB for a one-hour meeting at 16 kHz f32) is accepted; the streamed
/// decode below avoids doubling it with a raw byte copy. A trailing partial
/// sample (crash mid-write) is dropped.
pub fn read(path: &Path) -> Result<Vec<f32>> {
    let file = std::fs::File::open(path)
        .with_context(|| format!("open meeting audio spool {}", path.display()))?;
    let len = file.metadata().context("stat meeting audio spool")?.len() as usize;
    let mut samples = Vec::with_capacity(len / 4);

    let mut reader = std::io::BufReader::new(file);
    let mut buf = [0u8; 64 * 1024];
    // Bytes of a sample split across two reads: `carry_len` already arrived.
    let mut carry = [0u8; 4];
    let mut carry_len = 0usize;
    loop {
        let n = reader.read(&mut buf).context("read meeting audio spool")?;
        if n == 0 {
            break;
        }
        let mut i = 0;
        while carry_len > 0 && i < n {
            carry[carry_len] = buf[i];
            carry_len += 1;
            i += 1;
            if carry_len == 4 {
                samples.push(f32::from_le_bytes(carry));
                carry_len = 0;
            }
        }
        let chunks = buf[i..n].chunks_exact(4);
        let rem = chunks.remainder();
        for chunk in chunks {
            samples.push(f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
        }
        carry[..rem.len()].copy_from_slice(rem);
        carry_len = rem.len();
    }
    Ok(samples)
}

/// Delete every `*.audio.tmp` leftover in `dir`, returning how many were
/// removed. Crash recovery: a meeting that never reached its stop path must
/// not leave raw audio behind. Only spool files are touched — meeting records
/// and exports are user data and never matched.
pub fn sweep(dir: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut removed = 0;
    for path in entries.filter_map(|e| e.ok()).map(|e| e.path()) {
        let is_spool = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(SPOOL_SUFFIX));
        if !is_spool {
            continue;
        }
        match std::fs::remove_file(&path) {
            Ok(()) => removed += 1,
            Err(e) => tracing::warn!(?path, "Failed to sweep leftover meeting audio spool: {e}"),
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_read_round_trip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let samples: Vec<f32> = (0..10_000).map(|i| (i as f32 * 0.001).sin()).collect();

        let mut writer = SpoolWriter::create(dir.path(), 42).expect("create");
        writer.append(&samples).expect("append");
        let path = writer.finish();
        assert_eq!(path, spool_path(dir.path(), 42));

        assert_eq!(read(&path).expect("read"), samples);
    }

    #[test]
    fn appends_across_chunks_equal_one_concatenated_buffer() {
        let dir = tempfile::tempdir().expect("tempdir");
        let chunks: Vec<Vec<f32>> = vec![
            vec![0.1, -0.2, 0.3],
            vec![],
            (0..5_000).map(|i| i as f32 / 5_000.0).collect(),
            vec![f32::MIN_POSITIVE, -1.0, 1.0],
        ];

        let mut writer = SpoolWriter::create(dir.path(), 7).expect("create");
        for chunk in &chunks {
            writer.append(chunk).expect("append");
        }
        let path = writer.finish();

        let expected: Vec<f32> = chunks.concat();
        assert_eq!(read(&path).expect("read"), expected);
    }

    #[test]
    fn read_drops_a_trailing_partial_sample() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = spool_path(dir.path(), 1);
        let mut bytes = Vec::new();
        for s in [1.0f32, 2.0, 3.0] {
            bytes.extend_from_slice(&s.to_le_bytes());
        }
        bytes.extend_from_slice(&[0xAA, 0xBB]); // crash mid-write
        std::fs::write(&path, bytes).expect("write");

        assert_eq!(read(&path).expect("read"), vec![1.0, 2.0, 3.0]);
    }

    #[test]
    fn delete_removes_the_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut writer = SpoolWriter::create(dir.path(), 9).expect("create");
        writer.append(&[0.5; 100]).expect("append");
        let path = writer.path().to_path_buf();
        assert!(path.exists());

        writer.delete();
        assert!(!path.exists());
        // Removing an already-gone spool is silent (idempotent cleanup paths).
        remove(&path);
    }

    #[test]
    fn sweep_removes_only_spool_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(spool_path(dir.path(), 100), [0u8; 8]).expect("spool");
        std::fs::write(spool_path(dir.path(), 200), [0u8; 4]).expect("spool");
        std::fs::write(dir.path().join("100.json"), "{}").expect("record");
        std::fs::write(dir.path().join("100.md"), "# export").expect("export");
        std::fs::write(dir.path().join("stray.tmp"), "x").expect("stray");

        assert_eq!(sweep(dir.path()), 2);
        assert!(!spool_path(dir.path(), 100).exists());
        assert!(!spool_path(dir.path(), 200).exists());
        assert!(dir.path().join("100.json").exists());
        assert!(dir.path().join("100.md").exists());
        assert!(dir.path().join("stray.tmp").exists());
        // Idempotent, and a missing dir sweeps nothing.
        assert_eq!(sweep(dir.path()), 0);
        assert_eq!(sweep(&dir.path().join("nope")), 0);
    }
}
