//! Catalog entry and download for the local LLM model. Mirrors the STT model
//! manager: streaming download with progress, SHA256 verification while
//! streaming, and an atomic tempfile + rename finalize.

use anyhow::{Context, Result};
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use std::path::PathBuf;

/// Qwen3-1.7B Instruct, Q4_K_M GGUF. Apache-2.0 (base model `Qwen/Qwen3-1.7B`),
/// quantized by Unsloth.
///
/// Re-pinning: read `lfs.oid` (the git-LFS pointer, which is the file's own
/// SHA256) and `lfs.size` from
/// `https://huggingface.co/api/models/unsloth/Qwen3-1.7B-GGUF/tree/main`,
/// so no full download is needed to pin. Re-verify the repo LICENSE on every
/// re-pin.
pub const QWEN3_1_7B_URL: &str =
    "https://huggingface.co/unsloth/Qwen3-1.7B-GGUF/resolve/main/Qwen3-1.7B-Q4_K_M.gguf";

/// Local filename under [`llm_dir`].
pub const QWEN3_1_7B_FILENAME: &str = "Qwen3-1.7B-Q4_K_M.gguf";

/// Exact file size in bytes (from the LFS pointer); used for progress totals.
pub const QWEN3_1_7B_SIZE_BYTES: u64 = 1_107_409_472;

/// Pinned SHA256 (git-LFS oid) of the GGUF file.
pub const QWEN3_1_7B_SHA256: &str =
    "b139949c5bd74937ad8ed8c8cf3d9ffb1e99c866c823204dc42c0d91fa181897";

/// Directory where LLM models are stored (`data_dir()/murmur/llm/`).
pub fn llm_dir() -> Result<PathBuf> {
    let dir = dirs::data_dir()
        .ok_or_else(|| anyhow::anyhow!("Could not determine data directory"))?
        .join("murmur")
        .join("llm");
    Ok(dir)
}

/// Full path of the Qwen3 GGUF file.
pub fn model_path() -> Result<PathBuf> {
    Ok(llm_dir()?.join(QWEN3_1_7B_FILENAME))
}

/// Whether the model is present and non-empty. The tempfile + rename finalize
/// means a partial download never lands at this path, but stay defensive.
pub fn is_downloaded() -> bool {
    model_path()
        .ok()
        .and_then(|p| p.metadata().ok())
        .map(|m| m.len() > 0)
        .unwrap_or(false)
}

/// Download the model with a terminal progress bar.
pub async fn download() -> Result<PathBuf> {
    let pb = indicatif::ProgressBar::new(QWEN3_1_7B_SIZE_BYTES);
    pb.set_style(
        indicatif::ProgressStyle::default_bar()
            .template("{msg} [{bar:40}] {bytes}/{total_bytes} ({eta})")
            .unwrap_or_else(|_| indicatif::ProgressStyle::default_bar())
            .progress_chars("=> "),
    );
    pb.set_message(QWEN3_1_7B_FILENAME);

    let path = download_with_progress(|downloaded, total| {
        if let Some(total) = total {
            pb.set_length(total);
        }
        pb.set_position(downloaded);
    })
    .await?;

    pb.finish_with_message(format!("{QWEN3_1_7B_FILENAME} downloaded"));
    Ok(path)
}

/// Download the model with a progress callback `(bytes_downloaded, total)`,
/// verifying the pinned SHA256 while streaming. A corrupt or tampered
/// artifact is deleted, never finalized.
pub async fn download_with_progress<F>(on_progress: F) -> Result<PathBuf>
where
    F: Fn(u64, Option<u64>),
{
    let dir = llm_dir()?;
    std::fs::create_dir_all(&dir).context("Failed to create LLM model directory")?;
    let dest = dir.join(QWEN3_1_7B_FILENAME);
    let temp_path = dest.with_extension("partial");

    tracing::info!(url = QWEN3_1_7B_URL, "downloading LLM model");

    let client = reqwest::Client::new();
    let response = client
        .get(QWEN3_1_7B_URL)
        .send()
        .await
        .context("Failed to start LLM model download")?
        .error_for_status()
        .context("LLM model download request failed")?;

    let total = response.content_length().or(Some(QWEN3_1_7B_SIZE_BYTES));
    on_progress(0, total);

    let mut out = tokio::fs::File::create(&temp_path)
        .await
        .context("Failed to create temp file")?;
    let mut hasher = Sha256::new();
    let mut downloaded: u64 = 0;
    let mut stream = response.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("Error reading download stream")?;
        hasher.update(&chunk);
        tokio::io::AsyncWriteExt::write_all(&mut out, &chunk)
            .await
            .context("Failed to write chunk")?;
        downloaded += chunk.len() as u64;
        on_progress(downloaded, total);
    }

    tokio::io::AsyncWriteExt::flush(&mut out)
        .await
        .context("Failed to flush downloaded file")?;
    drop(out);

    if downloaded == 0 {
        let _ = tokio::fs::remove_file(&temp_path).await;
        anyhow::bail!("Downloaded LLM model is empty");
    }

    let actual = format!("{:x}", hasher.finalize());
    if let Err(e) = verify_sha256(&actual, QWEN3_1_7B_SHA256, QWEN3_1_7B_FILENAME) {
        let _ = tokio::fs::remove_file(&temp_path).await;
        return Err(e);
    }

    tokio::fs::rename(&temp_path, &dest)
        .await
        .context("Failed to finalize downloaded model")?;

    tracing::info!(path = %dest.display(), "LLM model ready");
    Ok(dest)
}

/// Compare a computed SHA256 hex digest against the pinned value.
fn verify_sha256(actual: &str, expected: &str, label: &str) -> Result<()> {
    if actual != expected {
        anyhow::bail!("SHA256 mismatch for {label}: expected {expected}, got {actual}");
    }
    tracing::info!("Checksum verified for {label}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::integrity::sha256_hex;

    #[test]
    fn verify_accepts_known_blob_and_rejects_tampered() {
        let blob = b"murmur llm test blob";
        let hash = sha256_hex(blob);
        assert!(verify_sha256(&hash, &hash, "blob").is_ok());

        let tampered = sha256_hex(b"murmur llm test blob, tampered");
        assert!(verify_sha256(&tampered, &hash, "blob").is_err());
    }

    #[test]
    fn catalog_metadata_is_well_formed() {
        assert!(QWEN3_1_7B_URL.starts_with("https://huggingface.co/"));
        assert!(QWEN3_1_7B_URL.ends_with(QWEN3_1_7B_FILENAME));
        assert_eq!(QWEN3_1_7B_SHA256.len(), 64);
        assert!(
            QWEN3_1_7B_SHA256
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
        // A Q4_K_M quant of a 1.7B model is roughly 1 GB; catch a stale or
        // truncated pin at compile time.
        const { assert!(QWEN3_1_7B_SIZE_BYTES > 500 * 1024 * 1024) };
        assert!(model_path().is_ok_and(|p| p.ends_with(QWEN3_1_7B_FILENAME)));
    }
}
