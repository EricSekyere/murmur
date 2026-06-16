//! Tauri commands invoked from the frontend.

use std::sync::Arc;

use murmur_core::config::{AppProfile, Settings, TranscriptionProfile};
use murmur_core::output::OutputMode;
use murmur_core::stt::models::{ModelManager, SttModel};
use murmur_core::voice_commands::Snippet;
use tauri::{Emitter, Manager, State};
use tauri_plugin_global_shortcut::GlobalShortcutExt;

use crate::model_setup::spawn_download_and_init;
use crate::session::{end_recording, handle_toggle};
use crate::state::{AppState, ModelChangedEvent, ModelInfo};

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
        "model_multilingual": settings.model.is_multilingual(),
        "app_profiles": settings.app_profiles,
        "caption_position": settings.caption_position,
    })
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

    // Changing models while recording would silently drop all phrases once
    // the engine becomes None, so stop first. Use the un-debounced stop:
    // handle_toggle can swallow this stop if the user just toggled, leaving
    // the mic running against a None engine.
    end_recording(&app);

    {
        let mut guard = state.engine.lock().unwrap_or_else(|e| e.into_inner());
        *guard = None;
    }
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

    {
        let mut settings = state.settings.lock().unwrap_or_else(|e| e.into_inner());
        settings.model = model;
        save_settings(&settings);
    }

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
    app_profiles: Option<Vec<AppProfile>>,
    caption_position: Option<String>,
) -> Result<(), String> {
    let mut settings = state.settings.lock().unwrap_or_else(|e| e.into_inner());

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
        if !(0.3..=10.0).contains(&pp) {
            return Err(format!("phrase_pause_secs must be 0.3-10.0, got {}", pp));
        }
        settings.phrase_pause_secs = pp;
    }
    if let Some(st) = session_timeout_secs {
        if !(0.0..=300.0).contains(&st) {
            return Err(format!("session_timeout_secs must be 0-300, got {}", st));
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
        // Trim, drop blanks, cap at 100 entries to keep the prompt bounded.
        settings.custom_vocabulary = vocab
            .into_iter()
            .map(|w| w.trim().to_string())
            .filter(|w| !w.is_empty())
            .take(100)
            .collect();
    }
    if let Some(sf) = sound_feedback {
        settings.sound_feedback = sf;
    }
    if let Some(vt) = vad_threshold {
        if !(0.05..=0.95).contains(&vt) {
            return Err(format!("vad_threshold must be 0.05-0.95, got {}", vt));
        }
        settings.vad_threshold = vt;
    }
    if let Some(lp) = live_preview {
        settings.live_preview = lp;
    }
    if let Some(snips) = snippets {
        // Trim, drop entries missing a trigger or expansion, cap at 100.
        settings.snippets = snips
            .into_iter()
            .map(|s| Snippet {
                trigger: s.trigger.trim().to_string(),
                expansion: s.expansion,
            })
            .filter(|s| !s.trigger.is_empty() && !s.expansion.is_empty())
            .take(100)
            .collect();
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
    if let Some(profiles) = app_profiles {
        // Trim the app pattern, drop entries with no pattern or no override,
        // cap at 50 to keep session-start matching cheap.
        settings.app_profiles = profiles
            .into_iter()
            .map(|p| AppProfile {
                app: p.app.trim().to_lowercase(),
                output_mode: p.output_mode,
                developer_mode: p.developer_mode,
            })
            .filter(|p| {
                !p.app.is_empty() && (p.output_mode.is_some() || p.developer_mode.is_some())
            })
            .take(50)
            .collect();
    }
    if let Some(pos) = caption_position {
        if pos != "pill" && pos != "window" {
            return Err(format!("Unknown caption position: {}", pos));
        }
        settings.caption_position = pos.clone();
        let _ = app.emit(
            "caption-mode",
            serde_json::json!({ "at_window": pos == "window" }),
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
    // Make sure it is visible and sitting above other windows.
    let _ = widget.show();
    let _ = widget.set_always_on_top(false);
    let _ = widget.set_always_on_top(true);
    pull_widget_on_screen(&widget);
    // The widget JS plays an attention animation on this event.
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
