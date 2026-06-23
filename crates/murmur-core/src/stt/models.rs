use anyhow::{Context, Result};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::PathBuf;

const WHISPER_BASE_URL: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main";
const PARAKEET_BASE_URL: &str =
    "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v2-onnx/resolve/main";

/// STT backend type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Backend {
    Whisper,
    Parakeet,
}

impl std::fmt::Display for Backend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Whisper => write!(f, "whisper"),
            Self::Parakeet => write!(f, "parakeet"),
        }
    }
}

/// File to download for a model.
pub struct ModelFile {
    /// Remote filename on HuggingFace.
    pub remote_name: &'static str,
    /// Local filename to save as.
    pub local_name: &'static str,
    /// Expected SHA256 checksum.
    pub sha256: &'static str,
}

/// Available STT model variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SttModel {
    // Whisper models (via whisper-rs, GGML format)
    #[serde(alias = "base-en")]
    WhisperBaseEn,
    #[serde(alias = "small-en")]
    WhisperSmallEn,
    #[serde(alias = "medium-en")]
    WhisperMediumEn,
    #[serde(alias = "large-v3-turbo")]
    WhisperLargeV3Turbo,
    // Parakeet models (via parakeet-rs, ONNX format)
    ParakeetTdt06bV2,
}

impl SttModel {
    /// Human-readable name for display.
    pub fn name(&self) -> &str {
        match self {
            Self::WhisperBaseEn => "Whisper Base (English)",
            Self::WhisperSmallEn => "Whisper Small (English)",
            Self::WhisperMediumEn => "Whisper Medium (English)",
            Self::WhisperLargeV3Turbo => "Whisper Large v3 Turbo",
            Self::ParakeetTdt06bV2 => "Parakeet TDT 0.6B v2",
        }
    }

    /// Short name used in logs and CLI (backward compat with old WhisperModel names).
    pub fn short_name(&self) -> &str {
        match self {
            Self::WhisperBaseEn => "base.en",
            Self::WhisperSmallEn => "small.en",
            Self::WhisperMediumEn => "medium.en",
            Self::WhisperLargeV3Turbo => "large-v3-turbo",
            Self::ParakeetTdt06bV2 => "parakeet-tdt-0.6b-v2",
        }
    }

    /// Serde ID string (kebab-case, matches serde serialization).
    pub fn id(&self) -> &str {
        match self {
            Self::WhisperBaseEn => "whisper-base-en",
            Self::WhisperSmallEn => "whisper-small-en",
            Self::WhisperMediumEn => "whisper-medium-en",
            Self::WhisperLargeV3Turbo => "whisper-large-v3-turbo",
            Self::ParakeetTdt06bV2 => "parakeet-tdt-06b-v2",
        }
    }

    /// Which STT backend this model uses.
    pub fn backend(&self) -> Backend {
        match self {
            Self::WhisperBaseEn
            | Self::WhisperSmallEn
            | Self::WhisperMediumEn
            | Self::WhisperLargeV3Turbo => Backend::Whisper,
            Self::ParakeetTdt06bV2 => Backend::Parakeet,
        }
    }

    /// Whether the model can transcribe languages other than English and
    /// translate to English. Only the multilingual Whisper checkpoints can;
    /// the `.en` models and Parakeet are English-only.
    pub fn is_multilingual(&self) -> bool {
        matches!(self, Self::WhisperLargeV3Turbo)
    }

    /// Approximate total download size in MB.
    pub fn size_mb(&self) -> u32 {
        match self {
            Self::WhisperBaseEn => 148,
            Self::WhisperSmallEn => 488,
            Self::WhisperMediumEn => 1533,
            Self::WhisperLargeV3Turbo => 1624,
            Self::ParakeetTdt06bV2 => 661,
        }
    }

    /// Short description for UI display.
    pub fn description(&self) -> &str {
        match self {
            Self::WhisperBaseEn => "Fast, lower accuracy",
            Self::WhisperSmallEn => "Good balance of speed and accuracy",
            Self::WhisperMediumEn => "Higher accuracy, slower. Needs 4 GB+ RAM",
            Self::WhisperLargeV3Turbo => "Best Whisper accuracy, slowest. Needs 6 GB+ RAM",
            Self::ParakeetTdt06bV2 => "Best accuracy, native punctuation & capitalization",
        }
    }

    /// Estimated RAM usage during inference in MB.
    pub fn ram_estimate_mb(&self) -> u32 {
        match self {
            Self::WhisperBaseEn => 400,
            Self::WhisperSmallEn => 1000,
            Self::WhisperMediumEn => 3500,
            Self::WhisperLargeV3Turbo => 5000,
            Self::ParakeetTdt06bV2 => 1500,
        }
    }

    /// All available models.
    pub fn all() -> &'static [SttModel] {
        &[
            Self::WhisperBaseEn,
            Self::WhisperSmallEn,
            Self::WhisperMediumEn,
            Self::WhisperLargeV3Turbo,
            Self::ParakeetTdt06bV2,
        ]
    }

    /// Parse a model name or ID string.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            // Kebab-case IDs (serde format)
            "whisper-base-en" => Some(Self::WhisperBaseEn),
            "whisper-small-en" => Some(Self::WhisperSmallEn),
            "whisper-medium-en" => Some(Self::WhisperMediumEn),
            "whisper-large-v3-turbo" => Some(Self::WhisperLargeV3Turbo),
            "parakeet-tdt-06b-v2" => Some(Self::ParakeetTdt06bV2),
            // Legacy short names (backward compat)
            "base-en" | "base.en" => Some(Self::WhisperBaseEn),
            "small-en" | "small.en" => Some(Self::WhisperSmallEn),
            "medium-en" | "medium.en" => Some(Self::WhisperMediumEn),
            "large-v3-turbo" | "large-v3-turbo.en" => Some(Self::WhisperLargeV3Turbo),
            "parakeet-tdt-0.6b-v2" => Some(Self::ParakeetTdt06bV2),
            _ => None,
        }
    }

    /// Files that need to be downloaded for this model.
    fn model_files(&self) -> Vec<ModelFile> {
        match self {
            Self::WhisperBaseEn => vec![ModelFile {
                remote_name: "ggml-base.en.bin",
                local_name: "ggml-base.en.bin",
                sha256: "a03779c86df3323075f5e796cb2ce5029f00ec8869eee3fdfb897afe36c6d002",
            }],
            Self::WhisperSmallEn => vec![ModelFile {
                remote_name: "ggml-small.en.bin",
                local_name: "ggml-small.en.bin",
                sha256: "c6138d6d58ecc8322097e0f987c32f1be8bb0a18532a3f88f734d1bbf9c41e5d",
            }],
            Self::WhisperMediumEn => vec![ModelFile {
                remote_name: "ggml-medium.en.bin",
                local_name: "ggml-medium.en.bin",
                sha256: "cc37e93478338ec7700281a7ac30a10128929eb8f427dda2e865faa8f6da4356",
            }],
            Self::WhisperLargeV3Turbo => vec![ModelFile {
                remote_name: "ggml-large-v3-turbo.bin",
                local_name: "ggml-large-v3-turbo.bin",
                sha256: "1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69",
            }],
            Self::ParakeetTdt06bV2 => vec![
                // Pinned to istupakov/parakeet-tdt-0.6b-v2-onnx @ main (the LFS
                // oid is the file's SHA256; verified against the resolved bytes).
                ModelFile {
                    remote_name: "encoder-model.int8.onnx",
                    local_name: "encoder-model.onnx",
                    sha256: "3e0581fda6ab843888b51e56d7ee78b6d5bc3237ec113af1f732d1d5286aa155",
                },
                ModelFile {
                    remote_name: "decoder_joint-model.int8.onnx",
                    local_name: "decoder_joint-model.onnx",
                    sha256: "a449f49acd68979d418651dd2dcb737cc0f1bf0225e009e29ee326354edbf7d3",
                },
                ModelFile {
                    remote_name: "vocab.txt",
                    local_name: "vocab.txt",
                    sha256: "ec182b70dd42113aff6c5372c75cac58c952443eb22322f57bbd7f53977d497d",
                },
            ],
        }
    }

    /// Base download URL for this model's files.
    fn base_url(&self) -> &str {
        match self.backend() {
            Backend::Whisper => WHISPER_BASE_URL,
            Backend::Parakeet => PARAKEET_BASE_URL,
        }
    }
}

impl std::fmt::Display for SttModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.short_name())
    }
}

/// Manages model downloads, storage, and selection.
pub struct ModelManager {
    models_dir: PathBuf,
}

impl ModelManager {
    /// Create a new ModelManager that stores models in the given directory.
    pub fn new(models_dir: PathBuf) -> Self {
        Self { models_dir }
    }

    /// Get the default models directory (data_dir/murmur/models/).
    pub fn default_dir() -> Result<PathBuf> {
        let dir = dirs::data_dir()
            .ok_or_else(|| anyhow::anyhow!("Could not determine data directory"))?
            .join("murmur")
            .join("models");
        Ok(dir)
    }

    /// Get the directory or file path where a model's files are stored.
    /// Whisper: single file in models_dir.
    /// Parakeet: subdirectory in models_dir.
    pub fn model_dir(&self, model: SttModel) -> PathBuf {
        match model.backend() {
            Backend::Whisper => self.models_dir.clone(),
            Backend::Parakeet => self.models_dir.join(model.short_name()),
        }
    }

    /// Get the primary model path for engine initialization.
    /// Whisper: path to the .bin file.
    /// Parakeet: path to the model directory.
    pub fn model_path(&self, model: SttModel) -> PathBuf {
        match model.backend() {
            Backend::Whisper => {
                let files = model.model_files();
                self.models_dir.join(files[0].local_name)
            }
            Backend::Parakeet => self.model_dir(model),
        }
    }

    /// Check if all required files for a model are present and non-empty.
    /// Empty or missing files (e.g. an interrupted download that left a stub)
    /// are treated as not downloaded so the file is fetched again.
    pub fn is_downloaded(&self, model: SttModel) -> bool {
        let dir = self.model_dir(model);
        model.model_files().iter().all(|f| {
            dir.join(f.local_name)
                .metadata()
                .map(|m| m.len() > 0)
                .unwrap_or(false)
        })
    }

    /// List all downloaded models.
    pub fn list_downloaded(&self) -> Vec<SttModel> {
        SttModel::all()
            .iter()
            .filter(|m| self.is_downloaded(**m))
            .copied()
            .collect()
    }

    /// Download a model with a terminal progress bar and SHA256 verification.
    pub async fn download(&self, model: SttModel) -> Result<PathBuf> {
        let dir = self.model_dir(model);
        std::fs::create_dir_all(&dir).context("Failed to create model directory")?;

        let files = model.model_files();
        let client = reqwest::Client::new();

        for file in &files {
            let url = format!("{}/{}", model.base_url(), file.remote_name);
            let dest = dir.join(file.local_name);
            let temp_path = dest.with_extension("partial");

            tracing::info!("Downloading {}...", file.remote_name);

            let response = client
                .get(&url)
                .send()
                .await
                .context("Failed to start download")?
                .error_for_status()
                .context("Download request failed")?;

            let total_size = response.content_length();

            let pb = if let Some(total) = total_size {
                let pb = indicatif::ProgressBar::new(total);
                pb.set_style(
                    indicatif::ProgressStyle::default_bar()
                        .template("{msg} [{bar:40}] {bytes}/{total_bytes} ({eta})")
                        .expect("valid template")
                        .progress_chars("=> "),
                );
                pb.set_message(file.local_name.to_string());
                pb
            } else {
                let pb = indicatif::ProgressBar::new_spinner();
                pb.set_message(format!("Downloading {}...", file.local_name));
                pb
            };

            let mut out = tokio::fs::File::create(&temp_path)
                .await
                .context("Failed to create temp file")?;
            let mut hasher = Sha256::new();
            let mut stream = response.bytes_stream();

            while let Some(chunk) = stream.next().await {
                let chunk = chunk.context("Error reading download stream")?;
                hasher.update(&chunk);
                tokio::io::AsyncWriteExt::write_all(&mut out, &chunk)
                    .await
                    .context("Failed to write chunk")?;
                pb.inc(chunk.len() as u64);
            }

            pb.finish_with_message(format!("{} downloaded", file.local_name));

            let hash = format!("{:x}", hasher.finalize());
            verify_download(&temp_path, file, &hash).await?;

            tokio::fs::rename(&temp_path, &dest)
                .await
                .context("Failed to finalize downloaded file")?;
        }

        let path = self.model_path(model);
        tracing::info!("Model {} ready at {}", model.name(), path.display());
        Ok(path)
    }

    /// Download a model with a progress callback instead of a terminal progress bar.
    ///
    /// The callback receives `(bytes_downloaded, total_bytes)` where `total_bytes`
    /// is the total across all files for this model.
    pub async fn download_with_progress<F>(
        &self,
        model: SttModel,
        on_progress: F,
    ) -> Result<PathBuf>
    where
        F: Fn(u64, Option<u64>),
    {
        let dir = self.model_dir(model);
        std::fs::create_dir_all(&dir).context("Failed to create model directory")?;

        let files = model.model_files();
        let client = reqwest::Client::new();

        // Estimate total size from model size_mb (for progress across all files)
        let estimated_total = (model.size_mb() as u64) * 1024 * 1024;
        let mut cumulative_downloaded: u64 = 0;

        on_progress(0, Some(estimated_total));

        for file in &files {
            let url = format!("{}/{}", model.base_url(), file.remote_name);
            let dest = dir.join(file.local_name);
            let temp_path = dest.with_extension("partial");

            tracing::info!("Downloading {}...", file.remote_name);

            let response = client
                .get(&url)
                .send()
                .await
                .context("Failed to start download")?
                .error_for_status()
                .context("Download request failed")?;

            let mut out = tokio::fs::File::create(&temp_path)
                .await
                .context("Failed to create temp file")?;
            let mut hasher = Sha256::new();
            let mut stream = response.bytes_stream();

            while let Some(chunk) = stream.next().await {
                let chunk = chunk.context("Error reading download stream")?;
                hasher.update(&chunk);
                tokio::io::AsyncWriteExt::write_all(&mut out, &chunk)
                    .await
                    .context("Failed to write chunk")?;
                cumulative_downloaded += chunk.len() as u64;
                on_progress(cumulative_downloaded, Some(estimated_total));
            }

            let hash = format!("{:x}", hasher.finalize());
            verify_download(&temp_path, file, &hash).await?;

            tokio::fs::rename(&temp_path, &dest)
                .await
                .context("Failed to finalize downloaded file")?;
        }

        let path = self.model_path(model);
        tracing::info!("Model {} ready at {}", model.name(), path.display());
        Ok(path)
    }
}

/// Reject an empty download and verify its SHA256 when one is pinned. A model
/// file without a pinned checksum cannot be verified, so warn loudly rather
/// than accepting it silently.
async fn verify_download(temp_path: &std::path::Path, file: &ModelFile, hash: &str) -> Result<()> {
    let len = tokio::fs::metadata(temp_path)
        .await
        .map(|m| m.len())
        .unwrap_or(0);
    if len == 0 {
        let _ = tokio::fs::remove_file(temp_path).await;
        anyhow::bail!("Downloaded file {} is empty", file.local_name);
    }

    if file.sha256.is_empty() {
        tracing::warn!(
            "No pinned checksum for {}; integrity cannot be verified (sha256={})",
            file.local_name,
            hash
        );
        return Ok(());
    }

    if hash != file.sha256 {
        let _ = tokio::fs::remove_file(temp_path).await;
        anyhow::bail!(
            "SHA256 mismatch for {}: expected {}, got {}",
            file.local_name,
            file.sha256,
            hash
        );
    }
    tracing::info!("Checksum verified for {}", file.local_name);
    Ok(())
}
