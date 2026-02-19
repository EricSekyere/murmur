use anyhow::{Context, Result};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::PathBuf;

const BASE_URL: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main";

/// Available Whisper model variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WhisperModel {
    BaseEn,
    SmallEn,
    MediumEn,
    LargeV3Turbo,
}

impl WhisperModel {
    /// Human-readable name.
    pub fn name(&self) -> &str {
        match self {
            Self::BaseEn => "base.en",
            Self::SmallEn => "small.en",
            Self::MediumEn => "medium.en",
            Self::LargeV3Turbo => "large-v3-turbo",
        }
    }

    /// Expected file name for the GGML model.
    pub fn filename(&self) -> &str {
        match self {
            Self::BaseEn => "ggml-base.en.bin",
            Self::SmallEn => "ggml-small.en.bin",
            Self::MediumEn => "ggml-medium.en.bin",
            Self::LargeV3Turbo => "ggml-large-v3-turbo.bin",
        }
    }

    /// Download URL on HuggingFace.
    pub fn url(&self) -> String {
        format!("{}/{}", BASE_URL, self.filename())
    }

    /// Expected SHA256 checksum.
    pub fn sha256(&self) -> &str {
        match self {
            Self::BaseEn => "a03779c86df3323075f5e796cb2ce5029f00ec8869eee3fdfb897afe36c6d002",
            Self::SmallEn => "c6138d6d58ecc8322097e0f987c32f1be8bb0a18532a3f88f734d1bbf9c41e5d",
            Self::MediumEn => "cc37e93478338ec7700281a7ac30a10128929eb8f427dda2e865faa8f6da4356",
            Self::LargeV3Turbo => "1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69",
        }
    }

    /// Approximate download size in MB.
    pub fn size_mb(&self) -> u32 {
        match self {
            Self::BaseEn => 148,
            Self::SmallEn => 488,
            Self::MediumEn => 1533,
            Self::LargeV3Turbo => 1624,
        }
    }

    /// All available models.
    pub fn all() -> &'static [WhisperModel] {
        &[
            Self::BaseEn,
            Self::SmallEn,
            Self::MediumEn,
            Self::LargeV3Turbo,
        ]
    }

    /// Parse a model name string (e.g. "base-en", "small-en").
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "base-en" | "base.en" => Some(Self::BaseEn),
            "small-en" | "small.en" => Some(Self::SmallEn),
            "medium-en" | "medium.en" => Some(Self::MediumEn),
            "large-v3-turbo" | "large-v3-turbo.en" => Some(Self::LargeV3Turbo),
            _ => None,
        }
    }
}

impl std::fmt::Display for WhisperModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
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

    /// Get the default models directory (data_dir/voitex/models/).
    pub fn default_dir() -> Result<PathBuf> {
        let dir = dirs::data_dir()
            .ok_or_else(|| anyhow::anyhow!("Could not determine data directory"))?
            .join("voitex")
            .join("models");
        Ok(dir)
    }

    /// Check if a model is already downloaded.
    pub fn is_downloaded(&self, model: WhisperModel) -> bool {
        self.model_path(model).exists()
    }

    /// Get the local file path for a model.
    pub fn model_path(&self, model: WhisperModel) -> PathBuf {
        self.models_dir.join(model.filename())
    }

    /// List all downloaded models.
    pub fn list_downloaded(&self) -> Vec<WhisperModel> {
        WhisperModel::all()
            .iter()
            .filter(|m| self.is_downloaded(**m))
            .copied()
            .collect()
    }

    /// Download a model from HuggingFace with progress reporting and SHA256 verification.
    /// Returns the path to the downloaded file.
    pub async fn download(&self, model: WhisperModel) -> Result<PathBuf> {
        std::fs::create_dir_all(&self.models_dir)
            .context("Failed to create models directory")?;

        let dest = self.model_path(model);
        let temp_path = dest.with_extension("bin.partial");

        tracing::info!("Downloading {} (~{} MB)...", model.name(), model.size_mb());

        let client = reqwest::Client::new();
        let response = client
            .get(model.url())
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
            pb.set_message(model.name().to_string());
            pb
        } else {
            let pb = indicatif::ProgressBar::new_spinner();
            pb.set_message(format!("Downloading {}...", model.name()));
            pb
        };

        let mut file = tokio::fs::File::create(&temp_path)
            .await
            .context("Failed to create temp file")?;
        let mut hasher = Sha256::new();
        let mut stream = response.bytes_stream();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.context("Error reading download stream")?;
            hasher.update(&chunk);
            tokio::io::AsyncWriteExt::write_all(&mut file, &chunk)
                .await
                .context("Failed to write chunk")?;
            pb.inc(chunk.len() as u64);
        }

        pb.finish_with_message(format!("{} downloaded", model.name()));

        // Verify SHA256 checksum
        let hash = format!("{:x}", hasher.finalize());
        let expected = model.sha256();
        if hash != expected {
            // Clean up partial file
            let _ = tokio::fs::remove_file(&temp_path).await;
            anyhow::bail!(
                "SHA256 mismatch for {}: expected {}, got {}",
                model.name(),
                expected,
                hash
            );
        }

        tracing::info!("Checksum verified for {}", model.name());

        // Atomic rename from temp to final path
        tokio::fs::rename(&temp_path, &dest)
            .await
            .context("Failed to finalize downloaded model")?;

        tracing::info!("Model saved to {}", dest.display());
        Ok(dest)
    }
}
