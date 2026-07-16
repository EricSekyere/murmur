//! Murmur desktop app: tray, windows, and wiring between the audio worker,
//! STT engine, and frontend.

mod audio_worker;
mod calibration;
mod caption;
// Public: the command-mode executor is exercised by the UI layer.
pub mod command_exec;
mod command_mode;
mod commands;
mod focus;
mod input;
mod model_setup;
pub mod native_actions;
mod preview;
mod rewrite;
mod session;
mod sound;
mod state;
mod transcribe;
mod updater;
mod watcher;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Context;
use murmur_core::config::Settings;
use murmur_core::output::OutputMode;
use murmur_core::stt::engine::SttEngine;
use tauri::{
    Emitter, Manager,
    image::Image,
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};
use tauri_plugin_global_shortcut::GlobalShortcutExt;

use state::AppState;

/// Run as a stdio MCP server (`murmur-app mcp`), which is what Claude / Cursor
/// spawn after the in-app "Connect to editor" button writes their config. Talks
/// JSON-RPC over stdin/stdout and never starts the GUI; the protocol owns
/// stdout, so the only diagnostics go to stderr. Returns a process exit code.
pub fn run_mcp() -> i32 {
    // A fresh stderr subscriber: file logging isn't set up in this mode, and
    // stdout must stay pure JSON-RPC.
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .try_init();

    let runtime = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            tracing::error!("Failed to start MCP runtime: {e}");
            return 1;
        }
    };
    match runtime.block_on(murmur_mcp::serve()) {
        Ok(()) => 0,
        Err(e) => {
            tracing::error!("MCP server error: {e}");
            1
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() -> anyhow::Result<()> {
    // Migrate the legacy config directory before logging runs: init_logging
    // creates the new murmur directory, and the migration only fires when that
    // directory does not yet exist.
    Settings::migrate_from_voitex();
    let _log_guard = init_logging();
    let settings = load_settings()?;
    let model = settings.model;
    let hotkey = settings.hotkey.clone();
    let show_widget_on_start = settings.show_widget;

    let history_path = murmur_core::history::History::default_path()
        .context("Failed to determine history path")?;
    let history = murmur_core::history::History::load(&history_path);

    let insights_path = murmur_core::insights::Insights::default_path()
        .context("Failed to determine insights path")?;
    let mut insights = murmur_core::insights::Insights::load(&insights_path);
    // First run with the aggregate: seed it from whatever history is still
    // stored so records start with the recent past. Best effort — a failed
    // save must never block startup.
    if insights.is_empty() {
        insights.backfill_from_history(&history);
        if !insights.is_empty()
            && let Err(e) = insights.save(&insights_path)
        {
            tracing::warn!("Failed to save backfilled insights: {}", e);
        }
    }

    // Engine loads in the background so startup is instant; the UI shows a
    // loading banner until it is ready.
    let engine: Arc<Mutex<Option<SttEngine>>> = Arc::new(Mutex::new(None));
    let engine_for_setup = Arc::clone(&engine);

    let command_state =
        command_mode::CommandState::new().context("initializing command mode context")?;
    let command_shortcut = command_mode::hotkey_shortcut();

    tauri::Builder::default()
        // Must be the first plugin: a second launch hands off to the running
        // instance and exits, so two copies can never run at once. Multiple
        // instances would double-capture audio and race on the clipboard
        // during clipboard+paste output (corrupting what gets pasted).
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            // An `mcp` launch is an editor spawning the stdio server; the normal
            // `main()` branch handles it before Tauri inits, but guard here too
            // so a stale/downgraded config that reaches the GUI never yanks the
            // window to the front on every editor connect.
            if args.iter().any(|a| a == "mcp") {
                return;
            }
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(move |app, shortcut, event| {
                    // The command-mode hotkey is a separate activation channel
                    // from dictation. Pressed only, so the key release does
                    // not re-toggle.
                    if command_shortcut.as_ref() == Some(shortcut) {
                        if event.state == tauri_plugin_global_shortcut::ShortcutState::Pressed {
                            command_mode::toggle_mode(app);
                        }
                        return;
                    }
                    input::handle_hotkey_event(app, event.state);
                })
                .build(),
        )
        .manage(AppState {
            audio: std::sync::OnceLock::new(),
            engine,
            engine_loaded: std::sync::atomic::AtomicBool::new(false),
            recording: Mutex::new(false),
            session_generation: std::sync::atomic::AtomicU64::new(0),
            streaming_worker: Mutex::new(None),
            settings: Mutex::new(settings),
            last_toggle: Mutex::new(Instant::now() - Duration::from_secs(10)),
            session_prev_text: Mutex::new(String::new()),
            last_delivered_len: Mutex::new(0),
            history: Mutex::new(history),
            history_path,
            insights: Mutex::new(insights),
            insights_path,
            session_dev_mode: Mutex::new(None),
            #[cfg(windows)]
            previous_foreground: Mutex::new(0),
            #[cfg(windows)]
            last_external_foreground: Mutex::new(0),
            startup_notice: Mutex::new(None),
            suppress_output: std::sync::atomic::AtomicBool::new(false),
            project_vocab: Mutex::new(Vec::new()),
            project_files: Mutex::new(Vec::new()),
            codebase_watcher: Mutex::new(None),
            indexing: std::sync::atomic::AtomicBool::new(false),
            index_pending: std::sync::atomic::AtomicBool::new(false),
            command_mode: std::sync::atomic::AtomicBool::new(false),
            command: tokio::sync::Mutex::new(command_state),
            #[cfg(feature = "full")]
            help: Arc::new(Mutex::new(None)),
            #[cfg(feature = "llm")]
            llm: Arc::new(Mutex::new(None)),
        })
        .on_window_event(|window, event| {
            // Hide the main window on close so the tray can re-show it.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event
                && window.label() == "main"
            {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_status,
            commands::take_startup_notice,
            commands::set_output_suppressed,
            commands::toggle_recording,
            commands::get_history,
            commands::clear_history,
            commands::get_config,
            commands::download_model,
            commands::list_models,
            commands::change_model,
            commands::get_developer_mode,
            commands::set_developer_mode,
            commands::update_settings,
            commands::list_audio_devices,
            commands::set_widget_visible,
            commands::locate_widget,
            commands::pick_project_folder,
            commands::set_codebase_vocabulary,
            commands::mark_whats_new_seen,
            commands::mcp_install,
            commands::get_usage_stats,
            commands::get_records,
            commands::learn_vocabulary,
            command_mode::run_command,
            command_mode::confirm_pending,
            command_mode::cancel_pending,
            rewrite::rewrite_selection,
            #[cfg(feature = "full")]
            commands::help_search,
            #[cfg(feature = "full")]
            commands::help_ready,
            updater::install_update,
        ])
        .setup(move |app| setup_app(app, engine_for_setup, model, &hotkey, show_widget_on_start))
        .run(tauri::generate_context!())
        .context("error while running Murmur")?;

    Ok(())
}

/// File-based logging so release builds have visible logs. The returned
/// guard must stay alive for the lifetime of the app.
fn init_logging() -> tracing_appender::non_blocking::WorkerGuard {
    let log_dir = if let Ok(appdata) = std::env::var("APPDATA") {
        std::path::PathBuf::from(appdata).join("murmur")
    } else if let Ok(home) = std::env::var("HOME") {
        std::path::PathBuf::from(home)
            .join(".config")
            .join("murmur")
    } else {
        std::path::PathBuf::from(".")
    };
    let _ = std::fs::create_dir_all(&log_dir);

    let file_appender = tracing_appender::rolling::daily(&log_dir, "app");
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(non_blocking)
        .with_ansi(false)
        .init();
    guard
}

fn load_settings() -> anyhow::Result<Settings> {
    let config_path = Settings::default_path().context("Failed to determine config path")?;
    let mut settings = Settings::load(&config_path).context("Failed to load settings")?;

    // Legacy configs: stdout is display-only and makes no sense for the
    // desktop app, where users expect text delivered to the active window.
    if settings.output_mode == OutputMode::Stdout {
        settings.output_mode = OutputMode::Auto;
        if let Err(e) = settings.save(&config_path) {
            tracing::warn!("Failed to persist output_mode migration: {}", e);
        }
        tracing::info!("Migrated desktop output mode from stdout to auto");
    }
    Ok(settings)
}

fn setup_app(
    app: &mut tauri::App,
    engine: Arc<Mutex<Option<SttEngine>>>,
    model: murmur_core::stt::models::SttModel,
    hotkey: &str,
    show_widget_on_start: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let state = app.state::<AppState>();
    let handle = audio_worker::Handle::spawn(app.handle().clone());
    let _ = state.audio.set(handle);

    #[cfg(windows)]
    focus::spawn_foreground_tracker(app.handle().clone());

    build_tray(app)?;
    register_hotkey(app, hotkey);
    command_mode::register_hotkey(app);
    configure_widget(app, show_widget_on_start);

    // The roaming caption is display-only: make it click-through so it never
    // intercepts input meant for the window beneath it.
    if let Some(caption) = app.get_webview_window("caption") {
        let _ = caption.set_ignore_cursor_events(true);
    }

    model_setup::spawn_download_and_init(app.handle().clone(), engine, model);
    input::spawn_global_input_listener(app.handle().clone());
    updater::spawn_startup_check(app.handle().clone());
    spawn_project_index(app.handle().clone());
    #[cfg(feature = "full")]
    spawn_help_index(app.handle().clone());
    watcher::rewatch(app.handle());

    tracing::info!("Murmur app started");
    Ok(())
}

/// Index the configured project in the background and store its symbols in
/// `project_vocab`, so transcription can bias toward them. No-ops when the
/// indexer is disabled or no project root is set. Safe to call again (e.g.
/// after a settings change) — it overwrites the cached result.
pub(crate) fn spawn_project_index(app: tauri::AppHandle) {
    use std::sync::atomic::Ordering;

    {
        let state = app.state::<AppState>();
        let settings = state.settings.lock().unwrap_or_else(|e| e.into_inner());
        if !settings.indexer.enabled || settings.indexer.project_roots.is_empty() {
            return;
        }
    }

    // Coalesce: if a scan is already running, flag that another is needed and
    // let the running one pick it up, instead of stacking concurrent full scans
    // (a burst of file-watcher events over a large tree would otherwise spawn an
    // unbounded number of indexing threads).
    {
        let state = app.state::<AppState>();
        if state.indexing.swap(true, Ordering::AcqRel) {
            state.index_pending.store(true, Ordering::Release);
            return;
        }
    }

    std::thread::spawn(move || {
        loop {
            run_one_index(&app);

            // A change that arrived during the scan set `index_pending`; run once
            // more for it. The re-check after releasing `indexing` re-claims a
            // change that landed in the tiny window between the swap and store.
            let state = app.state::<AppState>();
            if state.index_pending.swap(false, Ordering::AcqRel) {
                continue;
            }
            state.indexing.store(false, Ordering::Release);
            if !state.index_pending.load(Ordering::Acquire) {
                break;
            }
            if state.indexing.swap(true, Ordering::AcqRel) {
                break; // another invocation re-claimed it
            }
        }
    });
}

/// One project-index pass: read the current roots, scan, and publish the result.
fn run_one_index(app: &tauri::AppHandle) {
    use murmur_core::indexer::{IndexConfig, index_project_files, index_projects};

    let (enabled, roots, max_symbols, extensions) = {
        let state = app.state::<AppState>();
        let settings = state.settings.lock().unwrap_or_else(|e| e.into_inner());
        let idx = &settings.indexer;
        (
            idx.enabled,
            idx.project_roots.clone(),
            idx.max_symbols,
            idx.extensions.clone(),
        )
    };
    if !enabled || roots.is_empty() {
        return;
    }

    let cfg = IndexConfig {
        max_symbols,
        extensions,
        ..IndexConfig::default()
    };
    match index_projects(&roots, &cfg) {
        Ok(symbols) => {
            let count = symbols.len();
            tracing::info!(
                "Codebase index: {} symbols from {} folder(s)",
                count,
                roots.len()
            );
            let state = app.state::<AppState>();
            *state
                .project_vocab
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = symbols;
            let _ = app.emit(
                "codebase-index",
                serde_json::json!({ "count": count, "enabled": true }),
            );
        }
        Err(e) => {
            tracing::warn!("Codebase index failed: {:#}", e);
            let _ = app.emit(
                "codebase-index",
                serde_json::json!({ "count": 0, "enabled": true, "error": e.to_string() }),
            );
        }
    }

    // Path index for spoken file resolution ("open the … file" in command
    // mode). Best-effort like the vocabulary: a failure logs and keeps the
    // previous list.
    match index_project_files(&roots, &cfg) {
        Ok(files) => {
            tracing::info!(count = files.len(), "codebase file-path index ready");
            let state = app.state::<AppState>();
            *state
                .project_files
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = files;
        }
        Err(e) => {
            tracing::warn!("Codebase file-path index failed: {:#}", e);
        }
    }
}

/// Prepare the local Help search engine in the background: download the small
/// embedder model if missing, then embed the bundled corpus off the async
/// reactor and store the ready engine in `AppState::help`. Emits `help-ready`
/// so the Help view can drop its "preparing" note. Non-fatal: on any failure
/// Help simply stays unavailable (the command returns no hits).
#[cfg(feature = "full")]
pub(crate) fn spawn_help_index(app: tauri::AppHandle) {
    use murmur_core::help::{self, HelpEngine};

    tauri::async_runtime::spawn(async move {
        if let Err(e) = help::download().await {
            tracing::warn!(
                "Help embedder download failed; Help search unavailable: {:#}",
                e
            );
            return;
        }

        // Corpus embedding is CPU-heavy: build off the reactor.
        let engine = match tokio::task::spawn_blocking(HelpEngine::load).await {
            Ok(Ok(engine)) => engine,
            Ok(Err(e)) => {
                tracing::warn!("Help engine load failed; Help search unavailable: {:#}", e);
                return;
            }
            Err(e) => {
                tracing::warn!("Help engine load task panicked; Help search unavailable: {e}");
                return;
            }
        };

        let Some(state) = app.try_state::<AppState>() else {
            return;
        };
        *state.help.lock().unwrap_or_else(|e| e.into_inner()) = Some(engine);
        let _ = app.emit("help-ready", serde_json::json!({ "ready": true }));
        tracing::info!("Help search ready");
    });
}

fn build_tray(app: &mut tauri::App) -> tauri::Result<()> {
    let show_i = MenuItem::with_id(app, "show", "Show", true, None::<&str>)?;
    let widget_i = MenuItem::with_id(app, "toggle_widget", "Toggle Widget", true, None::<&str>)?;
    let quit_i = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show_i, &widget_i, &quit_i])?;

    let icon = app.default_window_icon().cloned().unwrap_or_else(|| {
        tracing::warn!("No default window icon found, using fallback");
        Image::new_owned(vec![0, 0, 0, 0], 1, 1)
    });

    TrayIconBuilder::new()
        .icon(icon)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .tooltip("Murmur - Voice to Text")
        .on_menu_event(|app, event| match event.id.as_ref() {
            "quit" => app.exit(0),
            "show" => show_main_window(app),
            "toggle_widget" => {
                if let Some(widget) = app.get_webview_window("widget") {
                    let _ = if widget.is_visible().unwrap_or(false) {
                        widget.hide()
                    } else {
                        widget.show()
                    };
                }
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

fn show_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

fn register_hotkey(app: &tauri::App, hotkey: &str) {
    // Clear stale registrations from a previous instance (e.g. after a
    // force-kill that skipped cleanup).
    let _ = app.global_shortcut().unregister_all();

    let failure = match hotkey.parse::<tauri_plugin_global_shortcut::Shortcut>() {
        Ok(shortcut) => match app.global_shortcut().register(shortcut) {
            Ok(()) => {
                tracing::info!("Registered global hotkey: {}", hotkey);
                None
            }
            Err(e) => {
                tracing::warn!(
                    "Could not register hotkey '{}': {:?} (app still works via UI)",
                    hotkey,
                    e
                );
                Some(format!(
                    "Hotkey '{hotkey}' is already in use by another app. Murmur still works \
                     via the mic button or your double-tap key; pick a new hotkey in Settings."
                ))
            }
        },
        Err(e) => {
            tracing::warn!("Could not parse hotkey '{}': {:?}", hotkey, e);
            Some(format!(
                "Hotkey '{hotkey}' is not valid. Set a working hotkey in Settings."
            ))
        }
    };

    if let Some(message) = failure {
        let state = app.state::<AppState>();
        *state
            .startup_notice
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(message);
    }
}

/// Make the widget window truly transparent: clear the WebView2 background
/// and disable the DWM shadow at runtime (the config flag alone has been
/// unreliable on some Windows builds).
fn configure_widget(app: &tauri::App, show_on_start: bool) {
    if let Some(widget) = app.get_webview_window("widget") {
        let _ = widget.set_background_color(Some(tauri::window::Color(0, 0, 0, 0)));
        let _ = widget.set_shadow(false);
        if !show_on_start {
            let _ = widget.hide();
        }
    }
}
