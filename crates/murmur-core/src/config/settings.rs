use crate::cloud::CloudConfig;
use crate::llm::RewriteMode;
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
    /// LLM rewrite mode for text delivered into this app (None = fall back to
    /// the global `default_rewrite_mode`).
    #[serde(default)]
    pub rewrite_mode: Option<RewriteMode>,
    /// Custom rewrite instruction used instead of the built-in mode text when
    /// rewriting a selection in this app (e.g. "Rewrite as a Conventional
    /// Commit message"). Trimmed and capped on load; None = built-in text.
    #[serde(default)]
    pub rewrite_prompt: Option<String>,
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

/// A spoken-form → path-segment alias for spoken file/directory navigation
/// ("source" → `src`). Defined here rather than in the indexer module so
/// configs still load with the `indexer` feature off; the built-in defaults
/// are compiled into `indexer::apply_aliases`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathAlias {
    /// The spoken phrase, matched case-insensitively on whole words.
    #[serde(default)]
    pub spoken: String,
    /// The path segment that replaces it (e.g. `src`, `package.json`).
    #[serde(default)]
    pub path: String,
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

    /// Spoken placeholders replaced inline by the current clipboard text
    /// (text substitution, never a paste keystroke). Whole-word,
    /// case-insensitive match anywhere in a phrase; empty disables the feature.
    #[serde(default = "default_clipboard_placeholders")]
    pub clipboard_placeholders: Vec<String>,

    /// Spoken language: "auto" to detect, or a code like "en"/"es"/"fr".
    /// Only honored by multilingual models (the `.en` models are English-only).
    #[serde(default = "default_language")]
    pub language: String,

    /// Translate recognized speech to English (multilingual models only).
    #[serde(default)]
    pub translate_to_english: bool,

    /// Show each delivered phrase's English translation in the live caption.
    /// Only takes effect while translate_to_english runs on a multilingual
    /// model; fully on-device (reuses the whisper translate output).
    #[serde(default)]
    pub show_translated_caption: bool,

    /// Per-app overrides applied when the foreground app matches at session
    /// start. The first matching profile wins.
    #[serde(default)]
    pub app_profiles: Vec<AppProfile>,

    /// LLM rewrite mode used when no app profile overrides it (None = deliver
    /// text without an LLM rewrite). Only takes effect when the LLM runtime is
    /// built in and its model is available.
    #[serde(default)]
    pub default_rewrite_mode: Option<RewriteMode>,

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

    /// Repair premature terminal punctuation at phrase junctions: when a
    /// pause splits a sentence and the next phrase continues it, the stale
    /// mark is backspaced and the phrases joined ("...store. And bought" ->
    /// "...store and bought"). On by default; the toggle exists to opt out.
    #[serde(default = "default_true")]
    pub smart_punctuation: bool,

    /// Allow a connected coding agent (via the MCP `request_dictation` tool)
    /// to start voice capture so the user can answer a question by speaking.
    /// Off = the MCP server stays strictly read-only.
    #[serde(default = "default_true")]
    pub mcp_dictation_enabled: bool,

    /// Expose a localhost-only WebSocket API that streams live dictation
    /// events to editor plugins (VS Code, Neovim). Token-authenticated.
    /// Off by default: it opens a network listener, even if loopback-only.
    #[serde(default)]
    pub local_api_enabled: bool,

    /// Add local context (the target app's name and the current clipboard
    /// text) to the selection-rewrite prompt. Strictly on-device: the context
    /// only ever enters the local model's prompt and is never logged or
    /// transmitted. Off by default because clipboard text is sensitive even
    /// when it stays local.
    #[serde(default)]
    pub context_injection_enabled: bool,

    /// Use the OS voice-capture path (echo cancellation + noise suppression) so
    /// the mic doesn't pick up audio from your own speakers. Windows only for
    /// now; falls back to the raw mic elsewhere or if it can't be opened.
    #[serde(default = "default_true")]
    pub echo_cancellation: bool,

    /// Keep the microphone stream open between dictations so the first word
    /// is never clipped by a cold device open. While idle, audio is discarded
    /// immediately — nothing is buffered or written anywhere until dictation
    /// starts. Off by default: the OS mic-in-use indicator stays lit while
    /// the stream is warm, which users must opt into knowingly.
    #[serde(default)]
    pub mic_warm_start: bool,

    /// Codebase-derived vocabulary settings (off by default).
    #[serde(default)]
    pub indexer: IndexerSettings,

    /// Spoken path aliases for command mode's "open the … file" / "go to the
    /// … folder": each maps a spoken phrase to a path segment before the query
    /// is resolved against the project index. Built-ins (source → src,
    /// package json → package.json, …) are always active; an entry with the
    /// same spoken form overrides its builtin.
    #[serde(default)]
    pub path_aliases: Vec<PathAlias>,

    /// Opt-in BYO-key cloud rewrite backend. None or a
    /// disabled table means fully local operation, the default. The API key is
    /// never stored here: it is read from the MURMUR_CLOUD_API_KEY environment
    /// variable at call time (a platform keyring later). While a cloud rewrite
    /// is active the UI must show a visible "speech leaving device" indicator;
    /// that indicator is the app layer's responsibility, not core's.
    #[serde(default)]
    pub cloud: Option<CloudConfig>,

    /// Last app version whose "What's New" highlights the user dismissed, so the
    /// panel only auto-opens once per update.
    #[serde(default)]
    pub whats_new_seen_version: Option<String>,

    /// Daily word target shown as progress on the Analytics dashboard
    /// (0 = disabled). Clamped to [`MAX_DAILY_WORD_GOAL`] on load.
    #[serde(default)]
    pub daily_word_goal: usize,

    /// Unload the STT (and rewrite) model after this many seconds without
    /// activity, freeing its RAM; it reloads automatically on the next use.
    /// 0 = keep the model loaded forever (the default). Non-zero values must
    /// fall in [`MODEL_IDLE_UNLOAD_MIN_SECS`]..=[`MODEL_IDLE_UNLOAD_MAX_SECS`].
    #[serde(default)]
    pub model_idle_unload_secs: u64,
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
pub const MAX_DAILY_WORD_GOAL: usize = 100_000;
pub const MODEL_IDLE_UNLOAD_MIN_SECS: u64 = 60;
pub const MODEL_IDLE_UNLOAD_MAX_SECS: u64 = 86_400;

// Collection caps: bound user text reaching the decoder prompt / keystrokes.
pub const MAX_VOCAB_ENTRIES: usize = 100;
pub const MAX_SNIPPETS: usize = 100;
pub const MAX_APP_PROFILES: usize = 50;
pub const MAX_CLIPBOARD_PLACEHOLDERS: usize = 16;
pub const MAX_PATH_ALIASES: usize = 100;
const MAX_VOCAB_ENTRY_CHARS: usize = 100;
const MAX_SNIPPET_TRIGGER_CHARS: usize = 100;
const MAX_SNIPPET_EXPANSION_CHARS: usize = 2_000;
const MAX_APP_PATTERN_CHARS: usize = 100;
const MAX_REWRITE_PROMPT_CHARS: usize = 500;
const MAX_CLIPBOARD_PLACEHOLDER_CHARS: usize = 100;
const MAX_PATH_ALIAS_SPOKEN_CHARS: usize = 100;
const MAX_PATH_ALIAS_PATH_CHARS: usize = 260;

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
    // Parakeet is the fast, accurate CPU default: ~0.1s/phrase with native
    // punctuation. Whisper stays available, but medium/large are only practical
    // with a GPU and small.en is slower than Parakeet on CPU.
    SttModel::ParakeetTdt06bV2
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
    60.0
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

fn default_clipboard_placeholders() -> Vec<String> {
    vec!["insert clipboard".into(), "paste clipboard".into()]
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
            clipboard_placeholders: default_clipboard_placeholders(),
            language: default_language(),
            translate_to_english: false,
            show_translated_caption: false,
            app_profiles: Vec::new(),
            default_rewrite_mode: None,
            caption_position: default_caption_position(),
            save_history: true,
            clean_speech: true,
            smart_punctuation: true,
            mcp_dictation_enabled: true,
            local_api_enabled: false,
            context_injection_enabled: false,
            echo_cancellation: true,
            mic_warm_start: false,
            indexer: IndexerSettings::default(),
            path_aliases: Vec::new(),
            cloud: None,
            whats_new_seen_version: None,
            daily_word_goal: 0,
            model_idle_unload_secs: 0,
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

    /// Load settings without touching the disk: a missing or invalid file
    /// yields defaults and is never created, backed up, or rewritten.
    ///
    /// For read-only consumers like the MCP server, which share the config
    /// file with the running app; [`Self::load`]'s recovery writes would race
    /// the app's own saves.
    pub fn load_readonly(path: &PathBuf) -> Self {
        if !path.exists() {
            return Self::default();
        }
        Self::read_and_validate(path).unwrap_or_else(|e| {
            tracing::warn!(
                "Config at {} is unreadable or invalid ({}); using defaults without modifying it",
                path.display(),
                e
            );
            Self::default()
        })
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
    pub fn save(&self, path: &std::path::Path) -> Result<()> {
        let content = toml::to_string_pretty(self)?;
        crate::fsutil::atomic_write(path, content.as_bytes())?;
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
            // Custom rewrite prompt: drop blank ones, trim and cap the rest.
            p.rewrite_prompt = p.rewrite_prompt.take().and_then(|prompt| {
                let mut prompt = prompt.trim().to_string();
                truncate_chars(&mut prompt, MAX_REWRITE_PROMPT_CHARS);
                (!prompt.is_empty()).then_some(prompt)
            });
        }
        // Placeholders match case-insensitively, so store them normalized;
        // drop empties and duplicates the matcher would never use.
        let mut placeholders: Vec<String> = Vec::new();
        for entry in std::mem::take(&mut self.clipboard_placeholders) {
            let mut normalized = entry.trim().to_lowercase();
            truncate_chars(&mut normalized, MAX_CLIPBOARD_PLACEHOLDER_CHARS);
            if !normalized.is_empty() && !placeholders.contains(&normalized) {
                placeholders.push(normalized);
            }
        }
        placeholders.truncate(MAX_CLIPBOARD_PLACEHOLDERS);
        self.clipboard_placeholders = placeholders;
        self.indexer.max_symbols = self.indexer.max_symbols.clamp(1, MAX_INDEX_SYMBOLS);
        self.indexer.extensions.truncate(MAX_INDEX_EXTENSIONS);
        self.indexer.project_roots.truncate(MAX_INDEX_ROOTS);
        // Aliases match case-insensitively, so store the spoken form
        // normalized; an entry missing either side can never fire, drop it.
        let mut aliases: Vec<PathAlias> = Vec::new();
        for mut alias in std::mem::take(&mut self.path_aliases) {
            alias.spoken = alias.spoken.trim().to_lowercase();
            alias.path = alias.path.trim().to_string();
            truncate_chars(&mut alias.spoken, MAX_PATH_ALIAS_SPOKEN_CHARS);
            truncate_chars(&mut alias.path, MAX_PATH_ALIAS_PATH_CHARS);
            if !alias.spoken.is_empty() && !alias.path.is_empty() {
                aliases.push(alias);
            }
        }
        aliases.truncate(MAX_PATH_ALIASES);
        self.path_aliases = aliases;
        self.daily_word_goal = self.daily_word_goal.min(MAX_DAILY_WORD_GOAL);
    }

    /// Effective LLM rewrite mode for the foreground process: the first
    /// matching profile's mode if set, otherwise the global default. None
    /// means deliver text without an LLM rewrite.
    pub fn rewrite_mode_for(&self, process_name: &str) -> Option<RewriteMode> {
        self.app_profiles
            .iter()
            .find(|p| p.matches(process_name))
            .and_then(|p| p.rewrite_mode)
            .or(self.default_rewrite_mode)
    }

    /// Custom rewrite instruction for the foreground process: the first
    /// matching profile that carries a prompt wins. None = use the built-in
    /// mode text.
    pub fn rewrite_prompt_for(&self, process_name: &str) -> Option<&str> {
        self.app_profiles
            .iter()
            .filter(|p| p.matches(process_name))
            .find_map(|p| p.rewrite_prompt.as_deref())
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

        if self.model_idle_unload_secs != 0
            && !(MODEL_IDLE_UNLOAD_MIN_SECS..=MODEL_IDLE_UNLOAD_MAX_SECS)
                .contains(&self.model_idle_unload_secs)
        {
            anyhow::bail!(
                "model_idle_unload_secs must be 0 (never) or between {} and {}, got {}",
                MODEL_IDLE_UNLOAD_MIN_SECS,
                MODEL_IDLE_UNLOAD_MAX_SECS,
                self.model_idle_unload_secs
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
            rewrite_mode: None,
            rewrite_prompt: None,
        }
    }

    fn rewrite_profile(app: &str, mode: Option<RewriteMode>) -> AppProfile {
        AppProfile {
            app: app.to_string(),
            output_mode: None,
            developer_mode: None,
            rewrite_mode: mode,
            rewrite_prompt: None,
        }
    }

    fn prompt_profile(app: &str, prompt: &str) -> AppProfile {
        AppProfile {
            rewrite_prompt: Some(prompt.to_string()),
            ..rewrite_profile(app, None)
        }
    }

    #[test]
    fn load_readonly_never_touches_a_corrupt_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "not [valid toml").expect("write");

        let settings = Settings::load_readonly(&path);
        assert_eq!(settings.save_history, Settings::default().save_history);
        // Unlike load(), nothing on disk may change: no .bak, no rewrite.
        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            "not [valid toml"
        );
        assert!(!path.with_extension("toml.bak").exists());
    }

    #[test]
    fn load_readonly_does_not_create_a_missing_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        let _ = Settings::load_readonly(&path);
        assert!(!path.exists());
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

    #[test]
    fn rewrite_mode_round_trips_through_toml() {
        let settings = Settings {
            default_rewrite_mode: Some(RewriteMode::CleanUp),
            app_profiles: vec![rewrite_profile("slack", Some(RewriteMode::Casual))],
            ..Settings::default()
        };
        let toml_str = toml::to_string_pretty(&settings).unwrap();
        let reloaded: Settings = toml::from_str(&toml_str).unwrap();
        assert_eq!(reloaded.default_rewrite_mode, Some(RewriteMode::CleanUp));
        assert_eq!(
            reloaded.app_profiles[0].rewrite_mode,
            Some(RewriteMode::Casual)
        );
    }

    #[test]
    fn old_config_without_rewrite_fields_still_loads() {
        // A pre-rewrite config: no default_rewrite_mode, no per-profile
        // rewrite_mode. Must load with both defaulting to None.
        let old = r#"
            hotkey = "ctrl+shift+space"

            [[app_profiles]]
            app = "code"
            developer_mode = true
        "#;
        let settings: Settings = toml::from_str(old).unwrap();
        assert_eq!(settings.default_rewrite_mode, None);
        assert_eq!(settings.app_profiles[0].rewrite_mode, None);
        assert_eq!(settings.app_profiles[0].developer_mode, Some(true));
        // Later additions must also default off/empty on an old config.
        assert_eq!(settings.app_profiles[0].rewrite_prompt, None);
        assert!(!settings.context_injection_enabled);
    }

    #[test]
    fn rewrite_prompt_and_context_injection_round_trip_through_toml() {
        let settings = Settings {
            context_injection_enabled: true,
            app_profiles: vec![prompt_profile("code", "Rewrite as a commit message.")],
            ..Settings::default()
        };
        let text = toml::to_string_pretty(&settings).unwrap();
        let reloaded: Settings = toml::from_str(&text).unwrap();
        assert!(reloaded.context_injection_enabled);
        assert_eq!(
            reloaded.app_profiles[0].rewrite_prompt.as_deref(),
            Some("Rewrite as a commit message.")
        );
        // Clipboard entering a prompt is opt-in: a fresh config stays off.
        assert!(!Settings::default().context_injection_enabled);
    }

    #[test]
    fn clamp_trims_caps_and_drops_rewrite_prompts() {
        let mut settings = Settings {
            app_profiles: vec![
                prompt_profile("code", "  Rewrite as a commit message.  "),
                prompt_profile("slack", "   "),
                prompt_profile("term", &"é".repeat(600)),
            ],
            ..Settings::default()
        };
        settings.clamp_collections();
        assert_eq!(
            settings.app_profiles[0].rewrite_prompt.as_deref(),
            Some("Rewrite as a commit message.")
        );
        // Whitespace-only prompts are dropped, not stored as empty strings.
        assert_eq!(settings.app_profiles[1].rewrite_prompt, None);
        // Over-long prompts are capped on a char boundary.
        assert_eq!(
            settings.app_profiles[2]
                .rewrite_prompt
                .as_deref()
                .map(|p| p.chars().count()),
            Some(500)
        );
    }

    #[test]
    fn rewrite_prompt_for_takes_first_matching_profile_with_a_prompt() {
        let settings = Settings {
            app_profiles: vec![
                rewrite_profile("code", Some(RewriteMode::Formal)),
                prompt_profile("code", "Rewrite as a commit message."),
                prompt_profile("slack", "Match a friendly Slack tone."),
            ],
            ..Settings::default()
        };
        // The first matching profile has no prompt; the next matching one wins.
        assert_eq!(
            settings.rewrite_prompt_for("Code.exe"),
            Some("Rewrite as a commit message.")
        );
        assert_eq!(
            settings.rewrite_prompt_for("slack.exe"),
            Some("Match a friendly Slack tone.")
        );
        assert_eq!(settings.rewrite_prompt_for("chrome.exe"), None);
    }

    #[test]
    fn old_config_without_translated_caption_field_loads_disabled() {
        let old = r#"hotkey = "ctrl+shift+space""#;
        let settings: Settings = toml::from_str(old).unwrap();
        assert!(!settings.show_translated_caption);
    }

    #[test]
    fn translated_caption_setting_round_trips_through_toml() {
        let settings = Settings {
            show_translated_caption: true,
            ..Settings::default()
        };
        let text = toml::to_string_pretty(&settings).unwrap();
        let reloaded: Settings = toml::from_str(&text).unwrap();
        assert!(reloaded.show_translated_caption);
    }

    #[test]
    fn clipboard_placeholders_round_trip_through_toml() {
        let settings = Settings {
            clipboard_placeholders: vec!["insert clipboard".into(), "drop it here".into()],
            ..Settings::default()
        };
        let text = toml::to_string_pretty(&settings).unwrap();
        let reloaded: Settings = toml::from_str(&text).unwrap();
        assert_eq!(
            reloaded.clipboard_placeholders,
            settings.clipboard_placeholders
        );
    }

    #[test]
    fn old_config_without_clipboard_placeholders_loads_with_defaults() {
        let old = r#"hotkey = "ctrl+shift+space""#;
        let settings: Settings = toml::from_str(old).unwrap();
        assert_eq!(
            settings.clipboard_placeholders,
            vec![
                "insert clipboard".to_string(),
                "paste clipboard".to_string()
            ]
        );
    }

    #[test]
    fn clamp_normalizes_clipboard_placeholders() {
        let mut settings = Settings {
            clipboard_placeholders: vec![
                "  Insert Clipboard  ".into(),
                "insert clipboard".into(),
                "   ".into(),
                "".into(),
                "Paste Clipboard".into(),
            ],
            ..Settings::default()
        };
        settings.clamp_collections();
        assert_eq!(
            settings.clipboard_placeholders,
            vec![
                "insert clipboard".to_string(),
                "paste clipboard".to_string()
            ]
        );

        // The count cap holds even for a hand-edited config full of entries.
        let mut oversized = Settings {
            clipboard_placeholders: (0..40).map(|i| format!("placeholder {i}")).collect(),
            ..Settings::default()
        };
        oversized.clamp_collections();
        assert_eq!(
            oversized.clipboard_placeholders.len(),
            MAX_CLIPBOARD_PLACEHOLDERS
        );
    }

    #[test]
    fn daily_word_goal_round_trips_through_toml() {
        let settings = Settings {
            daily_word_goal: 500,
            ..Settings::default()
        };
        let text = toml::to_string_pretty(&settings).unwrap();
        let reloaded: Settings = toml::from_str(&text).unwrap();
        assert_eq!(reloaded.daily_word_goal, 500);
    }

    #[test]
    fn old_config_without_daily_word_goal_loads_disabled() {
        let old = r#"hotkey = "ctrl+shift+space""#;
        let settings: Settings = toml::from_str(old).unwrap();
        assert_eq!(settings.daily_word_goal, 0);
    }

    #[test]
    fn clamp_caps_daily_word_goal() {
        let mut settings = Settings {
            daily_word_goal: 1_000_000,
            ..Settings::default()
        };
        settings.clamp_collections();
        assert_eq!(settings.daily_word_goal, MAX_DAILY_WORD_GOAL);

        // In-range values (including 0 = disabled) pass through untouched.
        let mut in_range = Settings {
            daily_word_goal: 500,
            ..Settings::default()
        };
        in_range.clamp_collections();
        assert_eq!(in_range.daily_word_goal, 500);
    }

    #[test]
    fn model_idle_unload_round_trips_through_toml() {
        let settings = Settings {
            model_idle_unload_secs: 900,
            ..Settings::default()
        };
        let text = toml::to_string_pretty(&settings).unwrap();
        let reloaded: Settings = toml::from_str(&text).unwrap();
        assert_eq!(reloaded.model_idle_unload_secs, 900);
    }

    #[test]
    fn old_config_without_model_idle_unload_loads_disabled() {
        let old = r#"hotkey = "ctrl+shift+space""#;
        let settings: Settings = toml::from_str(old).unwrap();
        assert_eq!(settings.model_idle_unload_secs, 0);
        // Freeing a loaded model is opt-in; a fresh config must default off.
        assert_eq!(Settings::default().model_idle_unload_secs, 0);
    }

    #[test]
    fn model_idle_unload_validation_accepts_zero_and_the_range_bounds() {
        for secs in [
            0,
            MODEL_IDLE_UNLOAD_MIN_SECS,
            900,
            MODEL_IDLE_UNLOAD_MAX_SECS,
        ] {
            let settings = Settings {
                model_idle_unload_secs: secs,
                ..Settings::default()
            };
            assert!(settings.validate().is_ok(), "should accept {secs}");
        }
    }

    #[test]
    fn model_idle_unload_validation_rejects_out_of_range_values() {
        for secs in [
            1,
            MODEL_IDLE_UNLOAD_MIN_SECS - 1,
            MODEL_IDLE_UNLOAD_MAX_SECS + 1,
        ] {
            let settings = Settings {
                model_idle_unload_secs: secs,
                ..Settings::default()
            };
            assert!(settings.validate().is_err(), "should reject {secs}");
        }
    }

    #[test]
    fn old_config_without_path_aliases_loads_empty() {
        let old = r#"hotkey = "ctrl+shift+space""#;
        let settings: Settings = toml::from_str(old).unwrap();
        assert!(settings.path_aliases.is_empty());
        assert!(Settings::default().path_aliases.is_empty());
    }

    #[test]
    fn path_aliases_round_trip_through_toml() {
        let settings = Settings {
            path_aliases: vec![
                PathAlias {
                    spoken: "utils".into(),
                    path: "src/utils".into(),
                },
                PathAlias {
                    spoken: "dot env".into(),
                    path: ".env".into(),
                },
            ],
            ..Settings::default()
        };
        let text = toml::to_string_pretty(&settings).unwrap();
        let reloaded: Settings = toml::from_str(&text).unwrap();
        assert_eq!(reloaded.path_aliases, settings.path_aliases);
    }

    #[test]
    fn clamp_normalizes_and_caps_path_aliases() {
        let mut settings = Settings {
            path_aliases: vec![
                PathAlias {
                    spoken: "  Package JSON  ".into(),
                    path: " package.json ".into(),
                },
                PathAlias {
                    spoken: "".into(),
                    path: "src".into(),
                },
                PathAlias {
                    spoken: "source".into(),
                    path: "   ".into(),
                },
            ],
            ..Settings::default()
        };
        settings.clamp_collections();
        assert_eq!(
            settings.path_aliases,
            vec![PathAlias {
                spoken: "package json".into(),
                path: "package.json".into(),
            }]
        );

        let mut oversized = Settings {
            path_aliases: (0..150)
                .map(|i| PathAlias {
                    spoken: format!("alias {i}"),
                    path: format!("path{i}"),
                })
                .collect(),
            ..Settings::default()
        };
        oversized.clamp_collections();
        assert_eq!(oversized.path_aliases.len(), MAX_PATH_ALIASES);
    }

    #[test]
    fn old_config_without_smart_punctuation_field_loads_enabled() {
        let old = r#"hotkey = "ctrl+shift+space""#;
        let settings: Settings = toml::from_str(old).unwrap();
        assert!(settings.smart_punctuation);
        // The fix is wanted by default; the setting exists only to opt out.
        assert!(Settings::default().smart_punctuation);
    }

    #[test]
    fn smart_punctuation_setting_round_trips_through_toml() {
        let settings = Settings {
            smart_punctuation: false,
            ..Settings::default()
        };
        let text = toml::to_string_pretty(&settings).unwrap();
        let reloaded: Settings = toml::from_str(&text).unwrap();
        assert!(!reloaded.smart_punctuation);
    }

    #[test]
    fn old_config_without_mcp_dictation_field_loads_enabled() {
        let old = r#"hotkey = "ctrl+shift+space""#;
        let settings: Settings = toml::from_str(old).unwrap();
        assert!(settings.mcp_dictation_enabled);
    }

    #[test]
    fn mcp_dictation_setting_round_trips_through_toml() {
        let settings = Settings {
            mcp_dictation_enabled: false,
            ..Settings::default()
        };
        let text = toml::to_string_pretty(&settings).unwrap();
        let reloaded: Settings = toml::from_str(&text).unwrap();
        assert!(!reloaded.mcp_dictation_enabled);
    }

    #[test]
    fn old_config_without_local_api_field_loads_disabled() {
        let old = r#"hotkey = "ctrl+shift+space""#;
        let settings: Settings = toml::from_str(old).unwrap();
        assert!(!settings.local_api_enabled);
        // An opt-in network listener must also default off on a fresh config.
        assert!(!Settings::default().local_api_enabled);
    }

    #[test]
    fn local_api_setting_round_trips_through_toml() {
        let settings = Settings {
            local_api_enabled: true,
            ..Settings::default()
        };
        let text = toml::to_string_pretty(&settings).unwrap();
        let reloaded: Settings = toml::from_str(&text).unwrap();
        assert!(reloaded.local_api_enabled);
    }

    #[test]
    fn old_config_without_mic_warm_start_field_loads_disabled() {
        let old = r#"hotkey = "ctrl+shift+space""#;
        let settings: Settings = toml::from_str(old).unwrap();
        assert!(!settings.mic_warm_start);
        // Keeping the mic stream open is opt-in; a fresh config must default off.
        assert!(!Settings::default().mic_warm_start);
    }

    #[test]
    fn mic_warm_start_setting_round_trips_through_toml() {
        let settings = Settings {
            mic_warm_start: true,
            ..Settings::default()
        };
        let text = toml::to_string_pretty(&settings).unwrap();
        let reloaded: Settings = toml::from_str(&text).unwrap();
        assert!(reloaded.mic_warm_start);
    }

    #[test]
    fn old_config_without_cloud_field_loads_with_cloud_none() {
        let old = r#"hotkey = "ctrl+shift+space""#;
        let settings: Settings = toml::from_str(old).unwrap();
        assert!(settings.cloud.is_none());
        // Defaults also carry no cloud table, so a fresh config stays local-only.
        assert!(Settings::default().cloud.is_none());
    }

    #[test]
    fn cloud_config_round_trips_through_settings_toml() {
        let settings = Settings {
            cloud: Some(CloudConfig {
                enabled: false,
                base_url: "https://api.example.com/v1".to_string(),
                model: "gpt-4o-mini".to_string(),
            }),
            ..Settings::default()
        };
        let text = toml::to_string_pretty(&settings).unwrap();
        let reloaded: Settings = toml::from_str(&text).unwrap();
        assert_eq!(reloaded.cloud, settings.cloud);
    }

    #[test]
    fn cloud_table_without_enabled_flag_stays_disabled() {
        let cfg = r#"
            hotkey = "ctrl+shift+space"

            [cloud]
            base_url = "https://api.example.com/v1"
            model = "gpt-4o-mini"
        "#;
        let settings: Settings = toml::from_str(cfg).unwrap();
        let cloud = settings.cloud.expect("cloud table should parse");
        assert!(!cloud.enabled, "enabled must default to false");
    }

    #[test]
    fn profile_rewrite_mode_wins_over_global_default() {
        let settings = Settings {
            default_rewrite_mode: Some(RewriteMode::CleanUp),
            app_profiles: vec![rewrite_profile("code", Some(RewriteMode::Formal))],
            ..Settings::default()
        };
        assert_eq!(
            settings.rewrite_mode_for("Code.exe"),
            Some(RewriteMode::Formal)
        );
    }

    #[test]
    fn profile_without_rewrite_mode_falls_back_to_global_default() {
        let settings = Settings {
            default_rewrite_mode: Some(RewriteMode::BulletList),
            app_profiles: vec![rewrite_profile("code", None)],
            ..Settings::default()
        };
        assert_eq!(
            settings.rewrite_mode_for("Code.exe"),
            Some(RewriteMode::BulletList)
        );
        // No matching profile at all also falls back to the global default.
        assert_eq!(
            settings.rewrite_mode_for("slack.exe"),
            Some(RewriteMode::BulletList)
        );
    }

    #[test]
    fn no_profile_mode_and_no_default_yields_none() {
        let settings = Settings {
            app_profiles: vec![rewrite_profile("code", None)],
            ..Settings::default()
        };
        assert_eq!(settings.rewrite_mode_for("Code.exe"), None);
        assert_eq!(settings.rewrite_mode_for("unmatched.exe"), None);
    }
}
