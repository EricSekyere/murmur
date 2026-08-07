//! Shared resumable artifact downloader.
//!
//! Every large HTTP download in Murmur (STT models, ONNX Runtime archive,
//! Silero VAD, Sortformer diarization, LLM GGUF, Help embedder) streams
//! through here. Bytes land in a sibling `<name>.partial` file, never the
//! final path; an interrupted download leaves the partial behind and the next
//! attempt resumes it with an HTTP `Range` request instead of restarting. The
//! pinned SHA256 is verified before the partial is renamed into place, so a
//! corrupt or tampered artifact is never observable at the final path.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Give up if the server never answers the initial connect.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// Cap the gap between two body chunks rather than the transfer as a whole:
/// artifacts here run to several hundred megabytes, so any total timeout large
/// enough for a slow link would also be too large to catch a stalled one. A
/// per-read deadline fires on the actual failure mode (headers sent, then
/// silence) while a genuinely slow but progressing download runs as long as it
/// needs.
const READ_TIMEOUT: Duration = Duration::from_secs(60);

/// Shared client: connection pooling across the several artifacts a first run
/// fetches, and one place where the timeouts above are guaranteed to apply.
static CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(READ_TIMEOUT)
        .build()
        // Only fails if TLS setup fails, in which case no download could work
        // anyway; the plain client keeps that a per-request error, not a panic.
        .unwrap_or_else(|e| {
            tracing::error!("Failed to build download client, using defaults: {}", e);
            reqwest::Client::new()
        })
});

/// Destinations with a `fetch_to_file` running against them right now.
///
/// Two fetches sharing a `dest` share the same `.partial`, and both would open
/// it in append mode and write the full body into it. Serializing by waiting
/// would only make the loser re-verify bytes the winner already finalized, so
/// the second caller fails fast instead and the caller (a retry button, a
/// second window) can decide whether to retry.
static IN_FLIGHT: LazyLock<Mutex<HashSet<PathBuf>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

/// Claims `dest` for the duration of one fetch, releasing it on drop so an
/// error path or a cancelled task cannot leak the claim.
struct DestClaim(PathBuf);

impl DestClaim {
    fn acquire(dest: &Path, label: &str) -> Result<Self> {
        // Resolve through the parent so `./models/x` and an absolute path to
        // the same file collide; the file itself may not exist yet.
        let key = match dest.parent().map(std::fs::canonicalize) {
            Some(Ok(parent)) => match dest.file_name() {
                Some(name) => parent.join(name),
                None => dest.to_path_buf(),
            },
            _ => dest.to_path_buf(),
        };
        let mut in_flight = IN_FLIGHT.lock().unwrap_or_else(|e| e.into_inner());
        anyhow::ensure!(
            in_flight.insert(key.clone()),
            "A download of {label} into this location is already in progress"
        );
        Ok(Self(key))
    }
}

impl Drop for DestClaim {
    fn drop(&mut self) {
        IN_FLIGHT
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.0);
    }
}

/// How the server answered a (possibly ranged) fetch request.
enum FetchStart {
    /// Body starts at byte zero: a fresh download, or the server ignored the
    /// `Range` header (200 instead of 206).
    Full { total: Option<u64> },
    /// 206: the body resumes at `start`. `total` is the full artifact size
    /// (start + remaining body length).
    Resumed {
        start: Option<u64>,
        total: Option<u64>,
    },
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
                let start = response
                    .headers()
                    .get(reqwest::header::CONTENT_RANGE)
                    .and_then(|v| v.to_str().ok())
                    .and_then(content_range_start);
                Ok((FetchStart::Resumed { start, total }, HttpBody(response)))
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

/// First byte offset of a `Content-Range: bytes <start>-<end>/<total>` header.
fn content_range_start(value: &str) -> Option<u64> {
    value
        .trim()
        .strip_prefix("bytes ")?
        .split('-')
        .next()?
        .trim()
        .parse()
        .ok()
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
        client: CLIENT.clone(),
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
    let _claim = DestClaim::acquire(dest, label)?;
    let partial = partial_path(dest);
    let offset = tokio::fs::metadata(&partial)
        .await
        .map(|m| m.len())
        .unwrap_or(0);

    let (mut start, mut body) = source.begin(url, offset).await?;

    if matches!(start, FetchStart::RangeNotSatisfiable) && offset > 0 {
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

    let (mut out, mut done, total) = match start {
        FetchStart::Resumed { start, total } => {
            // A server that starts the range anywhere but the requested offset
            // would splice a gap or an overlap into the partial. The final
            // checksum catches that for pinned artifacts; this catches it for
            // unpinned ones too, and points at the real culprit.
            if let Some(start) = start {
                anyhow::ensure!(
                    start == offset,
                    "Server resumed {label} at byte {start}, expected {offset}"
                );
            }
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

    // Hash the finished partial off disk, not the bytes seen on the socket:
    // only the file is what gets renamed to `dest`, and a digest accumulated
    // from the stream would certify bytes that never had to match it.
    let mut hasher = Sha256::new();
    let on_disk = hash_file(&partial, &mut hasher).await?;
    let actual = format!("{:x}", hasher.finalize());
    let verified =
        crate::integrity::verify_hash_or_log(&actual, expected_sha256, label).and_then(|()| {
            anyhow::ensure!(
                on_disk == done,
                "Downloaded file {label} is {on_disk} bytes on disk, expected {done}"
            );
            Ok(())
        });
    if let Err(e) = verified {
        // A corrupt partial must not survive to poison the next attempt.
        let _ = tokio::fs::remove_file(&partial).await;
        return Err(e);
    }
    tokio::fs::rename(&partial, dest)
        .await
        .context("Failed to finalize downloaded file")?;
    Ok(on_disk)
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
            // Yield so concurrent fetches actually interleave in the
            // concurrency test instead of each running to completion.
            tokio::task::yield_now().await;
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
                FetchStart::Resumed {
                    start: Some(offset),
                    total,
                },
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

    #[tokio::test]
    async fn concurrent_fetches_to_the_same_dest_cannot_corrupt_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let dest = dir.path().join("artifact.bin");
        let src = source(BODY, true);
        let pin = sha256_hex(BODY);

        let (a, b) = tokio::join!(
            fetch_with_source(&src, "u", &dest, &pin, "artifact", |_, _| {}),
            fetch_with_source(&src, "u", &dest, &pin, "artifact", |_, _| {}),
        );

        // Exactly one may proceed; the loser must say why, not append a second
        // copy of the body into the shared partial.
        let losers: Vec<_> = [&a, &b].iter().filter_map(|r| r.as_ref().err()).collect();
        assert_eq!(losers.len(), 1, "expected one winner and one refusal");
        assert!(
            losers[0].to_string().contains("already in progress"),
            "unexpected error: {}",
            losers[0]
        );
        assert_eq!(std::fs::read(&dest).expect("read dest"), BODY);
        assert!(!partial_path(&dest).exists());
    }

    #[tokio::test]
    async fn a_claim_is_released_after_the_fetch_finishes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let dest = dir.path().join("artifact.bin");
        let src = source(BODY, true);
        let pin = sha256_hex(BODY);

        fetch_with_source(&src, "u", &dest, &pin, "artifact", |_, _| {})
            .await
            .expect("first fetch");
        std::fs::remove_file(&dest).expect("remove dest");
        fetch_with_source(&src, "u", &dest, &pin, "artifact", |_, _| {})
            .await
            .expect("second fetch must not be blocked by a stale claim");
    }

    /// The bytes on disk, not the bytes seen on the wire, are what the pin
    /// must certify: this body writes a tail it never reports as a chunk.
    struct LyingSource;

    struct LyingBody {
        path: PathBuf,
        chunks: VecDeque<Vec<u8>>,
    }

    impl FetchBody for LyingBody {
        async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>> {
            match self.chunks.pop_front() {
                Some(chunk) => Ok(Some(chunk)),
                None => {
                    use std::io::Write;
                    let mut f = std::fs::OpenOptions::new()
                        .append(true)
                        .open(&self.path)
                        .expect("append to partial");
                    f.write_all(b"tampered").expect("write tamper");
                    Ok(None)
                }
            }
        }
    }

    impl FetchSource for LyingSource {
        type Body = LyingBody;

        async fn begin(&self, url: &str, _offset: u64) -> Result<(FetchStart, LyingBody)> {
            Ok((
                FetchStart::Full {
                    total: Some(BODY.len() as u64),
                },
                LyingBody {
                    path: PathBuf::from(url),
                    chunks: BODY.chunks(7).map(<[u8]>::to_vec).collect(),
                },
            ))
        }
    }

    #[tokio::test]
    async fn bytes_added_behind_the_stream_still_fail_the_pin() {
        let dir = tempfile::tempdir().expect("tempdir");
        let dest = dir.path().join("artifact.bin");
        let partial = partial_path(&dest);
        let url = partial.to_string_lossy().into_owned();

        let err = fetch_with_source(
            &LyingSource,
            &url,
            &dest,
            &sha256_hex(BODY),
            "artifact",
            |_, _| {},
        )
        .await
        .expect_err("disk contents must be what is verified");

        assert!(
            err.to_string().contains("SHA256 mismatch"),
            "unexpected error: {err}"
        );
        assert!(!dest.exists(), "tampered bytes must never reach dest");
    }

    /// Answers every request, including a fresh one at offset 0, with 416.
    struct AlwaysUnsatisfiable;

    impl FetchSource for AlwaysUnsatisfiable {
        type Body = FakeBody;

        async fn begin(&self, _url: &str, _offset: u64) -> Result<(FetchStart, FakeBody)> {
            Ok((
                FetchStart::RangeNotSatisfiable,
                FakeBody {
                    chunks: VecDeque::new(),
                },
            ))
        }
    }

    #[tokio::test]
    async fn range_not_satisfiable_without_a_partial_blames_the_server() {
        let dir = tempfile::tempdir().expect("tempdir");
        let dest = dir.path().join("artifact.bin");

        let err = fetch_with_source(
            &AlwaysUnsatisfiable,
            "u",
            &dest,
            &sha256_hex(BODY),
            "artifact",
            |_, _| {},
        )
        .await
        .expect_err("416 on a fresh fetch is a server fault");

        let msg = err.to_string();
        assert!(msg.contains("Server rejected download"), "got: {msg}");
        assert!(
            !msg.contains("partial file"),
            "must not blame local disk: {msg}"
        );
    }

    #[test]
    fn content_range_start_is_parsed() {
        assert_eq!(content_range_start("bytes 100-199/200"), Some(100));
        assert_eq!(content_range_start("bytes */200"), None);
        assert_eq!(content_range_start("items 0-1/2"), None);
    }

    /// A server that resumes somewhere other than where it was asked to.
    struct MisalignedSource;

    impl FetchSource for MisalignedSource {
        type Body = FakeBody;

        async fn begin(&self, _url: &str, offset: u64) -> Result<(FetchStart, FakeBody)> {
            Ok((
                FetchStart::Resumed {
                    start: Some(offset - 1),
                    total: Some(BODY.len() as u64),
                },
                chunked(&BODY[offset as usize - 1..]),
            ))
        }
    }

    #[tokio::test]
    async fn a_misaligned_resume_is_rejected() {
        let dir = tempfile::tempdir().expect("tempdir");
        let dest = dir.path().join("artifact.bin");
        std::fs::write(partial_path(&dest), &BODY[..10]).expect("seed partial");

        let err = fetch_with_source(&MisalignedSource, "u", &dest, "", "artifact", |_, _| {})
            .await
            .expect_err("misaligned resume must be rejected");

        assert!(
            err.to_string().contains("resumed artifact at byte 9"),
            "unexpected error: {err}"
        );
    }

    /// Sends headers and a first chunk, then stalls forever. Mirrors the hang
    /// the production read timeout exists to break, at test speed.
    fn stalling_server() -> (String, std::sync::mpsc::Sender<()>) {
        use std::io::Write;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
        std::thread::spawn(move || {
            if let Ok((mut sock, _)) = listener.accept() {
                // Drain the request headers before replying; hyper drops the
                // connection if the response races ahead of its own request.
                let mut reader = std::io::BufReader::new(match sock.try_clone() {
                    Ok(clone) => clone,
                    Err(_) => return,
                });
                let mut line = String::new();
                while std::io::BufRead::read_line(&mut reader, &mut line).is_ok_and(|n| n > 0) {
                    if line == "\r\n" {
                        break;
                    }
                    line.clear();
                }
                let _ = sock.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 36\r\n\r\n0123456789");
                let _ = sock.flush();
                // Hold the connection open until the test is done with it.
                let _ = stop_rx.recv();
            }
        });
        (format!("http://{addr}/artifact"), stop_tx)
    }

    #[tokio::test]
    async fn a_stalled_body_times_out_and_keeps_the_partial() {
        let dir = tempfile::tempdir().expect("tempdir");
        let dest = dir.path().join("artifact.bin");
        let (url, stop) = stalling_server();
        // Same mechanism as the production client, shortened so the test is
        // fast; CONNECT_TIMEOUT/READ_TIMEOUT differ only in magnitude.
        let source = HttpSource {
            client: reqwest::Client::builder()
                .connect_timeout(CONNECT_TIMEOUT)
                .read_timeout(std::time::Duration::from_millis(300))
                .build()
                .expect("client"),
        };

        let err = fetch_with_source(
            &source,
            &url,
            &dest,
            &sha256_hex(BODY),
            "artifact",
            |_, _| {},
        )
        .await
        .expect_err("a stalled body must not hang forever");
        let _ = stop.send(());

        assert!(!dest.exists(), "an incomplete download must not finalize");
        // The bytes that did arrive stay behind for the resume path.
        assert_eq!(
            std::fs::read(partial_path(&dest)).unwrap_or_default(),
            b"0123456789",
            "timed-out partial must survive for resume: {err:#}"
        );
    }

    #[tokio::test]
    async fn a_timed_out_download_resumes_from_its_partial() {
        let dir = tempfile::tempdir().expect("tempdir");
        let dest = dir.path().join("artifact.bin");
        std::fs::write(partial_path(&dest), &BODY[..10]).expect("partial left by a timeout");
        let src = source(BODY, true);

        let len = fetch_with_source(&src, "u", &dest, &sha256_hex(BODY), "artifact", |_, _| {})
            .await
            .expect("resume after timeout");

        assert_eq!(len, BODY.len() as u64);
        assert_eq!(std::fs::read(&dest).expect("read dest"), BODY);
        assert_eq!(requested_offsets(&src), vec![10]);
    }
}
