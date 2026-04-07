use anyhow::{Context, Result};
use futures_util::StreamExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

const ORT_VERSION: &str = "1.23.0";

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
/// Downloads the release archive, extracts the shared library, and caches it
/// in the app data directory. Returns the path to the extracted DLL.
pub async fn download() -> Result<PathBuf> {
    let dir = ort_dir()?;
    std::fs::create_dir_all(&dir).context("Failed to create ONNX Runtime directory")?;

    let url = download_url();
    tracing::info!("Downloading ONNX Runtime v{} from {}", ORT_VERSION, url);

    let client = reqwest::Client::new();
    let response = client
        .get(&url)
        .send()
        .await
        .context("Failed to download ONNX Runtime")?
        .error_for_status()
        .context("ONNX Runtime download request failed")?;

    let bytes = response
        .bytes()
        .await
        .context("Failed to read ONNX Runtime download")?;

    tracing::info!("Downloaded {} bytes, extracting DLL...", bytes.len());

    let dir_clone = dir.clone();
    tokio::task::spawn_blocking(move || extract_dll(&bytes, &dir_clone))
        .await
        .context("Extraction task panicked")?
        .context("Failed to extract ONNX Runtime DLL")?;

    let path = dir.join(DLL_FILENAME);
    tracing::info!("ONNX Runtime DLL ready at {}", path.display());
    Ok(path)
}

/// Download the ONNX Runtime DLL with a progress callback.
///
/// The callback receives `(bytes_downloaded, total_bytes)`.
pub async fn download_with_progress<F>(on_progress: F) -> Result<PathBuf>
where
    F: Fn(u64, Option<u64>),
{
    let dir = ort_dir()?;
    std::fs::create_dir_all(&dir).context("Failed to create ONNX Runtime directory")?;

    let url = download_url();
    tracing::info!("Downloading ONNX Runtime v{} from {}", ORT_VERSION, url);

    let client = reqwest::Client::new();
    let response = client
        .get(&url)
        .send()
        .await
        .context("Failed to download ONNX Runtime")?
        .error_for_status()
        .context("ONNX Runtime download request failed")?;

    let total_size = response.content_length();
    let mut downloaded: u64 = 0;
    let mut all_bytes = Vec::new();
    let mut stream = response.bytes_stream();

    on_progress(0, total_size);

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("Error reading download stream")?;
        downloaded += chunk.len() as u64;
        all_bytes.extend_from_slice(&chunk);
        on_progress(downloaded, total_size);
    }

    tracing::info!("Downloaded {} bytes, extracting DLL...", all_bytes.len());

    let dir_clone = dir.clone();
    tokio::task::spawn_blocking(move || extract_dll(&all_bytes, &dir_clone))
        .await
        .context("Extraction task panicked")?
        .context("Failed to extract ONNX Runtime DLL")?;

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
            let mut out = std::fs::File::create(&dest)
                .with_context(|| format!("Failed to create {}", dest.display()))?;
            std::io::copy(&mut entry, &mut out).context("Failed to extract DLL from archive")?;
            tracing::info!("Extracted {} ({} bytes)", name, entry.size());
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

        if let Some(name) = path.file_name() {
            if name.to_str() == Some(DLL_FILENAME) {
                let dest = dest_dir.join(DLL_FILENAME);
                let mut out = std::fs::File::create(&dest)
                    .with_context(|| format!("Failed to create {}", dest.display()))?;
                std::io::copy(&mut entry, &mut out)
                    .context("Failed to extract DLL from archive")?;
                tracing::info!("Extracted {}", path.display());
                return Ok(());
            }
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
