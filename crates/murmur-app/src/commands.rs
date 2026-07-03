//! Tauri commands invoked from the frontend.

use std::sync::Arc;

use murmur_core::config::settings::{
    PHRASE_PAUSE_MAX_SECS, PHRASE_PAUSE_MIN_SECS, SESSION_TIMEOUT_MAX_SECS, VAD_THRESHOLD_MAX,
    VAD_THRESHOLD_MIN,
};
use murmur_core::config::{AppProfile, Settings, TranscriptionProfile};
use murmur_core::output::OutputMode;
use murmur_core::stt::models::{ModelManager, SttModel};
use murmur_core::voice_commands::Snippet;
use tauri::{Emitter, Manager, State};
use tauri_plugin_global_shortcut::GlobalShortcutExt;

use crate::model_setup::spawn_download_and_init;
use crate::session::{end_recording, handle_toggle};
use crate::state::{AppState, ModelChangedEvent, ModelInfo};

/// Return and clear any one-shot startup warning (e.g. hotkey registration failed).
#[tauri::command]
pub(crate) fn take_startup_notice(state: State<'_, AppState>) -> Option<String> {
    state
        .startup_notice
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
}

/// Display-only mode: while suppressed, phrases are shown in the UI but never
/// typed into the focused app (used by the onboarding mic test).
#[tauri::command]
pub(crate) fn set_output_suppressed(state: State<'_, AppState>, suppressed: bool) {
    state
        .suppress_output
        .store(suppressed, std::sync::atomic::Ordering::Release);
}

#[tauri::command]
pub(crate) fn get_status(state: State<'_, AppState>) -> serde_json::Value {
    let recording = *state.recording.lock().unwrap_or_else(|e| e.into_inner());
    let settings = state
        .settings
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    // Lock-free readiness flag: the engine mutex is held for the duration
    // of an inference and must not block UI status polling.
    let model_ready = state
        .engine_loaded
        .load(std::sync::atomic::Ordering::Acquire);

    serde_json::json!({
        "recording": recording,
        "model": settings.model.name(),
        "model_id": settings.model.id(),
        "model_ready": model_ready,
        "mode": if recording { "listening" } else { "idle" },
        "hotkey": settings.hotkey,
        "audio_device": settings.audio_device,
        "output_mode": settings.output_mode,
        "developer_mode": settings.developer_mode,
        "transcription_profile": settings.transcription_profile,
        "phrase_pause_secs": settings.phrase_pause_secs,
        "session_timeout_secs": settings.session_timeout_secs,
        "click_to_stop": settings.click_to_stop,
        "show_widget": settings.show_widget,
        "activation_mode": settings.activation_mode,
        "double_tap_key": settings.double_tap_key,
        "custom_vocabulary": settings.custom_vocabulary,
        "sound_feedback": settings.sound_feedback,
        "vad_threshold": settings.vad_threshold,
        "live_preview": settings.live_preview,
        "snippets": settings.snippets,
        "language": settings.language,
        "translate_to_english": settings.translate_to_english,
        "show_translated_caption": settings.show_translated_caption,
        "model_multilingual": settings.model.is_multilingual(),
        // Compile-time whisper GPU backend, so the UI can say which (if any)
        // this build accelerates Whisper with. Parakeet always runs on CPU.
        "gpu_backend": if cfg!(feature = "vulkan") { "vulkan" }
            else if cfg!(feature = "cuda") { "cuda" }
            else { "none" },
        "app_profiles": settings.app_profiles,
        "caption_position": settings.caption_position,
        "save_history": settings.save_history,
        "clean_speech": settings.clean_speech,
        "codebase_vocab_enabled": settings.indexer.enabled,
        "codebase_vocab_roots": settings
            .indexer
            .project_roots
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect::<Vec<_>>(),
        "codebase_vocab_count": state
            .project_vocab
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len(),
        "app_version": env!("CARGO_PKG_VERSION"),
        "whats_new_seen": settings.whats_new_seen_version,
        "command_mode": state
            .command_mode
            .load(std::sync::atomic::Ordering::Acquire),
        "command_hotkey": crate::command_mode::COMMAND_MODE_HOTKEY,
    })
}

/// Record that the user has seen this version's "What's New" highlights, so the
/// panel doesn't auto-open again until the next update.
#[tauri::command]
pub(crate) fn mark_whats_new_seen(state: State<'_, AppState>) -> Result<(), String> {
    let mut settings = state.settings.lock().unwrap_or_else(|e| e.into_inner());
    settings.whats_new_seen_version = Some(env!("CARGO_PKG_VERSION").to_string());
    let path = Settings::default_path().map_err(|e| e.to_string())?;
    settings
        .save(&path)
        .map_err(|e| format!("Failed to save settings: {e}"))?;
    Ok(())
}

/// Open a native folder picker and return the chosen path, or None if cancelled.
/// Async so the blocking dialog runs off the main thread.
#[tauri::command]
pub(crate) async fn pick_project_folder(app: tauri::AppHandle) -> Option<String> {
    use tauri_plugin_dialog::DialogExt;
    app.dialog()
        .file()
        .blocking_pick_folder()
        .and_then(|p| p.into_path().ok())
        .map(|p| p.to_string_lossy().into_owned())
}

/// Enable/disable codebase vocabulary and optionally set the project root, then
/// persist and re-index (or clear) in the background. The result count is
/// reported via the `codebase-index` event.
#[tauri::command]
pub(crate) fn set_codebase_vocabulary(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    enabled: bool,
    project_roots: Option<Vec<String>>,
) -> Result<(), String> {
    let active = {
        let mut settings = state.settings.lock().unwrap_or_else(|e| e.into_inner());
        settings.indexer.enabled = enabled;
        if let Some(roots) = project_roots {
            // Trim, drop blanks, dedup; clamp_collections caps the count on save.
            let mut seen = std::collections::HashSet::new();
            settings.indexer.project_roots = roots
                .into_iter()
                .map(|r| r.trim().to_string())
                .filter(|r| !r.is_empty() && seen.insert(r.clone()))
                .map(std::path::PathBuf::from)
                .collect();
        }
        let path = Settings::default_path().map_err(|e| e.to_string())?;
        settings
            .save(&path)
            .map_err(|e| format!("Failed to save settings: {e}"))?;
        settings.indexer.enabled && !settings.indexer.project_roots.is_empty()
    };

    if active {
        // spawn_project_index re-reads settings, indexes, and emits the result.
        crate::spawn_project_index(app.clone());
    } else {
        // Disabled or no folder: stop injecting immediately and report it.
        state
            .project_vocab
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        let _ = app.emit(
            "codebase-index",
            serde_json::json!({ "count": 0, "enabled": enabled }),
        );
    }
    // Re-point (or stop) the file watcher for the new root set.
    crate::watcher::rewatch(&app);
    Ok(())
}

#[tauri::command]
pub(crate) fn toggle_recording(app: tauri::AppHandle) -> Result<(), String> {
    handle_toggle(&app);
    Ok(())
}

/// Recent history entries matching `query` (case-insensitive substring; empty
/// or omitted matches all), newest first, capped at `limit` (default 200).
#[tauri::command]
pub(crate) fn get_history(
    state: State<'_, AppState>,
    query: Option<String>,
    limit: Option<usize>,
) -> serde_json::Value {
    let history = state.history.lock().unwrap_or_else(|e| e.into_inner());
    let entries = history.search(query.as_deref().unwrap_or(""), limit.unwrap_or(200));
    serde_json::json!({ "entries": entries })
}

/// Clear all stored history and persist the empty log.
#[tauri::command]
pub(crate) fn clear_history(state: State<'_, AppState>) -> Result<(), String> {
    let mut history = state.history.lock().unwrap_or_else(|e| e.into_inner());
    history.clear();
    history.save(&state.history_path).map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn get_config(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    let settings = state.settings.lock().unwrap_or_else(|e| e.into_inner());
    serde_json::to_value(&*settings).map_err(|e| e.to_string())
}

/// Manually trigger model download (fallback if auto-download failed).
#[tauri::command]
pub(crate) async fn download_model(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    {
        let guard = state.engine.lock().unwrap_or_else(|e| e.into_inner());
        if guard.is_some() {
            return Ok(());
        }
    }
    let model = state
        .settings
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .model;
    spawn_download_and_init(app, Arc::clone(&state.engine), model);
    Ok(())
}

#[tauri::command]
pub(crate) fn list_models(state: State<'_, AppState>) -> Result<Vec<ModelInfo>, String> {
    let model_mgr = ModelManager::new(ModelManager::default_dir().map_err(|e| e.to_string())?);
    let active_model = state
        .settings
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .model;

    Ok(SttModel::all()
        .iter()
        .map(|model| ModelInfo {
            id: model.id().to_string(),
            name: model.name().to_string(),
            backend: model.backend().to_string(),
            size_mb: model.size_mb(),
            ram_estimate_mb: model.ram_estimate_mb(),
            description: model.description().to_string(),
            downloaded: model_mgr.is_downloaded(*model),
            active: *model == active_model,
        })
        .collect())
}

#[tauri::command]
pub(crate) async fn change_model(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    model_id: String,
) -> Result<(), String> {
    let model =
        SttModel::from_name(&model_id).ok_or_else(|| format!("Unknown model '{}'", model_id))?;

    {
        let settings = state.settings.lock().unwrap_or_else(|e| e.into_inner());
        if settings.model == model {
            let engine_guard = state.engine.lock().unwrap_or_else(|e| e.into_inner());
            if engine_guard.is_some() {
                return Ok(());
            }
        }
    }

    // Keep the old engine in place (don't null it) so an in-flight phrase still
    // transcribes; "not ready" only blocks new sessions until the swap.
    end_recording(&app);
    state
        .engine_loaded
        .store(false, std::sync::atomic::Ordering::Release);

    let _ = app.emit(
        "model-changed",
        ModelChangedEvent {
            model_id: model.id().to_string(),
            model_name: model.name().to_string(),
            ready: false,
        },
    );

    // Persist first: download_and_init treats the saved model as the source of
    // truth, so a rapid second switch supersedes this one.
    let mismatch_warning = {
        let mut settings = state.settings.lock().unwrap_or_else(|e| e.into_inner());
        settings.model = model;
        save_settings(&settings);
        language_model_warning(&settings)
    };
    emit_settings_warnings(&app, mismatch_warning.into_iter().collect());

    spawn_download_and_init(app, Arc::clone(&state.engine), model);
    Ok(())
}

#[tauri::command]
pub(crate) fn get_developer_mode(state: State<'_, AppState>) -> bool {
    state
        .settings
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .developer_mode
}

#[tauri::command]
pub(crate) fn set_developer_mode(state: State<'_, AppState>, enabled: bool) -> Result<(), String> {
    let mut settings = state.settings.lock().unwrap_or_else(|e| e.into_inner());
    settings.developer_mode = enabled;
    if let Ok(path) = Settings::default_path() {
        settings.save(&path).map_err(|e| e.to_string())?;
    }
    tracing::info!(
        "Developer mode {}",
        if enabled { "enabled" } else { "disabled" }
    );
    Ok(())
}

/// Update one or more settings fields and persist to config.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub(crate) fn update_settings(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    hotkey: Option<String>,
    audio_device: Option<String>,
    output_mode: Option<String>,
    transcription_profile: Option<String>,
    phrase_pause_secs: Option<f32>,
    session_timeout_secs: Option<f32>,
    click_to_stop: Option<bool>,
    show_widget: Option<bool>,
    activation_mode: Option<String>,
    double_tap_key: Option<String>,
    custom_vocabulary: Option<Vec<String>>,
    sound_feedback: Option<bool>,
    vad_threshold: Option<f32>,
    live_preview: Option<bool>,
    snippets: Option<Vec<Snippet>>,
    language: Option<String>,
    translate_to_english: Option<bool>,
    show_translated_caption: Option<bool>,
    app_profiles: Option<Vec<AppProfile>>,
    caption_position: Option<String>,
    save_history: Option<bool>,
    clean_speech: Option<bool>,
) -> Result<(), String> {
    let mut settings = state.settings.lock().unwrap_or_else(|e| e.into_inner());
    // Only warn about a model/language mismatch if those fields were touched.
    let language_touched = language.is_some() || translate_to_english.is_some();

    if let Some(ref new_hotkey) = hotkey {
        apply_hotkey(&app, &mut settings, new_hotkey)?;
    }
    if let Some(ref mode_str) = output_mode {
        settings.output_mode = parse_output_mode(mode_str)?;
    }
    if let Some(device_name) = audio_device {
        let trimmed = device_name.trim();
        settings.audio_device = (!trimmed.is_empty()).then(|| trimmed.to_string());
    }
    if let Some(profile_str) = transcription_profile {
        settings.transcription_profile = parse_profile(&profile_str)?;
    }
    if let Some(pp) = phrase_pause_secs {
        if !(PHRASE_PAUSE_MIN_SECS..=PHRASE_PAUSE_MAX_SECS).contains(&pp) {
            return Err(format!(
                "phrase_pause_secs must be {}-{}, got {}",
                PHRASE_PAUSE_MIN_SECS, PHRASE_PAUSE_MAX_SECS, pp
            ));
        }
        settings.phrase_pause_secs = pp;
    }
    if let Some(st) = session_timeout_secs {
        if !(0.0..=SESSION_TIMEOUT_MAX_SECS).contains(&st) {
            return Err(format!(
                "session_timeout_secs must be 0-{}, got {}",
                SESSION_TIMEOUT_MAX_SECS, st
            ));
        }
        settings.session_timeout_secs = st;
    }
    if let Some(cts) = click_to_stop {
        settings.click_to_stop = cts;
    }
    if let Some(sw) = show_widget {
        settings.show_widget = sw;
        set_widget_window_visible(&app, sw);
    }
    if let Some(mode) = activation_mode {
        if mode != "toggle" && mode != "hold" {
            return Err(format!("Unknown activation mode: {}", mode));
        }
        settings.activation_mode = mode;
    }
    if let Some(key) = double_tap_key {
        let trimmed = key.trim();
        if !trimmed.is_empty() {
            settings.double_tap_key = trimmed.to_lowercase();
        }
    }
    if let Some(vocab) = custom_vocabulary {
        // Trim and drop blanks; clamp_collections enforces the count/length caps.
        settings.custom_vocabulary = vocab
            .into_iter()
            .map(|w| w.trim().to_string())
            .filter(|w| !w.is_empty())
            .collect();
    }
    if let Some(sf) = sound_feedback {
        settings.sound_feedback = sf;
    }
    if let Some(vt) = vad_threshold {
        if !(VAD_THRESHOLD_MIN..=VAD_THRESHOLD_MAX).contains(&vt) {
            return Err(format!(
                "vad_threshold must be {}-{}, got {}",
                VAD_THRESHOLD_MIN, VAD_THRESHOLD_MAX, vt
            ));
        }
        settings.vad_threshold = vt;
    }
    if let Some(lp) = live_preview {
        settings.live_preview = lp;
    }
    if let Some(snips) = snippets {
        // Trim and drop entries missing a trigger/expansion.
        settings.snippets = snips
            .into_iter()
            .map(|s| Snippet {
                trigger: s.trigger.trim().to_string(),
                expansion: s.expansion,
            })
            .filter(|s| !s.trigger.is_empty() && !s.expansion.is_empty())
            .collect();
        // Warn about snippets that can never fire (shadowed by a built-in, or duplicate).
        let warnings = murmur_core::voice_commands::snippet_warnings(&settings.snippets);
        emit_settings_warnings(&app, warnings);
    }
    if let Some(lang) = language {
        let trimmed = lang.trim();
        if !trimmed.is_empty() {
            settings.language = trimmed.to_lowercase();
        }
    }
    if let Some(tr) = translate_to_english {
        settings.translate_to_english = tr;
    }
    if let Some(stc) = show_translated_caption {
        settings.show_translated_caption = stc;
    }
    if let Some(profiles) = app_profiles {
        // Trim the pattern, drop entries with no pattern or no override.
        settings.app_profiles = profiles
            .into_iter()
            .map(|p| AppProfile {
                app: p.app.trim().to_lowercase(),
                output_mode: p.output_mode,
                developer_mode: p.developer_mode,
                rewrite_mode: p.rewrite_mode,
            })
            .filter(|p| {
                !p.app.is_empty()
                    && (p.output_mode.is_some()
                        || p.developer_mode.is_some()
                        || p.rewrite_mode.is_some())
            })
            .collect();
    }
    if let Some(pos) = caption_position {
        if pos != "pill" && pos != "window" {
            return Err(format!("Unknown caption position: {}", pos));
        }
        tracing::info!("Caption position set to: {}", pos);
        settings.caption_position = pos.clone();
        let _ = app.emit(
            "caption-mode",
            serde_json::json!({ "at_window": pos == "window" }),
        );
    }
    if let Some(sh) = save_history {
        // Turning history off purges what is already stored, so nothing lingers
        // on disk or stays readable through the MCP server. (settings → history
        // lock order matches record_history.)
        if !sh && settings.save_history {
            let mut history = state.history.lock().unwrap_or_else(|e| e.into_inner());
            history.clear();
            // The UI promises "store nothing on disk", so delete the file (and any
            // parse-error .bak) rather than rewriting an empty one that lingers.
            let bak = state.history_path.with_extension("json.bak");
            for path in [&state.history_path, &bak] {
                if let Err(e) = std::fs::remove_file(path)
                    && e.kind() != std::io::ErrorKind::NotFound
                {
                    tracing::warn!(?path, "Failed to remove history file on opt-out: {}", e);
                }
            }
        }
        settings.save_history = sh;
    }
    if let Some(cs) = clean_speech {
        settings.clean_speech = cs;
    }

    // Same gate the loader uses, so the UI can't persist a config it would reject.
    settings.clamp_collections();
    settings.validate().map_err(|e| e.to_string())?;

    if language_touched {
        emit_settings_warnings(
            &app,
            language_model_warning(&settings).into_iter().collect(),
        );
    }

    if let Ok(path) = Settings::default_path() {
        settings.save(&path).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Validate, register, and persist a new hotkey, restoring the old binding
/// when registration fails so the user is never left without one.
fn apply_hotkey(
    app: &tauri::AppHandle,
    settings: &mut Settings,
    new_hotkey: &str,
) -> Result<(), String> {
    let trimmed = new_hotkey.trim().to_string();
    if trimmed.is_empty() {
        return Err("Hotkey cannot be empty".to_string());
    }
    let new_shortcut: tauri_plugin_global_shortcut::Shortcut = trimmed
        .parse()
        .map_err(|e| format!("Invalid hotkey '{}': {:?}", trimmed, e))?;

    let old_shortcut = settings
        .hotkey
        .parse::<tauri_plugin_global_shortcut::Shortcut>()
        .ok();
    if let Some(ref old) = old_shortcut {
        let _ = app.global_shortcut().unregister(*old);
    }

    if let Err(e) = app.global_shortcut().register(new_shortcut) {
        if let Some(ref old) = old_shortcut {
            let _ = app.global_shortcut().register(*old);
        }
        return Err(format!("Failed to register hotkey '{}': {:?}", trimmed, e));
    }

    settings.hotkey = trimmed;
    tracing::info!("Hotkey updated to: {}", settings.hotkey);
    Ok(())
}

/// Emit non-blocking settings warnings to the UI (no-op when empty).
fn emit_settings_warnings(app: &tauri::AppHandle, messages: Vec<String>) {
    if !messages.is_empty() {
        let _ = app.emit(
            "settings-warning",
            serde_json::json!({ "messages": messages }),
        );
    }
}

/// Warning when language/translate needs a multilingual model but the active
/// one is English-only (else non-English speech decodes as garbled English).
fn language_model_warning(settings: &Settings) -> Option<String> {
    if settings.model.is_multilingual() {
        return None;
    }
    let non_english = crate::transcribe::is_non_english_language(&settings.language);
    (settings.translate_to_english || non_english).then(|| {
        format!(
            "The {} model only transcribes English. Switch to a multilingual model \
             (Large v3 Turbo) to dictate in other languages or to translate.",
            settings.model.name()
        )
    })
}

fn parse_output_mode(mode_str: &str) -> Result<OutputMode, String> {
    match mode_str {
        "auto" => Ok(OutputMode::Auto),
        "clipboard" => Ok(OutputMode::Clipboard),
        "keyboard" => Ok(OutputMode::Keyboard),
        "clipboard_paste" => Ok(OutputMode::ClipboardPaste),
        // Legacy configs: stdout makes no sense for the desktop app.
        "stdout" => Ok(OutputMode::Auto),
        _ => Err(format!("Unknown output mode: {}", mode_str)),
    }
}

fn parse_profile(profile_str: &str) -> Result<TranscriptionProfile, String> {
    match profile_str {
        "relaxed" => Ok(TranscriptionProfile::Relaxed),
        "strict" => Ok(TranscriptionProfile::Strict),
        _ => Err(format!("Unknown transcription profile: {}", profile_str)),
    }
}

fn save_settings(settings: &Settings) {
    if let Ok(path) = Settings::default_path()
        && let Err(e) = settings.save(&path)
    {
        tracing::error!("Failed to save settings: {}", e);
    }
}

fn set_widget_window_visible(app: &tauri::AppHandle, visible: bool) {
    if let Some(widget) = app.get_webview_window("widget") {
        let _ = if visible {
            widget.show()
        } else {
            widget.hide()
        };
    }
}

/// Flash the floating pill so the user can spot it, pulling it back on-screen
/// first if it has been dragged off every monitor.
#[tauri::command]
pub(crate) fn locate_widget(app: tauri::AppHandle) -> Result<(), String> {
    let widget = app
        .get_webview_window("widget")
        .ok_or("Widget window not found")?;
    let _ = widget.show();
    let _ = widget.set_always_on_top(false);
    let _ = widget.set_always_on_top(true);
    pull_widget_on_screen(&widget);
    widget.emit("locate-pill", ()).map_err(|e| e.to_string())
}

/// If the widget sits entirely outside every monitor, move it back to the
/// primary monitor's top-left so the flash is actually visible.
fn pull_widget_on_screen(widget: &tauri::WebviewWindow) {
    let (Ok(pos), Ok(size)) = (widget.outer_position(), widget.outer_size()) else {
        return;
    };
    let monitors = widget.available_monitors().unwrap_or_default();
    if monitors.is_empty() {
        return;
    }
    let intersects = |m: &tauri::Monitor| {
        let mp = m.position();
        let ms = m.size();
        pos.x < mp.x + ms.width as i32
            && pos.x + size.width as i32 > mp.x
            && pos.y < mp.y + ms.height as i32
            && pos.y + size.height as i32 > mp.y
    };
    if monitors.iter().any(intersects) {
        return;
    }
    if let Ok(Some(primary)) = widget.primary_monitor() {
        let p = primary.position();
        let _ = widget.set_position(tauri::PhysicalPosition::new(p.x + 60, p.y + 60));
    }
}

#[tauri::command]
pub(crate) fn list_audio_devices() -> Result<Vec<String>, String> {
    use cpal::traits::{DeviceTrait, HostTrait};
    let host = cpal::default_host();
    let mut names = Vec::new();

    let default_name = host
        .default_input_device()
        .and_then(|d| d.name().ok())
        .unwrap_or_default();
    if !default_name.is_empty() {
        names.push(default_name.clone());
    }

    if let Ok(devices) = host.input_devices() {
        for device in devices {
            if let Ok(name) = device.name()
                && name != default_name
            {
                names.push(name);
            }
        }
    }
    Ok(names)
}

#[tauri::command]
pub(crate) fn set_widget_visible(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    visible: bool,
) -> Result<(), String> {
    set_widget_window_visible(&app, visible);
    let mut settings = state.settings.lock().unwrap_or_else(|e| e.into_inner());
    settings.show_widget = visible;
    save_settings(&settings);
    Ok(())
}

/// Wire Murmur into detected MCP clients (Cursor, Claude Desktop) so Claude and
/// Cursor can read the transcription history. The written config points at this
/// app binary, which serves MCP when relaunched as `murmur-app mcp`.
#[tauri::command]
pub(crate) fn mcp_install() -> Result<murmur_mcp::InstallReport, String> {
    murmur_mcp::install(None).map_err(|e| e.to_string())
}

/// Mine local history for distinctive technical terms the user has dictated
/// repeatedly and add them to the custom vocabulary so the decoder biases toward
/// them (Whisper only; Parakeet has no biasing API). Returns how many were added.
#[tauri::command]
pub(crate) fn learn_vocabulary(state: State<'_, AppState>) -> Result<usize, String> {
    // Hold settings across the read+write; lock history inside (settings → history
    // order matches record_history) so the candidate set and the merge are
    // consistent against a concurrent vocabulary edit.
    let mut settings = state.settings.lock().unwrap_or_else(|e| e.into_inner());
    let learned = {
        let history = state.history.lock().unwrap_or_else(|e| e.into_inner());
        history.learn_terms(&settings.custom_vocabulary, 20)
    };
    if learned.is_empty() {
        return Ok(0);
    }
    let before = settings.custom_vocabulary.len();
    settings.custom_vocabulary.extend(learned);
    settings.clamp_collections();
    settings.validate().map_err(|e| e.to_string())?;
    let added = settings.custom_vocabulary.len().saturating_sub(before);
    if let Ok(path) = Settings::default_path() {
        settings.save(&path).map_err(|e| e.to_string())?;
    }
    Ok(added)
}

/// Search the bundled Help articles for `query` and return the best-matching
/// sections, newest-relevant first. Returns an empty list (not an error) when
/// the Help engine is still preparing or unavailable, so the UI degrades
/// gracefully. Runs locally; nothing leaves the machine.
#[cfg(feature = "full")]
#[tauri::command]
pub(crate) fn help_search(
    state: State<'_, AppState>,
    query: String,
) -> Result<Vec<crate::state::HelpResultDto>, String> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    let guard = state.help.lock().unwrap_or_else(|e| e.into_inner());
    let Some(engine) = guard.as_ref() else {
        return Ok(Vec::new());
    };
    let hits = engine.search(trimmed, 6).map_err(|e| e.to_string())?;
    Ok(hits
        .into_iter()
        .map(|h| crate::state::HelpResultDto {
            article: h.article,
            heading: h.heading,
            body: h.body,
            score: h.score,
        })
        .collect())
}

/// Whether the local Help search engine has finished preparing.
#[cfg(feature = "full")]
#[tauri::command]
pub(crate) fn help_ready(state: State<'_, AppState>) -> bool {
    state
        .help
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .is_some()
}

/// On-device usage stats (words, top apps, streak) derived from local history.
#[tauri::command]
pub(crate) fn get_usage_stats(state: State<'_, AppState>) -> murmur_core::history::UsageStats {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    state
        .history
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .stats(now_ms)
}
