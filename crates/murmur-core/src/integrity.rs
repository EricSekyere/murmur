//! Download integrity helpers: verify each downloaded artifact against a pinned
//! SHA256, or log the computed hash to pin (trust-on-first-use) when none is set.

use anyhow::Result;
use sha2::{Digest, Sha256};

/// Compute the SHA256 of `bytes` as a lowercase hex string.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

/// Verify `bytes` against `expected` (a lowercase hex SHA256).
///
/// - `expected` empty: no hash pinned yet. Logs the computed hash so it can be
///   pinned, and accepts the download.
/// - `expected` set and matching: accepts.
/// - `expected` set and mismatched: returns an error so a tampered or corrupt
///   artifact is never loaded.
pub fn verify_or_log_sha256(bytes: &[u8], expected: &str, label: &str) -> Result<()> {
    let actual = sha256_hex(bytes);
    if expected.is_empty() {
        tracing::warn!(
            "No pinned checksum for {label}; integrity not verified (sha256={actual}). \
             Pin this value to enforce it on future downloads."
        );
        return Ok(());
    }
    if actual != expected {
        anyhow::bail!("SHA256 mismatch for {label}: expected {expected}, got {actual}");
    }
    tracing::info!("Checksum verified for {label}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_matches_known_vector() {
        // SHA256 of the empty string.
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn verify_accepts_when_unpinned() {
        assert!(verify_or_log_sha256(b"anything", "", "test").is_ok());
    }

    #[test]
    fn verify_accepts_matching_and_rejects_mismatch() {
        let hash = sha256_hex(b"payload");
        assert!(verify_or_log_sha256(b"payload", &hash, "test").is_ok());
        assert!(verify_or_log_sha256(b"tampered", &hash, "test").is_err());
    }
}
