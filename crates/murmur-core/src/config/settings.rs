use crate::output::OutputMode;
use crate::stt::models::SttModel;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Application settings, loaded from TOML config file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    /// Global hotkey for push-to-talk (e.g., "ctrl+shift+space").
    #[serde(default = "default_hotkey")]
    pub hotkey: String,

    /// Which STT model to use.
    #[serde(default = "default_model")]
    pub model: SttModel,

    /// How to output transcribed text.
    #[serde(default)]
    pub output_mode: OutputMode,

    /// VAD speech probability threshold (0.0 - 1.0).
    #[serde(default = "default_vad_threshold")]
    pub vad_threshold: f32,

    /// Preferred audio input device name (None = system default).
    #[serde(default)]
    pub audio_device: Option<String>,

    /// RMS threshold below which audio is considered silence (0.0 - 1.0).
    #[serde(default = "default_silence_rms_threshold")]
    pub silence_rms_threshold: f32,

    /// Seconds of silence after speech before auto-stop triggers.
    #[serde(default = "default_silence_timeout_secs")]
    pub silence_timeout_secs: f32,

    /// Seconds of silence pause that ends a phrase during streaming mode.
    #[serde(default = "default_phrase_pause_secs")]
    pub phrase_pause_secs: f32,

    /// Seconds of total inactivity before a streaming session ends.
    #[serde(default = "default_session_timeout_secs")]
    pub session_timeout_secs: f32,

    /// Developer mode: post-processes transcription for programming terms,
    /// symbols, filler removal, and casing formatters.
    #[serde(default)]
    pub developer_mode: bool,
}

fn default_hotkey() -> String {
    if cfg!(target_os = "macos") {
        "super+shift+space".to_string()
    } else {
        "ctrl+q".to_string()
    }
}

fn default_model() -> SttModel {
    SttModel::WhisperSmallEn
}

fn default_vad_threshold() -> f32 {
    0.5
}

fn default_silence_rms_threshold() -> f32 {
    0.0 // 0.0 = auto-calibrate from ambient noise
}

fn default_silence_timeout_secs() -> f32 {
    2.5
}

fn default_phrase_pause_secs() -> f32 {
    1.8
}

fn default_session_timeout_secs() -> f32 {
    10.0
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            hotkey: default_hotkey(),
            model: default_model(),
            output_mode: OutputMode::default(),
            vad_threshold: default_vad_threshold(),
            audio_device: None,
            silence_rms_threshold: default_silence_rms_threshold(),
            silence_timeout_secs: default_silence_timeout_secs(),
            phrase_pause_secs: default_phrase_pause_secs(),
            session_timeout_secs: default_session_timeout_secs(),
            developer_mode: false,
        }
    }
}

impl Settings {
    /// Get the default config file path (~/.murmur/config.toml).
    pub fn default_path() -> Result<PathBuf> {
        let dir = dirs::config_dir()
            .ok_or_else(|| anyhow::anyhow!("Could not determine config directory"))?
            .join("murmur");
        Ok(dir.join("config.toml"))
    }

    /// Migrate config directory from legacy "voitex" name if it exists.
    ///
    /// If the old `voitex` config directory exists and the new `murmur` directory
    /// does not, renames the directory so existing settings carry over seamlessly.
    pub fn migrate_from_voitex() {
        let Some(config_base) = dirs::config_dir() else {
            return;
        };
        let old_dir = config_base.join("voitex");
        let new_dir = config_base.join("murmur");
        if old_dir.exists() && !new_dir.exists() {
            match std::fs::rename(&old_dir, &new_dir) {
                Ok(()) => tracing::info!(
                    "Migrated config directory from {} to {}",
                    old_dir.display(),
                    new_dir.display()
                ),
                Err(e) => tracing::warn!(
                    "Failed to migrate config directory from {} to {}: {}",
                    old_dir.display(),
                    new_dir.display(),
                    e
                ),
            }
        }
    }

    /// Load settings from a TOML file, falling back to defaults.
    /// On first run (no config file), creates the file with defaults.
    pub fn load(path: &PathBuf) -> Result<Self> {
        if path.exists() {
            let content = std::fs::read_to_string(path)?;
            let settings: Settings = toml::from_str(&content)?;
            settings.validate()?;
            tracing::info!("Loaded config from {}", path.display());
            Ok(settings)
        } else {
            tracing::info!(
                "No config file found, creating defaults at {}",
                path.display()
            );
            let settings = Self::default();
            settings.save(path)?;
            Ok(settings)
        }
    }

    /// Save settings to a TOML file.
    pub fn save(&self, path: &PathBuf) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(self)?;
        std::fs::write(path, content)?;
        tracing::info!("Saved config to {}", path.display());
        Ok(())
    }

    /// Validate settings values.
    pub fn validate(&self) -> Result<()> {
        if !(0.0..=1.0).contains(&self.vad_threshold) {
            anyhow::bail!(
                "vad_threshold must be between 0.0 and 1.0, got {}",
                self.vad_threshold
            );
        }

        if !(0.0..=1.0).contains(&self.silence_rms_threshold) {
            anyhow::bail!(
                "silence_rms_threshold must be between 0.0 and 1.0, got {}",
                self.silence_rms_threshold
            );
        }

        if self.silence_timeout_secs <= 0.0 {
            anyhow::bail!(
                "silence_timeout_secs must be > 0.0, got {}",
                self.silence_timeout_secs
            );
        }

        if self.phrase_pause_secs <= 0.0 {
            anyhow::bail!(
                "phrase_pause_secs must be > 0.0, got {}",
                self.phrase_pause_secs
            );
        }

        if self.session_timeout_secs < 0.0 {
            anyhow::bail!(
                "session_timeout_secs must be >= 0.0 (0 = disabled), got {}",
                self.session_timeout_secs
            );
        }

        let hotkey = self.hotkey.trim();
        if hotkey.is_empty() {
            anyhow::bail!("hotkey cannot be empty");
        }
        if hotkey.contains('+') {
            for part in hotkey.split('+') {
                if part.trim().is_empty() {
                    anyhow::bail!("hotkey has empty part in '{}'", hotkey);
                }
            }
        }

        Ok(())
    }
}
