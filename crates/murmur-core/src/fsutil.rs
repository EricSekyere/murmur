//! Filesystem helpers shared by config, history, and permission persistence.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Write `bytes` to `path` atomically: write a uniquely named sibling
/// tempfile, then rename it over the target.
///
/// The tempfile name embeds the pid and a process-wide counter. A fixed
/// sibling name (the old `config.toml.tmp` pattern) let two processes
/// recovering the same file interleave writes into one tempfile and rename a
/// torn file into place. On rename failure the tempfile is removed
/// best-effort; a crash between write and rename can leak a single `.tmp`
/// sibling, which later successful saves never reuse.
///
/// # Errors
/// Propagates directory creation, write, and rename failures.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = unique_tmp_path(path);
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

fn unique_tmp_path(path: &Path) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(format!(".{}.{seq}.tmp", std::process::id()));
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_creates_parents_and_replaces_content() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nested").join("file.toml");
        atomic_write(&path, b"first").expect("write");
        atomic_write(&path, b"second").expect("overwrite");
        assert_eq!(std::fs::read(&path).expect("read"), b"second");
    }

    #[test]
    fn atomic_write_leaves_no_tempfile_behind() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("file.toml");
        atomic_write(&path, b"data").expect("write");
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .expect("read_dir")
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "tempfiles must not linger");
    }

    #[test]
    fn tmp_paths_are_unique_per_call() {
        let path = Path::new("some/config.toml");
        let a = unique_tmp_path(path);
        let b = unique_tmp_path(path);
        assert_ne!(a, b, "concurrent writers must never share a tempfile");
        assert!(a.to_string_lossy().ends_with(".tmp"));
    }
}
