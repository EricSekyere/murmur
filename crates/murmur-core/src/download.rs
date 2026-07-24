//! Shared resumable artifact downloader.
//!
//! Every large HTTP download in Murmur (STT models, ONNX Runtime archive,
//! Silero VAD, Sortformer diarization, LLM GGUF, Help embedder) streams
//! through here. Bytes land in a sibling `<name>.partial` file, never the
//! final path; an interrupted download leaves the partial behind and the next
//! attempt resumes it with an HTTP `Range` request instead of restarting. The
//! pinned SHA256 is verified before the partial is renamed into place, so a
//! corrupt or tampered artifact is never observable at the final path.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// How the server answered a (possibly ranged) fetch request.
enum FetchStart {
    /// Body starts at byte zero: a fresh download, or the server ignored the
    /// `Range` header (200 instead of 206).
    Full { total: Option<u64> },
    /// 206: the body resumes at the requested offset. `total` is the full
    /// artifact size (offset + remaining body length).
    Resumed { total: Option<u64> },
    /// 416: the requested offset is at or past the end of the artifact, so
    /// the partial is either already complete or longer than the artifact.
    RangeNotSatisfiable,
}

/// A source of artifact bytes, abstracted so the resume logic can be tested
/// against a scripted fake instead of the network.
trait FetchSource {
    type Body: FetchBody;
    /// Begin fetching `url` at `offset` (0 = plain full-body request).
    async fn begin(&self, url: &str, offset: u64) -> Result<(FetchStart, Self::Body)>;
}

/// A streaming fetch body.
trait FetchBody {
    /// The next chunk of the body, or `None` at its end.
    async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>>;
}

struct HttpSource {
    client: reqwest::Client,
}

struct HttpBody(reqwest::Response);

impl FetchBody for HttpBody {
    async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>> {
        let chunk = self
            .0
            .chunk()
            .await
            .context("Error reading download stream")?;
        Ok(chunk.map(|bytes| bytes.to_vec()))
    }
}

impl FetchSource for HttpSource {
    type Body = HttpBody;

    async fn begin(&self, url: &str, offset: u64) -> Result<(FetchStart, HttpBody)> {
        let mut request = self.client.get(url);
        if offset > 0 {
            request = request.header(reqwest::header::RANGE, format!("bytes={offset}-"));
        }
        let response = request.send().await.context("Failed to start download")?;
        match response.status() {
            reqwest::StatusCode::PARTIAL_CONTENT => {
                let total = response
                    .content_length()
                    .map(|remaining| offset + remaining);
                Ok((FetchStart::Resumed { total }, HttpBody(response)))
            }
            reqwest::StatusCode::RANGE_NOT_SATISFIABLE => {
                Ok((FetchStart::RangeNotSatisfiable, HttpBody(response)))
            }
            _ => {
                let response = response
                    .error_for_status()
                    .context("Download request failed")?;
                let total = response.content_length();
                Ok((FetchStart::Full { total }, HttpBody(response)))
            }
        }
    }
}

/// Sibling path where in-progress bytes accumulate (`<file name>.partial`).
/// Appended to the whole file name (not swapped for the extension) so two
/// artifacts differing only in extension can never share a partial.
pub fn partial_path(dest: &Path) -> PathBuf {
    let mut name = dest
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(".partial");
    dest.with_file_name(name)
}

/// Download `url` to `dest`: stream into a sibling `.partial` (resuming one
/// left by an earlier interrupted attempt), verify `expected_sha256` (empty =
/// warn-and-accept, matching [`crate::integrity::verify_or_log_sha256`]), and
/// rename into place. On a checksum mismatch the partial is deleted and the
/// mismatch error returned, so a retry refetches cleanly.
///
/// Returns the artifact length in bytes. `on_progress` receives
/// `(bytes_present, total_bytes)` where `bytes_present` includes the resumed
/// offset, so a resumed progress bar continues instead of restarting at zero.
pub async fn fetch_to_file<F>(
    url: &str,
    dest: &Path,
    expected_sha256: &str,
    label: &str,
    on_progress: F,
) -> Result<u64>
where
    F: FnMut(u64, Option<u64>),
{
    let source = HttpSource {
        client: reqwest::Client::new(),
    };
    fetch_with_source(&source, url, dest, expected_sha256, label, on_progress).await
}

async fn fetch_with_source<S, F>(
    source: &S,
    url: &str,
    dest: &Path,
    expected_sha256: &str,
    label: &str,
    mut on_progress: F,
) -> Result<u64>
where
    S: FetchSource,
    F: FnMut(u64, Option<u64>),
{
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .context("Failed to create download directory")?;
    }
    let partial = partial_path(dest);
    let offset = tokio::fs::metadata(&partial)
        .await
        .map(|m| m.len())
        .unwrap_or(0);

    let (mut start, mut body) = source.begin(url, offset).await?;

    if matches!(start, FetchStart::RangeNotSatisfiable) {
        // The partial already spans the whole artifact: finish it if its
        // checksum proves it complete, otherwise discard it and refetch.
        if let Some(len) = finalize_if_complete(&partial, dest, expected_sha256, label).await? {
            on_progress(len, Some(len));
            return Ok(len);
        }
        tokio::fs::remove_file(&partial)
            .await
            .context("Failed to discard stale partial file")?;
        (start, body) = source.begin(url, 0).await?;
    }

    let mut hasher = Sha256::new();
    let (mut out, mut done, total) = match start {
        FetchStart::Resumed { total } => {
            // Bytes appended below must hash together with what is on disk.
            let hashed = hash_file(&partial, &mut hasher).await?;
            anyhow::ensure!(
                hashed == offset,
                "partial file for {label} changed size during resume ({hashed} != {offset})"
            );
            let out = tokio::fs::OpenOptions::new()
                .append(true)
                .open(&partial)
                .await
                .context("Failed to open partial file for append")?;
            tracing::info!(label, resumed_bytes = offset, "resuming download");
            (out, offset, total)
        }
        FetchStart::Full { total } => {
            let out = tokio::fs::File::create(&partial)
                .await
                .context("Failed to create partial file")?;
            (out, 0, total)
        }
        // Only reachable if the server rejects a fresh zero-offset fetch.
        FetchStart::RangeNotSatisfiable => {
            anyhow::bail!("Server rejected download of {label} at offset 0")
        }
    };
    on_progress(done, total);

    while let Some(chunk) = body.next_chunk().await? {
        hasher.update(&chunk);
        out.write_all(&chunk)
            .await
            .context("Failed to write chunk")?;
        done += chunk.len() as u64;
        on_progress(done, total);
    }
    // Flush and close before verifying: async file writes complete in the
    // background, so a final write error (disk full) only surfaces here —
    // without this a truncated file could be renamed into place.
    out.flush()
        .await
        .context("Failed to flush downloaded file")?;
    drop(out);

    if done == 0 {
        let _ = tokio::fs::remove_file(&partial).await;
        anyhow::bail!("Downloaded file {label} is empty");
    }

    let actual = format!("{:x}", hasher.finalize());
    if let Err(e) = crate::integrity::verify_hash_or_log(&actual, expected_sha256, label) {
        // A corrupt partial must not survive to poison the next attempt.
        let _ = tokio::fs::remove_file(&partial).await;
        return Err(e);
    }
    tokio::fs::rename(&partial, dest)
        .await
        .context("Failed to finalize downloaded file")?;
    Ok(done)
}

/// If the partial's checksum matches the pin it is the complete artifact:
/// rename it into place and return its length. Unpinned artifacts cannot be
/// proven complete, so they are never finalized here.
async fn finalize_if_complete(
    partial: &Path,
    dest: &Path,
    expected_sha256: &str,
    label: &str,
) -> Result<Option<u64>> {
    if expected_sha256.is_empty() {
        return Ok(None);
    }
    let mut hasher = Sha256::new();
    let len = hash_file(partial, &mut hasher).await?;
    let actual = format!("{:x}", hasher.finalize());
    if actual != expected_sha256 {
        return Ok(None);
    }
    crate::integrity::verify_hash_or_log(&actual, expected_sha256, label)?;
    tokio::fs::rename(partial, dest)
        .await
        .context("Failed to finalize downloaded file")?;
    Ok(Some(len))
}

/// Feed an existing file through `hasher`, returning the byte count hashed.
async fn hash_file(path: &Path, hasher: &mut Sha256) -> Result<u64> {
    let mut file = tokio::fs::File::open(path)
        .await
        .context("Failed to open partial file")?;
    let mut buf = vec![0u8; 256 * 1024];
    let mut hashed = 0u64;
    loop {
        let read = file
            .read(&mut buf)
            .await
            .context("Failed to read partial file")?;
        if read == 0 {
            return Ok(hashed);
        }
        hasher.update(&buf[..read]);
        hashed += read as u64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::integrity::sha256_hex;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    const BODY: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";

    /// Scripted artifact server: serves `data`, optionally honoring Range
    /// requests, recording each requested offset.
    struct FakeSource {
        data: Vec<u8>,
        supports_range: bool,
        offsets: Mutex<Vec<u64>>,
    }

    struct FakeBody {
        chunks: VecDeque<Vec<u8>>,
    }

    impl FetchBody for FakeBody {
        async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>> {
            Ok(self.chunks.pop_front())
        }
    }

    impl FetchSource for FakeSource {
        type Body = FakeBody;

        async fn begin(&self, _url: &str, offset: u64) -> Result<(FetchStart, FakeBody)> {
            self.offsets
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(offset);
            let total = Some(self.data.len() as u64);
            if offset == 0 || !self.supports_range {
                return Ok((FetchStart::Full { total }, chunked(&self.data)));
            }
            if offset >= self.data.len() as u64 {
                return Ok((
                    FetchStart::RangeNotSatisfiable,
                    FakeBody {
                        chunks: VecDeque::new(),
                    },
                ));
            }
            Ok((
                FetchStart::Resumed { total },
                chunked(&self.data[offset as usize..]),
            ))
        }
    }

    fn chunked(data: &[u8]) -> FakeBody {
        FakeBody {
            chunks: data.chunks(7).map(<[u8]>::to_vec).collect(),
        }
    }

    fn source(data: &[u8], supports_range: bool) -> FakeSource {
        FakeSource {
            data: data.to_vec(),
            supports_range,
            offsets: Mutex::new(Vec::new()),
        }
    }

    fn requested_offsets(source: &FakeSource) -> Vec<u64> {
        source
            .offsets
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    #[test]
    fn partial_path_appends_to_the_full_file_name() {
        assert_eq!(
            partial_path(Path::new("models/encoder-model.onnx")),
            Path::new("models/encoder-model.onnx.partial")
        );
    }

    #[tokio::test]
    async fn fresh_download_verifies_and_finalizes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let dest = dir.path().join("artifact.bin");
        let src = source(BODY, true);
        let mut events: Vec<(u64, Option<u64>)> = Vec::new();

        let len = fetch_with_source(&src, "u", &dest, &sha256_hex(BODY), "artifact", |d, t| {
            events.push((d, t))
        })
        .await
        .expect("fresh download");

        assert_eq!(len, BODY.len() as u64);
        assert_eq!(std::fs::read(&dest).expect("read dest"), BODY);
        assert!(
            !partial_path(&dest).exists(),
            "partial must be renamed away"
        );
        assert_eq!(requested_offsets(&src), vec![0]);
        assert_eq!(events.first(), Some(&(0, Some(BODY.len() as u64))));
        assert_eq!(
            events.last(),
            Some(&(BODY.len() as u64, Some(BODY.len() as u64)))
        );
        assert!(
            events.windows(2).all(|w| w[0].0 <= w[1].0),
            "progress must be monotonic"
        );
    }

    #[tokio::test]
    async fn resume_appends_from_partial_offset() {
        let dir = tempfile::tempdir().expect("tempdir");
        let dest = dir.path().join("artifact.bin");
        std::fs::write(partial_path(&dest), &BODY[..10]).expect("seed partial");
        let src = source(BODY, true);
        let mut events: Vec<(u64, Option<u64>)> = Vec::new();

        let len = fetch_with_source(&src, "u", &dest, &sha256_hex(BODY), "artifact", |d, t| {
            events.push((d, t))
        })
        .await
        .expect("resumed download");

        assert_eq!(len, BODY.len() as u64);
        assert_eq!(std::fs::read(&dest).expect("read dest"), BODY);
        // Only the tail was requested, and the bar starts at the resumed
        // offset — never back at zero.
        assert_eq!(requested_offsets(&src), vec![10]);
        assert_eq!(events.first(), Some(&(10, Some(BODY.len() as u64))));
        assert!(events.iter().all(|(d, _)| *d >= 10));
    }

    #[tokio::test]
    async fn server_without_range_support_restarts_from_zero() {
        let dir = tempfile::tempdir().expect("tempdir");
        let dest = dir.path().join("artifact.bin");
        // Junk partial: the no-Range server's 200 must truncate it away.
        std::fs::write(partial_path(&dest), b"JUNK").expect("seed partial");
        let src = source(BODY, false);

        let len = fetch_with_source(&src, "u", &dest, &sha256_hex(BODY), "artifact", |_, _| {})
            .await
            .expect("restarted download");

        assert_eq!(len, BODY.len() as u64);
        assert_eq!(std::fs::read(&dest).expect("read dest"), BODY);
        // Resume was attempted (offset 4) but the 200 reply restarted cleanly.
        assert_eq!(requested_offsets(&src), vec![4]);
    }

    #[tokio::test]
    async fn corrupt_partial_fails_checksum_and_is_deleted() {
        let dir = tempfile::tempdir().expect("tempdir");
        let dest = dir.path().join("artifact.bin");
        std::fs::write(partial_path(&dest), b"corrupted!").expect("seed partial");
        let src = source(BODY, true);

        let err = fetch_with_source(&src, "u", &dest, &sha256_hex(BODY), "artifact", |_, _| {})
            .await
            .expect_err("corrupt resume must fail verification");

        assert!(
            err.to_string().contains("SHA256 mismatch"),
            "unexpected error: {err}"
        );
        assert!(
            !partial_path(&dest).exists(),
            "corrupt partial must be deleted"
        );
        assert!(
            !dest.exists(),
            "corrupt bytes must never reach the final path"
        );
    }

    #[tokio::test]
    async fn already_complete_partial_is_finalized_without_refetch() {
        let dir = tempfile::tempdir().expect("tempdir");
        let dest = dir.path().join("artifact.bin");
        std::fs::write(partial_path(&dest), BODY).expect("seed partial");
        let src = source(BODY, true);
        let mut events: Vec<(u64, Option<u64>)> = Vec::new();

        let len = fetch_with_source(&src, "u", &dest, &sha256_hex(BODY), "artifact", |d, t| {
            events.push((d, t))
        })
        .await
        .expect("complete partial finalizes");

        assert_eq!(len, BODY.len() as u64);
        assert_eq!(std::fs::read(&dest).expect("read dest"), BODY);
        // One probe at the end offset (answered 416), no body refetched.
        assert_eq!(requested_offsets(&src), vec![BODY.len() as u64]);
        assert_eq!(
            events.as_slice(),
            [(BODY.len() as u64, Some(BODY.len() as u64))]
        );
    }

    #[tokio::test]
    async fn overlong_corrupt_partial_is_discarded_and_refetched() {
        let dir = tempfile::tempdir().expect("tempdir");
        let dest = dir.path().join("artifact.bin");
        let mut overlong = BODY.to_vec();
        overlong.extend_from_slice(b"trailing garbage");
        std::fs::write(partial_path(&dest), &overlong).expect("seed partial");
        let src = source(BODY, true);

        let len = fetch_with_source(&src, "u", &dest, &sha256_hex(BODY), "artifact", |_, _| {})
            .await
            .expect("overlong partial refetches");

        assert_eq!(len, BODY.len() as u64);
        assert_eq!(std::fs::read(&dest).expect("read dest"), BODY);
        // 416 probe at the overlong offset, then a clean restart from zero.
        assert_eq!(requested_offsets(&src), vec![overlong.len() as u64, 0]);
    }
}
