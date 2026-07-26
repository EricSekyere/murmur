use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

const ORT_VERSION: &str = "1.23.0";

/// Pinned SHA256 of the ONNX Runtime release archive for this platform. These
/// are immutable GitHub release assets, so the hashes are stable. Every target
/// `download_url()` can fetch is pinned so the checksum gate never degrades to
/// a warning; keep these in step with `download_url()` and re-pin on every
/// ORT_VERSION bump.
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
const ORT_ARCHIVE_SHA256: &str = "72c23470310ec79a7d42d27fe9d257e6c98540c73fa5a1db1f67f538c6c16f2f";

#[cfg(all(target_os = "windows", target_arch = "aarch64"))]
const ORT_ARCHIVE_SHA256: &str = "097352d00a398097db2db7a33ea015cf725cac5d5b95b48bf8eff0cd154d3621";

// The osx-universal2 archive covers both Apple Silicon and Intel.
#[cfg(target_os = "macos")]
const ORT_ARCHIVE_SHA256: &str = "5e4365fb4a05aef353f6232b9a1848f37e608c421c9227e9224572205c0cfc08";

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
const ORT_ARCHIVE_SHA256: &str = "b6deea7f2e22c10c043019f294a0ea4d2a6c0ae52a009c34847640db75ec5580";

#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
const ORT_ARCHIVE_SHA256: &str = "0b9f47d140411d938e47915824d8daaa424df95a88b5f1fc843172a75168f7a0";

#[cfg(not(any(
    all(target_os = "windows", target_arch = "x86_64"),
    all(target_os = "windows", target_arch = "aarch64"),
    target_os = "macos",
    all(target_os = "linux", target_arch = "x86_64"),
    all(target_os = "linux", target_arch = "aarch64")
)))]
const ORT_ARCHIVE_SHA256: &str = "";

/// ONNX Runtime DLL filename for the current platform.
#[cfg(target_os = "windows")]
const DLL_FILENAME: &str = "onnxruntime.dll";

#[cfg(target_os = "macos")]
const DLL_FILENAME: &str = "libonnxruntime.dylib";

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
const DLL_FILENAME: &str = "libonnxruntime.so";

/// Download URL for the ONNX Runtime release archive.
fn download_url() -> String {
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    {
        format!(
            "https://github.com/microsoft/onnxruntime/releases/download/v{v}/onnxruntime-win-x64-{v}.zip",
            v = ORT_VERSION
        )
    }

    #[cfg(all(target_os = "windows", target_arch = "aarch64"))]
    {
        format!(
            "https://github.com/microsoft/onnxruntime/releases/download/v{v}/onnxruntime-win-arm64-{v}.zip",
            v = ORT_VERSION
        )
    }

    #[cfg(target_os = "macos")]
    {
        format!(
            "https://github.com/microsoft/onnxruntime/releases/download/v{v}/onnxruntime-osx-universal2-{v}.tgz",
            v = ORT_VERSION
        )
    }

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        format!(
            "https://github.com/microsoft/onnxruntime/releases/download/v{v}/onnxruntime-linux-x64-{v}.tgz",
            v = ORT_VERSION
        )
    }

    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    {
        format!(
            "https://github.com/microsoft/onnxruntime/releases/download/v{v}/onnxruntime-linux-aarch64-{v}.tgz",
            v = ORT_VERSION
        )
    }
}

/// Directory where the ONNX Runtime DLL is cached.
pub fn ort_dir() -> Result<PathBuf> {
    let dir = dirs::data_dir()
        .ok_or_else(|| anyhow::anyhow!("Could not determine data directory"))?
        .join("murmur")
        .join("onnxruntime");
    Ok(dir)
}

/// Full path to the ONNX Runtime DLL.
pub fn dll_path() -> Result<PathBuf> {
    Ok(ort_dir()?.join(DLL_FILENAME))
}

/// Check if the ONNX Runtime DLL is already downloaded.
pub fn is_downloaded() -> bool {
    dll_path().map(|p| p.exists()).unwrap_or(false)
}

/// Download the ONNX Runtime DLL from Microsoft's GitHub releases.
///
/// Downloads the release archive (resuming an interrupted transfer from its
/// `.partial` file), extracts the shared library, and caches it in the app
/// data directory. Returns the path to the extracted DLL.
pub async fn download() -> Result<PathBuf> {
    download_with_progress(|_, _| {}).await
}

/// Download the ONNX Runtime DLL with a progress callback.
///
/// The callback receives `(bytes_downloaded, total_bytes)`.
pub async fn download_with_progress<F>(on_progress: F) -> Result<PathBuf>
where
    F: Fn(u64, Option<u64>),
{
    let dir = ort_dir()?;
    let url = download_url();
    tracing::info!("Downloading ONNX Runtime v{} from {}", ORT_VERSION, url);

    let archive_name = url.rsplit('/').next().unwrap_or("onnxruntime-archive");
    let archive_path = dir.join(archive_name);
    crate::download::fetch_to_file(
        &url,
        &archive_path,
        ORT_ARCHIVE_SHA256,
        "ONNX Runtime archive",
        on_progress,
    )
    .await?;

    let bytes = tokio::fs::read(&archive_path)
        .await
        .context("Failed to read ONNX Runtime archive")?;
    tracing::info!("Downloaded {} bytes, extracting DLL...", bytes.len());

    let dir_clone = dir.clone();
    let extracted = tokio::task::spawn_blocking(move || extract_dll(&bytes, &dir_clone))
        .await
        .context("Extraction task panicked")?;
    // The verified archive only exists to be extracted; drop it on failure
    // too, so a retry starts clean instead of trusting a half-used file.
    let _ = tokio::fs::remove_file(&archive_path).await;
    extracted.context("Failed to extract ONNX Runtime DLL")?;

    let path = dir.join(DLL_FILENAME);
    tracing::info!("ONNX Runtime DLL ready at {}", path.display());
    Ok(path)
}

/// Extract the DLL from a ZIP archive (Windows).
#[cfg(target_os = "windows")]
fn extract_dll(archive_bytes: &[u8], dest_dir: &Path) -> Result<()> {
    use std::io::Cursor;

    let reader = Cursor::new(archive_bytes);
    let mut archive = zip::ZipArchive::new(reader).context("Failed to read ZIP archive")?;

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).context("Failed to read ZIP entry")?;
        let name = entry.name().to_string();

        // The DLL is at <prefix>/lib/onnxruntime.dll inside the archive
        if name.ends_with(&format!("/lib/{}", DLL_FILENAME)) || name == DLL_FILENAME {
            let dest = dest_dir.join(DLL_FILENAME);
            // Extract to memory, then write atomically: a partial write at the
            // final path passes the exists() check forever and permanently
            // breaks every ORT consumer until manually deleted.
            let mut buf = Vec::with_capacity(entry.size() as usize);
            std::io::copy(&mut entry, &mut buf).context("Failed to extract DLL from archive")?;
            crate::fsutil::atomic_write(&dest, &buf)
                .with_context(|| format!("Failed to write {}", dest.display()))?;
            tracing::info!("Extracted {} ({} bytes)", name, buf.len());
            return Ok(());
        }
    }

    anyhow::bail!(
        "Could not find {} in the ONNX Runtime archive",
        DLL_FILENAME
    )
}

/// Extract the DLL from a .tgz archive (macOS/Linux).
#[cfg(not(target_os = "windows"))]
fn extract_dll(archive_bytes: &[u8], dest_dir: &Path) -> Result<()> {
    use flate2::read::GzDecoder;
    use std::io::Cursor;

    let reader = Cursor::new(archive_bytes);
    let gz = GzDecoder::new(reader);
    let mut archive = tar::Archive::new(gz);

    for entry in archive.entries().context("Failed to read tar archive")? {
        let mut entry = entry.context("Failed to read tar entry")?;
        let path = entry
            .path()
            .context("Failed to read entry path")?
            .into_owned();

        if let Some(name) = path.file_name()
            && name.to_str() == Some(DLL_FILENAME)
        {
            let dest = dest_dir.join(DLL_FILENAME);
            // Extract to memory, then write atomically: a partial write at the
            // final path passes the exists() check forever and permanently
            // breaks every ORT consumer until manually deleted.
            let mut buf = Vec::new();
            std::io::copy(&mut entry, &mut buf).context("Failed to extract DLL from archive")?;
            crate::fsutil::atomic_write(&dest, &buf)
                .with_context(|| format!("Failed to write {}", dest.display()))?;
            tracing::info!("Extracted {}", path.display());
            return Ok(());
        }
    }

    anyhow::bail!(
        "Could not find {} in the ONNX Runtime archive",
        DLL_FILENAME
    )
}

static ORT_INITIALIZED: OnceLock<Result<(), String>> = OnceLock::new();

/// Initialize the ONNX Runtime environment with the downloaded DLL.
///
/// Must be called before any ort session creation (e.g., before parakeet-rs).
/// Thread-safe; only the first call performs initialization, subsequent calls are no-ops.
pub fn init_ort() -> Result<()> {
    let path = dll_path().context("Failed to determine ORT DLL path")?;

    if !path.exists() {
        anyhow::bail!(
            "ONNX Runtime DLL not found at {}. Download it first.",
            path.display()
        );
    }

    let result = ORT_INITIALIZED.get_or_init(|| {
        tracing::info!("Initializing ONNX Runtime from {}", path.display());
        match ort::init_from(&path) {
            Ok(builder) => {
                builder.commit();
                tracing::info!("ONNX Runtime initialized successfully");
                Ok(())
            }
            Err(e) => {
                tracing::error!("ONNX Runtime init_from failed: {}", e);
                Err(format!("{}", e))
            }
        }
    });

    match result {
        Ok(()) => Ok(()),
        Err(e) => {
            tracing::error!("ONNX Runtime initialization failed (cached error): {}", e);
            Err(anyhow::anyhow!("ONNX Runtime initialization failed: {}", e))
        }
    }
}

/// Apply the low-memory ORT session options shared by every in-process model
/// (Parakeet, Silero VAD, the Help embedder). Call on a fresh builder before
/// `commit_*`.
///
/// Disables two ORT defaults that bloat the resident set of a long-lived app
/// running short, bursty inferences:
/// - the CPU memory **arena**, which keeps each session's peak inference
///   activations reserved for the session's whole lifetime and never returns
///   them to the OS, so the app stays pinned at its busiest size even while
///   idle between utterances;
/// - the memory-**pattern** planner, which pre-reserves one contiguous
///   activation buffer sized for static shapes — wasted on our
///   variable-length audio inputs.
///
/// The trade is a little more per-inference allocation for a markedly smaller
/// idle resident set, which suits a dictation tool that sits idle between
/// phrases.
pub fn apply_low_memory(
    builder: ort::session::builder::SessionBuilder,
) -> ort::Result<ort::session::builder::SessionBuilder> {
    builder
        .with_memory_pattern(false)?
        .with_execution_providers([ort::ep::CPU::default().with_arena_allocator(false).build()])
}
