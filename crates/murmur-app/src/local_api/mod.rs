//! Local WebSocket API for editor plugins (VS Code, Neovim): streams the live
//! dictation events the frontend already receives and answers a couple of
//! control requests, so plugins can integrate precisely instead of relying on
//! synthetic keystrokes. Loopback-only, token-authenticated, off by default
//! (`local_api_enabled`); plugins find the ephemeral port and per-run token in
//! the discovery file. Protocol and setup are documented in docs/local-api.md.

mod discovery;
mod protocol;
mod server;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use serde_json::Value;
use tauri::{Listener, Manager};
use tokio::sync::broadcast;

use crate::state::AppState;
use protocol::{ApiBackend, ApiEvent, FORWARDED_EVENTS};

/// Start the local API if enabled; otherwise delete the discovery file so a
/// stale one never advertises a dead or previous endpoint. Toggling the
/// setting takes effect on the next app start.
pub(crate) fn spawn(app: tauri::AppHandle) {
    let path = match discovery::default_path() {
        Ok(path) => path,
        Err(e) => {
            tracing::warn!("Local API disabled, no config dir: {e}");
            return;
        }
    };
    let enabled = app
        .state::<AppState>()
        .settings
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .local_api_enabled;
    if !enabled {
        discovery::clear(&path);
        return;
    }

    let (events, _) = broadcast::channel(server::EVENT_BUFFER);
    forward_frontend_events(&app, events.clone());
    let backend: Arc<dyn ApiBackend> = Arc::new(TauriBackend { app: app.clone() });
    tauri::async_runtime::spawn(async move {
        if let Err(e) = start(path, events, backend).await {
            tracing::warn!("Local API failed to start: {e:#}");
        }
    });
}

/// Bind, publish the discovery file, then serve until the app exits.
async fn start(
    path: PathBuf,
    events: broadcast::Sender<ApiEvent>,
    backend: Arc<dyn ApiBackend>,
) -> Result<()> {
    // Ephemeral loopback port: never a conflict, never reachable off-machine.
    // The discovery file is how clients find it.
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .context("bind local API listener")?;
    let port = listener
        .local_addr()
        .context("local API listener address")?
        .port();
    let token = discovery::generate_token()?;
    discovery::write(&path, port, &token)?;
    // The port is safe to log; the token never is.
    tracing::info!(port, "Local API listening on 127.0.0.1");
    server::run(listener, token.into(), events, backend).await;
    Ok(())
}

/// Mirror the dictation events the frontend receives into the broadcast
/// channel the client tasks fan out from, payloads verbatim. A send error
/// only means no client is connected right now.
fn forward_frontend_events(app: &tauri::AppHandle, events: broadcast::Sender<ApiEvent>) {
    for name in FORWARDED_EVENTS {
        let events = events.clone();
        app.listen(name, move |event| {
            let payload = serde_json::from_str::<Value>(event.payload()).unwrap_or(Value::Null);
            let _ = events.send(ApiEvent { name, payload });
        });
    }
}

/// The live app behind client requests.
struct TauriBackend {
    app: tauri::AppHandle,
}

impl ApiBackend for TauriBackend {
    fn toggle_recording(&self) {
        // Same entry point as the hotkey and UI button: debounced, and safe
        // to call from any thread.
        crate::session::handle_toggle(&self.app);
    }

    fn start_meeting(&self) -> Result<(), String> {
        let state = self
            .app
            .try_state::<AppState>()
            .ok_or("app state unavailable")?;
        // Same claim discipline as the meeting_commands entry point: evaluate
        // the blocker and set the flag under the recording lock, so a racing
        // dictation toggle sees a consistent pair.
        let recording = state.recording.lock().unwrap_or_else(|e| e.into_inner());
        let blocker = crate::meeting_worker::meeting_start_blocker(
            *recording,
            state
                .meeting_active
                .load(std::sync::atomic::Ordering::Acquire),
            state
                .engine_loaded
                .load(std::sync::atomic::Ordering::Acquire),
        );
        if let Some(reason) = blocker {
            return Err(reason.to_string());
        }
        state
            .meeting_active
            .store(true, std::sync::atomic::Ordering::Release);
        drop(recording);
        let handle = crate::meeting_worker::spawn(self.app.clone());
        *state.meeting.lock().unwrap_or_else(|e| e.into_inner()) = Some(handle);
        Ok(())
    }

    fn stop_meeting(&self) -> Result<(), String> {
        let state = self
            .app
            .try_state::<AppState>()
            .ok_or("app state unavailable")?;
        let handle = state
            .meeting
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
            .ok_or("No meeting is being recorded")?;
        // Fire-and-forget shutdown: the final chunk may take seconds of
        // inference, and this trait method must not block the client's
        // event pump. The worker saves the record on its own thread.
        tauri::async_runtime::spawn_blocking(move || handle.stop_and_join());
        Ok(())
    }

    fn status(&self) -> Value {
        let Some(state) = self.app.try_state::<AppState>() else {
            return serde_json::json!({ "recording": false, "processing": false });
        };
        let recording = *state.recording.lock().unwrap_or_else(|e| e.into_inner());
        // "Processing" means an inference currently holds the engine lock —
        // the same condition that keeps the UI in its processing state
        // between a captured phrase and its delivered text.
        let processing = matches!(
            state.engine.try_lock(),
            Err(std::sync::TryLockError::WouldBlock)
        );
        serde_json::json!({ "recording": recording, "processing": processing })
    }
}
