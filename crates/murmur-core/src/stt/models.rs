use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const WHISPER_BASE_URL: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main";
const PARAKEET_V2_BASE_URL: &str =
    "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v2-onnx/resolve/main";
const PARAKEET_V3_BASE_URL: &str =
    "https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx/resolve/main";

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
    // Parakeet models (via parakeet-rs, ONNX format). Renamed explicitly:
    // rename_all's kebab-case yields "parakeet-tdt06b-v2", which diverges from
    // the documented id() form and made the documented ID fail the whole
    // Settings parse (resetting the config). The alias keeps configs written
    // with the old serde form loading.
    #[serde(rename = "parakeet-tdt-06b-v2", alias = "parakeet-tdt06b-v2")]
    ParakeetTdt06bV2,
    #[serde(rename = "parakeet-tdt-06b-v3", alias = "parakeet-tdt06b-v3")]
    ParakeetTdt06bV3,
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
            Self::ParakeetTdt06bV3 => "Parakeet TDT 0.6B v3",
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
            Self::ParakeetTdt06bV3 => "parakeet-tdt-0.6b-v3",
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
            Self::ParakeetTdt06bV3 => "parakeet-tdt-06b-v3",
        }
    }

    /// Which STT backend this model uses.
    pub fn backend(&self) -> Backend {
        match self {
            Self::WhisperBaseEn
            | Self::WhisperSmallEn
            | Self::WhisperMediumEn
            | Self::WhisperLargeV3Turbo => Backend::Whisper,
            Self::ParakeetTdt06bV2 | Self::ParakeetTdt06bV3 => Backend::Parakeet,
        }
    }

    /// Whether the model can transcribe languages other than English.
    /// The `.en` Whisper models and Parakeet v2 are English-only. Whisper
    /// Large v3 Turbo also honors the translate-to-English toggle; Parakeet
    /// v3 covers 25 European languages with automatic detection but always
    /// transcribes in the spoken language.
    pub fn is_multilingual(&self) -> bool {
        matches!(self, Self::WhisperLargeV3Turbo | Self::ParakeetTdt06bV3)
    }

    /// Whether the model honors the translate-to-English toggle. Parakeet v3
    /// is multilingual but always transcribes in the spoken language; the
    /// engine never applies translation on the Parakeet path.
    pub fn supports_translation(&self) -> bool {
        matches!(self, Self::WhisperLargeV3Turbo)
    }

    /// Whether a forced Speech Language is applied. Parakeet v3 detects the
    /// spoken language automatically and ignores the setting.
    pub fn supports_forced_language(&self) -> bool {
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
            Self::ParakeetTdt06bV3 => 670,
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
            Self::ParakeetTdt06bV3 => "Best accuracy, 25 languages with auto-detect",
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
            Self::ParakeetTdt06bV3 => 1600,
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
            Self::ParakeetTdt06bV3,
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
            "parakeet-tdt-06b-v3" => Some(Self::ParakeetTdt06bV3),
            // Legacy short names (backward compat)
            "base-en" | "base.en" => Some(Self::WhisperBaseEn),
            "small-en" | "small.en" => Some(Self::WhisperSmallEn),
            "medium-en" | "medium.en" => Some(Self::WhisperMediumEn),
            "large-v3-turbo" | "large-v3-turbo.en" => Some(Self::WhisperLargeV3Turbo),
            "parakeet-tdt-0.6b-v2" => Some(Self::ParakeetTdt06bV2),
            "parakeet-tdt-0.6b-v3" => Some(Self::ParakeetTdt06bV3),
            // Serde form written by configs before the explicit rename above
            "parakeet-tdt06b-v2" => Some(Self::ParakeetTdt06bV2),
            "parakeet-tdt06b-v3" => Some(Self::ParakeetTdt06bV3),
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
            Self::ParakeetTdt06bV3 => vec![
                // Pinned to istupakov/parakeet-tdt-0.6b-v3-onnx @ main. To
                // re-pin: the ONNX files are LFS, so their SHA256 is the
                // `oid sha256:` in the pointer at <repo>/raw/main/<file>
                // (also `lfs.oid` in /api/models/<repo>/tree/main);
                // vocab.txt is not LFS, hash its raw bytes directly.
                ModelFile {
                    remote_name: "encoder-model.int8.onnx",
                    local_name: "encoder-model.onnx",
                    sha256: "6139d2fa7e1b086097b277c7149725edbab89cc7c7ae64b23c741be4055aff09",
                },
                ModelFile {
                    remote_name: "decoder_joint-model.int8.onnx",
                    local_name: "decoder_joint-model.onnx",
                    sha256: "eea7483ee3d1a30375daedc8ed83e3960c91b098812127a0d99d1c8977667a70",
                },
                ModelFile {
                    remote_name: "vocab.txt",
                    local_name: "vocab.txt",
                    sha256: "d58544679ea4bc6ac563d1f545eb7d474bd6cfa467f0a6e2c1dc1c7d37e3c35d",
                },
            ],
        }
    }

    /// Base download URL for this model's files.
    fn base_url(&self) -> &str {
        match self {
            Self::WhisperBaseEn
            | Self::WhisperSmallEn
            | Self::WhisperMediumEn
            | Self::WhisperLargeV3Turbo => WHISPER_BASE_URL,
            Self::ParakeetTdt06bV2 => PARAKEET_V2_BASE_URL,
            Self::ParakeetTdt06bV3 => PARAKEET_V3_BASE_URL,
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
    /// Files already fully downloaded are skipped; an interrupted file resumes
    /// from its `.partial` remainder.
    pub async fn download(&self, model: SttModel) -> Result<PathBuf> {
        let dir = self.model_dir(model);
        for file in &model.model_files() {
            let dest = dir.join(file.local_name);
            if file_len(&dest) > 0 {
                continue;
            }
            let url = format!("{}/{}", model.base_url(), file.remote_name);
            tracing::info!("Downloading {}...", file.remote_name);

            let pb = indicatif::ProgressBar::new(0);
            pb.set_style(
                indicatif::ProgressStyle::default_bar()
                    .template("{msg} [{bar:40}] {bytes}/{total_bytes} ({eta})")
                    .unwrap_or_else(|_| indicatif::ProgressStyle::default_bar())
                    .progress_chars("=> "),
            );
            pb.set_message(file.local_name.to_string());

            crate::download::fetch_to_file(
                &url,
                &dest,
                file.sha256,
                file.local_name,
                |done, total| {
                    if let Some(total) = total {
                        pb.set_length(total);
                    }
                    pb.set_position(done);
                },
            )
            .await?;

            pb.finish_with_message(format!("{} downloaded", file.local_name));
        }

        let path = self.model_path(model);
        tracing::info!("Model {} ready at {}", model.name(), path.display());
        Ok(path)
    }

    /// Download a model with a progress callback instead of a terminal progress bar.
    ///
    /// The callback receives `(bytes_downloaded, total_bytes)` where `total_bytes`
    /// is the estimated total across all files for this model. Bytes already on
    /// disk (finished files and resumable partials) count from the first report,
    /// so a resumed download's bar continues instead of restarting at 0%.
    pub async fn download_with_progress<F>(
        &self,
        model: SttModel,
        on_progress: F,
    ) -> Result<PathBuf>
    where
        F: Fn(u64, Option<u64>),
    {
        let dir = self.model_dir(model);
        let files = model.model_files();
        let estimated_total = (model.size_mb() as u64) * 1024 * 1024;

        let preexisting: u64 = files
            .iter()
            .map(|f| {
                let dest = dir.join(f.local_name);
                let done = file_len(&dest);
                if done > 0 {
                    done
                } else {
                    file_len(&crate::download::partial_path(&dest))
                }
            })
            .sum();
        on_progress(preexisting, Some(estimated_total));

        let mut completed: u64 = 0;
        for file in &files {
            let dest = dir.join(file.local_name);
            let already = file_len(&dest);
            if already > 0 {
                // Present and non-empty means an earlier attempt passed
                // verification (the rename only happens after the checksum).
                completed += already;
                continue;
            }
            let url = format!("{}/{}", model.base_url(), file.remote_name);
            tracing::info!("Downloading {}...", file.remote_name);
            let base = completed;
            let written = crate::download::fetch_to_file(
                &url,
                &dest,
                file.sha256,
                file.local_name,
                |done, _| on_progress(base + done, Some(estimated_total)),
            )
            .await?;
            completed += written;
        }

        let path = self.model_path(model);
        tracing::info!("Model {} ready at {}", model.name(), path.display());
        Ok(path)
    }
}

/// Length of a file, or 0 when absent or unreadable.
fn file_len(path: &std::path::Path) -> u64 {
    path.metadata().map(|m| m.len()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serde_form_matches_id_for_every_variant() {
        // A divergence here is a config-destroying bug: Settings persists the
        // serde form, docs and the CLI use id(), and a form the other parser
        // rejects fails the whole Settings parse and resets it to defaults.
        for model in SttModel::all() {
            let serialized = serde_json::to_value(model).expect("serialize");
            assert_eq!(
                serialized,
                serde_json::Value::String(model.id().to_string()),
                "{model:?}: serde form and id() must agree"
            );
            let parsed: SttModel =
                serde_json::from_value(serde_json::Value::String(model.id().to_string()))
                    .expect("id() form must deserialize");
            assert_eq!(parsed, *model);
            assert_eq!(SttModel::from_name(model.id()), Some(*model));
        }
    }

    #[test]
    fn legacy_serde_forms_still_deserialize() {
        // Configs written before the Parakeet serde rename carry the plain
        // rename_all form; they must keep loading.
        for (legacy, expected) in [
            ("parakeet-tdt06b-v2", SttModel::ParakeetTdt06bV2),
            ("parakeet-tdt06b-v3", SttModel::ParakeetTdt06bV3),
        ] {
            let parsed: SttModel =
                serde_json::from_value(serde_json::Value::String(legacy.to_string()))
                    .expect("legacy serde form must deserialize");
            assert_eq!(parsed, expected);
            assert_eq!(SttModel::from_name(legacy), Some(expected));
        }
    }

    #[test]
    fn parakeet_v3_round_trips_id_and_short_name() {
        let m = SttModel::ParakeetTdt06bV3;
        assert_eq!(m.id(), "parakeet-tdt-06b-v3");
        assert_eq!(m.short_name(), "parakeet-tdt-0.6b-v3");
        assert_eq!(SttModel::from_name(m.id()), Some(m));
        assert_eq!(SttModel::from_name(m.short_name()), Some(m));
    }

    #[test]
    fn parakeet_v3_uses_parakeet_backend_and_is_listed() {
        assert_eq!(SttModel::ParakeetTdt06bV3.backend(), Backend::Parakeet);
        assert!(SttModel::all().contains(&SttModel::ParakeetTdt06bV3));
    }

    #[test]
    fn parakeet_v3_is_multilingual_v2_stays_english_only() {
        assert!(SttModel::ParakeetTdt06bV3.is_multilingual());
        assert!(!SttModel::ParakeetTdt06bV2.is_multilingual());
    }

    #[test]
    fn only_whisper_turbo_supports_translation_and_forced_language() {
        // Parakeet v3 is multilingual but the engine ignores set_language and
        // set_translate on the Parakeet path; the capability split keeps the
        // settings UI from silently no-opping (it warns instead).
        for model in SttModel::all() {
            let is_turbo = *model == SttModel::WhisperLargeV3Turbo;
            assert_eq!(model.supports_translation(), is_turbo, "{model:?}");
            assert_eq!(model.supports_forced_language(), is_turbo, "{model:?}");
        }
    }

    #[test]
    fn parakeet_v3_lists_expected_files() {
        let files = SttModel::ParakeetTdt06bV3.model_files();
        let remote: Vec<_> = files.iter().map(|f| f.remote_name).collect();
        let local: Vec<_> = files.iter().map(|f| f.local_name).collect();
        assert_eq!(
            remote,
            [
                "encoder-model.int8.onnx",
                "decoder_joint-model.int8.onnx",
                "vocab.txt"
            ]
        );
        assert_eq!(
            local,
            [
                "encoder-model.onnx",
                "decoder_joint-model.onnx",
                "vocab.txt"
            ]
        );
    }

    #[test]
    fn parakeet_v3_pins_a_full_sha256_for_every_file() {
        for f in SttModel::ParakeetTdt06bV3.model_files() {
            assert_eq!(f.sha256.len(), 64, "{}: pin is not 64 chars", f.remote_name);
            assert!(
                f.sha256.chars().all(|c| c.is_ascii_hexdigit()),
                "{}: pin is not hex",
                f.remote_name
            );
        }
    }

    #[test]
    fn parakeet_v3_downloads_from_the_v3_repo() {
        assert!(SttModel::ParakeetTdt06bV3.base_url().contains("v3-onnx"));
        assert!(SttModel::ParakeetTdt06bV2.base_url().contains("v2-onnx"));
    }

    #[test]
    fn parakeet_v3_serde_round_trips() {
        let json = serde_json::to_string(&SttModel::ParakeetTdt06bV3).expect("serialize");
        let back: SttModel = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, SttModel::ParakeetTdt06bV3);
    }
}
