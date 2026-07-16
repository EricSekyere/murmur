//! Discovery + auth file for the local API: `<config dir>/murmur/local-api.json`
//! tells local plugins which loopback port to connect to and which token to
//! present. Written on successful bind, deleted when the API is disabled, and
//! the token is regenerated every app start. Same same-user trust boundary as
//! history.json.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// On-disk schema: `{"port": 12345, "token": "<32 hex chars>"}`.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Discovery {
    pub port: u16,
    pub token: String,
}

/// `<config dir>/murmur/local-api.json`, derived from the config path so the
/// two files can never land in different directories.
pub(crate) fn default_path() -> Result<PathBuf> {
    let config = murmur_core::config::Settings::default_path()
        .context("Could not determine local API discovery path")?;
    let dir = config
        .parent()
        .ok_or_else(|| anyhow::anyhow!("config path has no parent directory"))?;
    Ok(dir.join("local-api.json"))
}

/// Publish the endpoint atomically, so a plugin polling the file can never
/// read a torn port/token pair.
pub(crate) fn write(path: &Path, port: u16, token: &str) -> Result<()> {
    let content = serde_json::to_string(&Discovery {
        port,
        token: token.to_string(),
    })?;
    murmur_core::fsutil::atomic_write(path, content.as_bytes())
        .with_context(|| format!("write local API discovery file at {}", path.display()))
}

/// Best-effort delete so a stale file never advertises a dead or previous
/// endpoint. A missing file is the normal case, not an error.
pub(crate) fn clear(path: &Path) {
    if let Err(e) = std::fs::remove_file(path)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(
            "Failed to remove local API discovery file at {}: {}",
            path.display(),
            e
        );
    }
}

/// Fresh auth token: 16 OS-random bytes as 32 lowercase hex chars.
pub(crate) fn generate_token() -> Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| anyhow::anyhow!("OS RNG unavailable: {e}"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_file_round_trips_port_and_token() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("local-api.json");
        write(&path, 49_152, "00112233445566778899aabbccddeeff").expect("write");

        let content = std::fs::read_to_string(&path).expect("read");
        let parsed: Discovery = serde_json::from_str(&content).expect("parse");
        assert_eq!(parsed.port, 49_152);
        assert_eq!(parsed.token, "00112233445566778899aabbccddeeff");
    }

    #[test]
    fn write_replaces_a_previous_endpoint() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("local-api.json");
        write(&path, 1_000, "aa").expect("first write");
        write(&path, 2_000, "bb").expect("second write");

        let parsed: Discovery =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("parse");
        assert_eq!(parsed.port, 2_000);
        assert_eq!(parsed.token, "bb");
    }

    #[test]
    fn tokens_are_32_hex_chars_and_unique_per_call() {
        let a = generate_token().expect("token");
        let b = generate_token().expect("token");
        for token in [&a, &b] {
            assert_eq!(token.len(), 32);
            assert!(token.chars().all(|c| c.is_ascii_hexdigit()));
        }
        assert_ne!(a, b, "tokens must never repeat across generations");
    }

    #[test]
    fn clear_ignores_a_missing_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        clear(&dir.path().join("absent.json"));
    }
}
