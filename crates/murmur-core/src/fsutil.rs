//! Filesystem helpers shared by config, history, and permission persistence,
//! plus the single place that resolves the config and home base directories.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Environment variable overriding the config base directory (the parent of
/// the `murmur` config folder).
pub const CONFIG_DIR_ENV: &str = "MURMUR_CONFIG_DIR";

/// Environment variable overriding the home directory used for client
/// configs (Cursor's `~/.cursor/mcp.json`).
pub const HOME_DIR_ENV: &str = "MURMUR_HOME_DIR";

/// Environment variable overriding the data base directory (the parent of
/// the `murmur` data folder holding models, ONNX Runtime, and caches).
pub const DATA_DIR_ENV: &str = "MURMUR_DATA_DIR";

/// The base directory all Murmur config paths hang off: `MURMUR_CONFIG_DIR`
/// when set to an absolute path, otherwise the platform default
/// (`dirs::config_dir()`: `%APPDATA%`, `~/Library/Application Support`, or
/// `$XDG_CONFIG_HOME`/`~/.config`).
///
/// The variable is read on every call, not cached: a spawned test process
/// sets it before launch anyway, and re-reading keeps in-process callers
/// consistent with the environment they were given rather than with whichever
/// call happened first. These are cold startup/save paths, so the lookup cost
/// is irrelevant. Empty and relative values are ignored (matching how the XDG
/// spec treats `XDG_CONFIG_HOME`), so a typo degrades to the platform default
/// instead of scattering files into a working-directory-relative location.
pub fn config_base_dir() -> Option<PathBuf> {
    resolve_base(
        std::env::var_os(CONFIG_DIR_ENV),
        CONFIG_DIR_ENV,
        dirs::config_dir,
    )
}

/// The home directory: `MURMUR_HOME_DIR` when set to an absolute path,
/// otherwise `dirs::home_dir()`. Same override semantics as
/// [`config_base_dir`].
pub fn home_dir() -> Option<PathBuf> {
    resolve_base(std::env::var_os(HOME_DIR_ENV), HOME_DIR_ENV, dirs::home_dir)
}

/// The base directory model and cache paths hang off: `MURMUR_DATA_DIR` when
/// set to an absolute path, otherwise `dirs::data_dir()`. Same override
/// semantics as [`config_base_dir`].
pub fn data_base_dir() -> Option<PathBuf> {
    resolve_base(std::env::var_os(DATA_DIR_ENV), DATA_DIR_ENV, dirs::data_dir)
}

fn resolve_base(
    override_value: Option<OsString>,
    var_name: &str,
    platform_default: impl FnOnce() -> Option<PathBuf>,
) -> Option<PathBuf> {
    sanitize_override(override_value, var_name).or_else(platform_default)
}

/// Accept only non-empty absolute override values; anything else falls back
/// to the platform default so a bad override can't misplace user data.
fn sanitize_override(value: Option<OsString>, var_name: &str) -> Option<PathBuf> {
    let value = value?;
    if value.is_empty() {
        return None;
    }
    let path = PathBuf::from(value);
    if !path.is_absolute() {
        tracing::warn!(?path, var_name, "ignoring relative directory override");
        return None;
    }
    Some(path)
}

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
    fn override_accepts_only_absolute_non_empty_values() {
        // Pure-value tests: env vars are process-global and would race
        // concurrently running tests, so the sanitizer takes the value.
        assert_eq!(sanitize_override(None, "X"), None);
        assert_eq!(sanitize_override(Some(OsString::new()), "X"), None);
        assert_eq!(
            sanitize_override(Some(OsString::from("relative/dir")), "X"),
            None
        );
        let abs = std::env::temp_dir();
        assert_eq!(
            sanitize_override(Some(abs.clone().into_os_string()), "X"),
            Some(abs)
        );
    }

    #[test]
    fn resolve_base_prefers_override_and_falls_back() {
        let abs = std::env::temp_dir();
        let fallback = || Some(PathBuf::from("/fallback"));
        assert_eq!(
            resolve_base(Some(abs.clone().into_os_string()), "X", fallback),
            Some(abs)
        );
        assert_eq!(
            resolve_base(Some(OsString::from("not-absolute")), "X", fallback),
            Some(PathBuf::from("/fallback"))
        );
        assert_eq!(
            resolve_base(None, "X", fallback),
            Some(PathBuf::from("/fallback"))
        );
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
