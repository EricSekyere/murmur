use crate::output::OutputMode;
use crate::stt::models::SttModel;
use crate::voice_commands::Snippet;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Per-application override applied for the duration of a session when the
/// foreground app matches. Unset fields fall back to the global settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppProfile {
    /// Case-insensitive substring of the target app's process name, e.g.
    /// "code", "slack", "windowsterminal".
    pub app: String,
    /// Output mode to use in this app (None = use the global setting).
    #[serde(default)]
    pub output_mode: Option<OutputMode>,
    /// Developer-mode override for this app (None = use the global toggle).
    #[serde(default)]
    pub developer_mode: Option<bool>,
}

/// Transcription filtering profile.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptionProfile {
    /// More permissive thresholds for quieter/shorter phrases.
    #[default]
    Relaxed,
    /// Stricter thresholds and stronger hallucination filtering.
    Strict,
}

/// Upper bounds for the codebase indexer, enforced on load.
const MAX_INDEX_SYMBOLS: usize = 128;
const MAX_INDEX_EXTENSIONS: usize = 32;
const MAX_INDEX_ROOTS: usize = 16;

/// Codebase-derived vocabulary: scan `project_root` for distinctive identifiers
/// and inject them into the STT vocabulary so project symbols transcribe
/// correctly. Disabled by default; only helps Whisper (Parakeet has no biasing).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexerSettings {
    /// Whether to index the projects and inject their symbols.
    #[serde(default)]
    pub enabled: bool,
    /// Project roots to scan. Indexing only runs when at least one is set;
    /// symbols from all roots share one ranked budget.
    #[serde(default)]
    pub project_roots: Vec<PathBuf>,
    /// Maximum symbols to inject (clamped to 1..=128 on load).
    #[serde(default = "default_index_max_symbols")]
    pub max_symbols: usize,
    /// Source extensions to scan; empty means the built-in defaults.
    #[serde(default)]
    pub extensions: Vec<String>,
}

impl Default for IndexerSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            project_roots: Vec::new(),
            max_symbols: default_index_max_symbols(),
            extensions: Vec::new(),
        }
    }
}

fn default_index_max_symbols() -> usize {
    64
}

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

    /// Transcription filtering profile (strict or relaxed).
    #[serde(default)]
    pub transcription_profile: TranscriptionProfile,

    /// Stop recording on any mouse click (default: false).
    /// When disabled, recording only stops via hotkey or mic button.
    #[serde(default)]
    pub click_to_stop: bool,

    /// Show the floating widget window (default: true).
    #[serde(default = "default_true")]
    pub show_widget: bool,

    /// Milliseconds to wait before sending keystrokes after hotkey release.
    /// Gives the target window time to regain focus. Set to 0 to disable.
    #[serde(default = "default_pre_output_delay_ms")]
    pub pre_output_delay_ms: u64,

    /// Key that toggles recording when double-tapped quickly.
    /// "ctrl" (both sides; Cmd on macOS), "rctrl"/"lctrl" for one side only,
    /// "rcmd" on macOS, or a single letter like "v". Taps only count when no
    /// other key is involved, so shortcuts like Ctrl+V are ignored.
    /// "rctrl" is the recommended value: it never types a character and is
    /// virtually never part of shortcuts.
    #[serde(default = "default_double_tap_key")]
    pub double_tap_key: String,

    /// How the double-tap/hold key activates recording:
    /// "toggle" (tap twice to start, twice to stop) or
    /// "hold" (push-to-talk: record while the key is held).
    #[serde(default = "default_activation_mode")]
    pub activation_mode: String,

    /// Words whisper tends to get wrong (names, jargon). Injected into the
    /// decoder prompt as a glossary so they transcribe correctly.
    #[serde(default)]
    pub custom_vocabulary: Vec<String>,

    /// Play a short chime when recording starts and stops.
    #[serde(default = "default_true")]
    pub sound_feedback: bool,

    /// Show interim transcription as you speak, before the phrase is final.
    /// Adds a little GPU/CPU work per session; disable for the lowest latency.
    #[serde(default = "default_true")]
    pub live_preview: bool,

    /// User-defined text snippets: say the trigger phrase, get the expansion
    /// typed. Matched only when the trigger is the entire phrase.
    #[serde(default)]
    pub snippets: Vec<Snippet>,

    /// Spoken language: "auto" to detect, or a code like "en"/"es"/"fr".
    /// Only honored by multilingual models (the `.en` models are English-only).
    #[serde(default = "default_language")]
    pub language: String,

    /// Translate recognized speech to English (multilingual models only).
    #[serde(default)]
    pub translate_to_english: bool,

    /// Per-app overrides applied when the foreground app matches at session
    /// start. The first matching profile wins.
    #[serde(default)]
    pub app_profiles: Vec<AppProfile>,

    /// Where the live preview caption appears: "pill" (under the floating
    /// pill) or "window" (near the bottom of the active window).
    #[serde(default = "default_caption_position")]
    pub caption_position: String,

    /// Persist delivered phrases to the searchable history log (off = store nothing).
    #[serde(default = "default_true")]
    pub save_history: bool,

    /// Clean up ordinary dictation: strip "um"/"uh" disfluencies and format
    /// spoken number lists. Off = deliver verbatim. Developer mode always runs
    /// its own fuller post-processing regardless of this flag.
    #[serde(default = "default_true")]
    pub clean_speech: bool,

    /// Use the OS voice-capture path (echo cancellation + noise suppression) so
    /// the mic doesn't pick up audio from your own speakers. Windows only for
    /// now; falls back to the raw mic elsewhere or if it can't be opened.
    #[serde(default = "default_true")]
    pub echo_cancellation: bool,

    /// Codebase-derived vocabulary settings (off by default).
    #[serde(default)]
    pub indexer: IndexerSettings,

    /// Last app version whose "What's New" highlights the user dismissed, so the
    /// panel only auto-opens once per update.
    #[serde(default)]
    pub whats_new_seen_version: Option<String>,
}

impl AppProfile {
    /// Whether this profile matches the process name (case-insensitive). Matches
    /// the executable stem as a whole word (split on separators and camelCase),
    /// so "code" matches `Code.exe`/`code-insiders.exe` but not `unicode.exe`.
    /// A multi-token pattern falls back to substring. Blank never matches.
    pub fn matches(&self, process_name: &str) -> bool {
        let pat = self.app.trim().to_lowercase();
        if pat.is_empty() {
            return false;
        }
        let stem = process_stem(process_name);
        let stem_lower = stem.to_lowercase();
        if stem_lower == pat {
            return true;
        }
        // Explicit multi-token patterns (e.g. "visual studio") keep substring
        // behaviour; single tokens must match a whole word.
        if pat.contains(|c: char| !c.is_alphanumeric()) {
            return stem_lower.contains(&pat);
        }
        split_words(stem).iter().any(|w| w == &pat)
    }
}

/// The executable stem: the file name with any directory path and a trailing
/// `.exe`/`.app` extension removed. Case is preserved for camelCase splitting.
fn process_stem(process_name: &str) -> &str {
    let file = process_name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(process_name);
    let lower = file.to_ascii_lowercase();
    if lower.ends_with(".exe") || lower.ends_with(".app") {
        &file[..file.len() - 4]
    } else {
        file
    }
}

/// Split a name into lowercase words on separators and camelCase boundaries:
/// "WindowsTerminal" -> ["windows", "terminal"], "code-insiders" -> ["code",
/// "insiders"], "unicode" -> ["unicode"].
fn split_words(stem: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut prev_lower_or_digit = false;
    for ch in stem.chars() {
        if !ch.is_alphanumeric() {
            if !cur.is_empty() {
                words.push(std::mem::take(&mut cur));
            }
            prev_lower_or_digit = false;
            continue;
        }
        if ch.is_uppercase() && prev_lower_or_digit && !cur.is_empty() {
            words.push(std::mem::take(&mut cur));
        }
        cur.extend(ch.to_lowercase());
        prev_lower_or_digit = ch.is_lowercase() || ch.is_numeric();
    }
    if !cur.is_empty() {
        words.push(cur);
    }
    words
}

// Shared validation bounds: one source of truth for `validate()` and the UI.
pub const VAD_THRESHOLD_MIN: f32 = 0.05;
pub const VAD_THRESHOLD_MAX: f32 = 0.95;
pub const PHRASE_PAUSE_MIN_SECS: f32 = 0.3;
pub const PHRASE_PAUSE_MAX_SECS: f32 = 10.0;
pub const SESSION_TIMEOUT_MAX_SECS: f32 = 300.0;

// Collection caps: bound user text reaching the decoder prompt / keystrokes.
pub const MAX_VOCAB_ENTRIES: usize = 100;
pub const MAX_SNIPPETS: usize = 100;
pub const MAX_APP_PROFILES: usize = 50;
const MAX_VOCAB_ENTRY_CHARS: usize = 100;
const MAX_SNIPPET_TRIGGER_CHARS: usize = 100;
const MAX_SNIPPET_EXPANSION_CHARS: usize = 2_000;
const MAX_APP_PATTERN_CHARS: usize = 100;

/// Truncate a string to at most `max` characters, on a UTF-8 boundary.
fn truncate_chars(s: &mut String, max: usize) {
    if s.chars().count() > max {
        *s = s.chars().take(max).collect();
    }
}

fn default_hotkey() -> String {
    // Avoid Ctrl+single-letter (collides with app shortcuts like Ctrl+Q and is
    // swallowed globally). The double-tap key is the collision-free primary.
    if cfg!(target_os = "macos") {
        "super+shift+space".to_string()
    } else {
        "ctrl+shift+space".to_string()
    }
}

fn default_model() -> SttModel {
    SttModel::WhisperSmallEn
}

fn default_vad_threshold() -> f32 {
    // Sensitive enough to catch normal speech without the user raising their
    // voice. Higher values (0.5+) reject quieter speech and force the user to
    // speak unnaturally loudly. Hallucinations on breaths/noise are handled
    // after transcription by the confidence and repeated-phrase filters.
    0.3
}

fn default_silence_rms_threshold() -> f32 {
    0.0 // 0.0 = auto-calibrate from ambient noise
}

fn default_silence_timeout_secs() -> f32 {
    2.5
}

fn default_phrase_pause_secs() -> f32 {
    // Short enough that text lands soon after you stop talking
    // (macOS-dictation feel), long enough not to split mid-sentence
    // breaths into separate phrases.
    0.6
}

fn default_session_timeout_secs() -> f32 {
    30.0
}

fn default_true() -> bool {
    true
}

fn default_activation_mode() -> String {
    "toggle".to_string()
}

fn default_language() -> String {
    "en".to_string()
}

fn default_caption_position() -> String {
    "pill".to_string()
}

fn default_pre_output_delay_ms() -> u64 {
    80
}

fn default_double_tap_key() -> String {
    // Right Ctrl never types a character and is virtually unused in
    // shortcuts, so double-tapping it cannot collide with typing or
    // copy/paste. macOS has no right-Ctrl convention; use Cmd there.
    if cfg!(windows) {
        "rctrl".to_string()
    } else {
        "ctrl".to_string()
    }
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
            transcription_profile: TranscriptionProfile::default(),
            click_to_stop: false,
            show_widget: true,
            pre_output_delay_ms: default_pre_output_delay_ms(),
            double_tap_key: default_double_tap_key(),
            activation_mode: default_activation_mode(),
            custom_vocabulary: Vec::new(),
            sound_feedback: true,
            live_preview: true,
            snippets: Vec::new(),
            language: default_language(),
            translate_to_english: false,
            app_profiles: Vec::new(),
            caption_position: default_caption_position(),
            save_history: true,
            clean_speech: true,
            echo_cancellation: true,
            indexer: IndexerSettings::default(),
            whats_new_seen_version: None,
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
        if !old_dir.exists() || new_dir.exists() {
            return;
        }

        if std::fs::rename(&old_dir, &new_dir).is_ok() {
            tracing::info!(
                "Migrated config directory from {} to {}",
                old_dir.display(),
                new_dir.display()
            );
            return;
        }

        // rename fails across volumes (EXDEV) — e.g. a legacy config on a
        // different drive than the new config dir. Fall back to a recursive
        // copy so settings still carry over; remove a partial copy on failure
        // so a later run can retry from the intact old directory.
        match Self::copy_dir_recursive(&old_dir, &new_dir) {
            Ok(()) => {
                let _ = std::fs::remove_dir_all(&old_dir);
                tracing::info!(
                    "Migrated config directory (copied) from {} to {}",
                    old_dir.display(),
                    new_dir.display()
                );
            }
            Err(e) => {
                let _ = std::fs::remove_dir_all(&new_dir);
                tracing::warn!(
                    "Failed to migrate config directory from {} to {}: {}",
                    old_dir.display(),
                    new_dir.display(),
                    e
                );
            }
        }
    }

    fn copy_dir_recursive(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
        std::fs::create_dir_all(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            let dst = to.join(entry.file_name());
            if entry.file_type()?.is_dir() {
                Self::copy_dir_recursive(&entry.path(), &dst)?;
            } else {
                std::fs::copy(entry.path(), &dst)?;
            }
        }
        Ok(())
    }

    /// Load settings from a TOML file, falling back to defaults.
    ///
    /// On first run (no config file), creates the file with defaults. If the
    /// file exists but is unreadable or invalid, it is backed up and defaults
    /// are used so a corrupt config never blocks startup.
    pub fn load(path: &PathBuf) -> Result<Self> {
        if !path.exists() {
            tracing::info!(
                "No config file found, creating defaults at {}",
                path.display()
            );
            let settings = Self::default();
            settings.save(path)?;
            return Ok(settings);
        }

        match Self::read_and_validate(path) {
            Ok(settings) => {
                tracing::info!("Loaded config from {}", path.display());
                Ok(settings)
            }
            Err(e) => {
                let backup = path.with_extension("toml.bak");
                tracing::warn!(
                    "Config at {} is unreadable or invalid ({}); backing it up to {} and using defaults",
                    path.display(),
                    e,
                    backup.display()
                );
                let _ = std::fs::rename(path, &backup);
                let settings = Self::default();
                if let Err(save_err) = settings.save(path) {
                    tracing::warn!("Failed to write fresh defaults: {}", save_err);
                }
                Ok(settings)
            }
        }
    }

    fn read_and_validate(path: &PathBuf) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let mut settings: Settings = toml::from_str(&content)?;
        // Truncate oversized collections before validating (don't reject the file).
        settings.clamp_collections();
        settings.validate()?;
        Ok(settings)
    }

    /// Save settings to a TOML file (atomic: write to tempfile, then rename).
    pub fn save(&self, path: &PathBuf) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(self)?;

        // Atomic write: write to a sibling tempfile first, then rename.
        // This prevents a crash mid-write from corrupting the config file.
        let tmp_path = path.with_extension("toml.tmp");
        std::fs::write(&tmp_path, &content)?;
        std::fs::rename(&tmp_path, path)?;

        tracing::info!("Saved config to {}", path.display());
        Ok(())
    }

    /// Clamp collections to their count and per-element length caps, so an
    /// over-large hand-edited config is truncated rather than rejected.
    pub fn clamp_collections(&mut self) {
        self.custom_vocabulary.truncate(MAX_VOCAB_ENTRIES);
        for w in &mut self.custom_vocabulary {
            truncate_chars(w, MAX_VOCAB_ENTRY_CHARS);
        }
        self.snippets.truncate(MAX_SNIPPETS);
        for s in &mut self.snippets {
            truncate_chars(&mut s.trigger, MAX_SNIPPET_TRIGGER_CHARS);
            truncate_chars(&mut s.expansion, MAX_SNIPPET_EXPANSION_CHARS);
        }
        self.app_profiles.truncate(MAX_APP_PROFILES);
        for p in &mut self.app_profiles {
            truncate_chars(&mut p.app, MAX_APP_PATTERN_CHARS);
        }
        self.indexer.max_symbols = self.indexer.max_symbols.clamp(1, MAX_INDEX_SYMBOLS);
        self.indexer.extensions.truncate(MAX_INDEX_EXTENSIONS);
        self.indexer.project_roots.truncate(MAX_INDEX_ROOTS);
    }

    /// Validate settings values.
    pub fn validate(&self) -> Result<()> {
        if !(VAD_THRESHOLD_MIN..=VAD_THRESHOLD_MAX).contains(&self.vad_threshold) {
            anyhow::bail!(
                "vad_threshold must be between {} and {}, got {}",
                VAD_THRESHOLD_MIN,
                VAD_THRESHOLD_MAX,
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

        if !(PHRASE_PAUSE_MIN_SECS..=PHRASE_PAUSE_MAX_SECS).contains(&self.phrase_pause_secs) {
            anyhow::bail!(
                "phrase_pause_secs must be between {} and {}, got {}",
                PHRASE_PAUSE_MIN_SECS,
                PHRASE_PAUSE_MAX_SECS,
                self.phrase_pause_secs
            );
        }

        if !(0.0..=SESSION_TIMEOUT_MAX_SECS).contains(&self.session_timeout_secs) {
            anyhow::bail!(
                "session_timeout_secs must be between 0.0 (disabled) and {}, got {}",
                SESSION_TIMEOUT_MAX_SECS,
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

        if self.activation_mode != "toggle" && self.activation_mode != "hold" {
            anyhow::bail!(
                "activation_mode must be 'toggle' or 'hold', got '{}'",
                self.activation_mode
            );
        }

        if self.language.trim().is_empty() {
            anyhow::bail!("language cannot be empty (use 'auto' or a code like 'en')");
        }

        if self.snippets.len() > MAX_SNIPPETS {
            anyhow::bail!(
                "too many snippets ({}, max {})",
                self.snippets.len(),
                MAX_SNIPPETS
            );
        }
        if self.app_profiles.len() > MAX_APP_PROFILES {
            anyhow::bail!(
                "too many app profiles ({}, max {})",
                self.app_profiles.len(),
                MAX_APP_PROFILES
            );
        }

        if self.caption_position != "pill" && self.caption_position != "window" {
            anyhow::bail!(
                "caption_position must be 'pill' or 'window', got '{}'",
                self.caption_position
            );
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(app: &str) -> AppProfile {
        AppProfile {
            app: app.to_string(),
            output_mode: None,
            developer_mode: Some(true),
        }
    }

    #[test]
    fn app_profile_matches_whole_word_case_insensitive() {
        assert!(profile("code").matches("Code.exe"));
        assert!(profile("code").matches("code-insiders.exe"));
        assert!(profile("Terminal").matches("WindowsTerminal.exe"));
        assert!(profile("chrome").matches("chrome.exe"));
        assert!(!profile("slack").matches("Code.exe"));
    }

    #[test]
    fn app_profile_does_not_match_mid_word_substring() {
        // "code" must not match the unrelated "unicode".
        assert!(!profile("code").matches("unicode.exe"));
        assert!(!profile("go").matches("google.exe"));
    }

    #[test]
    fn app_profile_multi_token_pattern_uses_substring() {
        assert!(profile("visual studio").matches("Visual Studio Code.exe"));
    }

    #[test]
    fn app_profile_blank_pattern_never_matches() {
        assert!(!profile("  ").matches("anything.exe"));
    }
}
