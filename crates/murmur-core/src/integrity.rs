//! Download integrity helpers: every downloaded artifact must match a pinned
//! SHA256. There is no "unpinned" mode. A missing or malformed pin is a
//! programming error and fails the download rather than degrading to a warning,
//! so a new caller cannot silently opt out of verification by passing "".

use anyhow::Result;
use sha2::{Digest, Sha256};

/// Number of hex characters in a SHA256 digest.
const SHA256_HEX_LEN: usize = 64;

/// Compute the SHA256 of `bytes` as a lowercase hex string.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

/// Reject a pin that is missing or not a lowercase 64-char hex digest.
///
/// Callers that download should run this before any network work so an
/// unpinned artifact never reaches the disk in the first place.
pub fn ensure_pinned(expected: &str, label: &str) -> Result<()> {
    if expected.is_empty() {
        anyhow::bail!(
            "No pinned SHA256 for {label}; refusing to download an unverifiable artifact"
        );
    }
    if expected.len() != SHA256_HEX_LEN
        || !expected
            .chars()
            .all(|c| c.is_ascii_digit() || c.is_ascii_lowercase() && c.is_ascii_hexdigit())
    {
        anyhow::bail!(
            "Pinned checksum for {label} is not a lowercase 64-character hex SHA256: {expected}"
        );
    }
    Ok(())
}

/// Verify `bytes` against `expected` (a lowercase hex SHA256).
///
/// Errors when the pin is absent or malformed, and when the digest does not
/// match, so a tampered, corrupt, or unverifiable artifact is never loaded.
pub fn verify_sha256(bytes: &[u8], expected: &str, label: &str) -> Result<()> {
    verify_sha256_hash(&sha256_hex(bytes), expected, label)
}

/// Same policy as [`verify_sha256`], for callers that hash while streaming and
/// hold only the computed lowercase-hex digest.
pub fn verify_sha256_hash(actual: &str, expected: &str, label: &str) -> Result<()> {
    ensure_pinned(expected, label)?;
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
    fn verify_rejects_an_empty_pin() {
        let err = verify_sha256(b"anything", "", "test").expect_err("empty pin must fail");
        assert!(err.to_string().contains("No pinned SHA256"), "got: {err}");
    }

    #[test]
    fn verify_rejects_a_malformed_pin() {
        assert!(verify_sha256(b"payload", "abc123", "test").is_err());
        let upper = sha256_hex(b"payload").to_uppercase();
        assert!(verify_sha256(b"payload", &upper, "test").is_err());
    }

    #[test]
    fn verify_accepts_matching_and_rejects_mismatch() {
        let hash = sha256_hex(b"payload");
        assert!(verify_sha256(b"payload", &hash, "test").is_ok());
        assert!(verify_sha256(b"tampered", &hash, "test").is_err());
    }

    #[test]
    fn verify_hash_variant_matches_byte_variant_policy() {
        let hash = sha256_hex(b"payload");
        assert!(verify_sha256_hash(&hash, &hash, "test").is_ok());
        assert!(verify_sha256_hash(&sha256_hex(b"tampered"), &hash, "test").is_err());
        assert!(verify_sha256_hash(&hash, "", "test").is_err());
    }

    #[test]
    fn ensure_pinned_accepts_a_real_digest() {
        assert!(ensure_pinned(&sha256_hex(b"x"), "test").is_ok());
    }
}
