use crate::output::OutputMode;
use crate::stt::models::WhisperModel;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Application settings, loaded from TOML config file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    /// Global hotkey for push-to-talk (e.g., "ctrl+shift+space").
    #[serde(default = "default_hotkey")]
    pub hotkey: String,

    /// Which Whisper model to use.
    #[serde(default = "default_model")]
    pub model: WhisperModel,

    /// How to output transcribed text.
    #[serde(default = "default_output_mode")]
    pub output_mode: OutputModeSetting,

    /// VAD speech probability threshold (0.0 - 1.0).
    #[serde(default = "default_vad_threshold")]
    pub vad_threshold: f32,

    /// Preferred audio input device name (None = system default).
    #[serde(default)]
    pub audio_device: Option<String>,
}

/// Serializable output mode (serde-friendly version of OutputMode).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputModeSetting {
    Keyboard,
    Clipboard,
    Stdout,
}

impl From<OutputModeSetting> for OutputMode {
    fn from(s: OutputModeSetting) -> Self {
        match s {
            OutputModeSetting::Keyboard => OutputMode::Keyboard,
            OutputModeSetting::Clipboard => OutputMode::Clipboard,
            OutputModeSetting::Stdout => OutputMode::Stdout,
        }
    }
}

fn default_hotkey() -> String {
    if cfg!(target_os = "macos") {
        "super+shift+space".to_string()
    } else {
        "ctrl+shift+space".to_string()
    }
}

fn default_model() -> WhisperModel {
    WhisperModel::SmallEn
}

fn default_output_mode() -> OutputModeSetting {
    OutputModeSetting::Keyboard
}

fn default_vad_threshold() -> f32 {
    0.5
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            hotkey: default_hotkey(),
            model: default_model(),
            output_mode: default_output_mode(),
            vad_threshold: default_vad_threshold(),
            audio_device: None,
        }
    }
}

impl Settings {
    /// Get the default config file path (~/.voitex/config.toml).
    pub fn default_path() -> Result<PathBuf> {
        let dir = dirs::config_dir()
            .ok_or_else(|| anyhow::anyhow!("Could not determine config directory"))?
            .join("voitex");
        Ok(dir.join("config.toml"))
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
            tracing::info!("No config file found, creating defaults at {}", path.display());
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
        if self.hotkey.trim().is_empty() {
            anyhow::bail!("hotkey cannot be empty");
        }
        Ok(())
    }
}
