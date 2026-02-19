use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager,
};

/// Tauri command: get the current app status.
#[tauri::command]
fn get_status() -> serde_json::Value {
    serde_json::json!({
        "recording": false,
        "model": "small.en",
        "mode": "idle"
    })
}

/// Tauri command: start listening for voice input.
#[tauri::command]
fn start_listening() -> Result<(), String> {
    tracing::info!("Start listening requested");
    // TODO: Wire up voitex-core audio capture
    Ok(())
}

/// Tauri command: stop listening.
#[tauri::command]
fn stop_listening() -> Result<(), String> {
    tracing::info!("Stop listening requested");
    // TODO: Wire up voitex-core stop + transcribe
    Ok(())
}

/// Tauri command: get the current configuration.
#[tauri::command]
fn get_config() -> Result<serde_json::Value, String> {
    let path = voitex_core::config::Settings::default_path().map_err(|e| e.to_string())?;
    let settings = voitex_core::config::Settings::load(&path).map_err(|e| e.to_string())?;
    serde_json::to_value(settings).map_err(|e| e.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            get_status,
            start_listening,
            stop_listening,
            get_config,
        ])
        .setup(|app| {
            // Build tray menu
            let show_i = MenuItem::with_id(app, "show", "Show", true, None::<&str>)?;
            let quit_i = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show_i, &quit_i])?;

            // Build tray icon
            let _tray = TrayIconBuilder::new()
                .icon(app.default_window_icon().cloned().unwrap())
                .menu(&menu)
                .show_menu_on_left_click(false)
                .tooltip("Voitex - Voice to Text")
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "quit" => {
                        app.exit(0);
                    }
                    "show" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.unminimize();
                            let _ = window.show();
                            let _ = window.set_focus();
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
                        let app = tray.app_handle();
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.unminimize();
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                })
                .build(app)?;

            tracing::info!("Voitex app started");
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Voitex");
}
