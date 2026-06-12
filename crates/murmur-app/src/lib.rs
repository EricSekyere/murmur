//! Murmur desktop app: tray, windows, and wiring between the audio worker,
//! STT engine, and frontend.

mod audio_worker;
mod calibration;
mod commands;
mod focus;
mod input;
mod model_setup;
mod session;
mod state;
mod transcribe;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Context;
use murmur_core::config::Settings;
use murmur_core::output::OutputMode;
use murmur_core::stt::engine::SttEngine;
use tauri::{
    Manager,
    image::Image,
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};
use tauri_plugin_global_shortcut::GlobalShortcutExt;

use state::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() -> anyhow::Result<()> {
    let _log_guard = init_logging();
    let settings = load_settings()?;
    let model = settings.model;
    let hotkey = settings.hotkey.clone();
    let show_widget_on_start = settings.show_widget;

    // Engine loads in the background so startup is instant; the UI shows a
    // loading banner until it is ready.
    let engine: Arc<Mutex<Option<SttEngine>>> = Arc::new(Mutex::new(None));
    let engine_for_setup = Arc::clone(&engine);

    tauri::Builder::default()
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(move |app, _shortcut, event| {
                    input::handle_hotkey_event(app, event.state);
                })
                .build(),
        )
        .manage(AppState {
            audio: std::sync::OnceLock::new(),
            engine,
            engine_loaded: std::sync::atomic::AtomicBool::new(false),
            recording: Mutex::new(false),
            settings: Mutex::new(settings),
            last_toggle: Mutex::new(Instant::now() - Duration::from_secs(10)),
            session_prev_text: Mutex::new(String::new()),
            #[cfg(windows)]
            previous_foreground: Mutex::new(0),
            #[cfg(windows)]
            last_external_foreground: Mutex::new(0),
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
            commands::toggle_recording,
            commands::get_config,
            commands::download_model,
            commands::list_models,
            commands::change_model,
            commands::get_developer_mode,
            commands::set_developer_mode,
            commands::update_settings,
            commands::list_audio_devices,
            commands::set_widget_visible,
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
    Settings::migrate_from_voitex();
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
    configure_widget(app, show_widget_on_start);

    model_setup::spawn_download_and_init(app.handle().clone(), engine, model);
    input::spawn_global_input_listener(app.handle().clone());

    tracing::info!("Murmur app started");
    Ok(())
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

    match hotkey.parse::<tauri_plugin_global_shortcut::Shortcut>() {
        Ok(shortcut) => match app.global_shortcut().register(shortcut) {
            Ok(()) => tracing::info!("Registered global hotkey: {}", hotkey),
            Err(e) => tracing::warn!(
                "Could not register hotkey '{}': {:?} (app still works via UI)",
                hotkey,
                e
            ),
        },
        Err(e) => tracing::warn!("Could not parse hotkey '{}': {:?}", hotkey, e),
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
