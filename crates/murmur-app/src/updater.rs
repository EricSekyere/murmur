//! Auto-update: checks GitHub releases on startup and applies updates on
//! request. Signature verification is handled by tauri-plugin-updater
//! against the public key in tauri.conf.json.

use serde::Serialize;
use tauri::{Emitter, Manager};
use tauri_plugin_updater::UpdaterExt;

#[derive(Serialize, Clone)]
struct UpdateAvailable {
    version: String,
    notes: String,
}

/// Check for an update in the background and, if one is found, emit
/// `update-available` so the UI can offer it. On failure (no network, or a
/// `latest.json` that 404s because the repo is private) emit
/// `update-check-failed` so the UI can note that updates aren't reaching the
/// user, instead of failing silently. Never disrupts the app.
pub(crate) fn spawn_startup_check(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        let result = match app.updater() {
            Ok(updater) => updater.check().await,
            Err(e) => Err(e),
        };
        match result {
            Ok(Some(update)) => {
                tracing::info!("Update available: {}", update.version);
                let _ = app.emit(
                    "update-available",
                    UpdateAvailable {
                        version: update.version.clone(),
                        notes: update.body.clone().unwrap_or_default(),
                    },
                );
            }
            Ok(None) => tracing::debug!("No update available"),
            Err(e) => {
                tracing::warn!("Update check failed: {}", e);
                let _ = app.emit(
                    "update-check-failed",
                    serde_json::json!({ "error": e.to_string() }),
                );
            }
        }
    });
}

/// Download, install, and relaunch into the new version. Returns an error
/// string for the UI if anything fails.
#[tauri::command]
pub(crate) async fn install_update(app: tauri::AppHandle) -> Result<(), String> {
    let updater = app.updater().map_err(|e| e.to_string())?;
    let update = updater
        .check()
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "No update available".to_string())?;

    tracing::info!("Installing update {}", update.version);
    update
        .download_and_install(|_chunk, _total| {}, || {})
        .await
        .map_err(|e| e.to_string())?;

    tauri::process::restart(&app.env());
}
