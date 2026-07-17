//! Model/runtime downloads and STT engine initialization.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::Context;
use murmur_core::stt::engine::SttEngine;
use murmur_core::stt::models::{Backend, ModelManager, SttModel};
#[cfg(feature = "vad")]
use murmur_core::stt::runtime;
use tauri::{Emitter, Manager};

use crate::state::{AppState, ModelChangedEvent, ModelDownloadProgress};

/// Spawn a background task that downloads the model, initializes the
/// engine, and reports progress to the UI.
pub(crate) fn spawn_download_and_init(
    app: tauri::AppHandle,
    engine: Arc<Mutex<Option<SttEngine>>>,
    model: SttModel,
) {
    tauri::async_runtime::spawn(async move {
        if let Err(e) = download_and_init_model(&app, &engine, model).await {
            tracing::error!("Model download/init failed: {}", e);
            // A previously-loaded engine is still usable after a failed switch:
            // resync settings + readiness to it so the UI doesn't report the
            // failed model as active.
            let loaded = engine
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_ref()
                .and_then(|engine| engine.model());
            if let Some(loaded) = loaded
                && let Some(app_state) = app.try_state::<AppState>()
            {
                resync_to_loaded(&app, &app_state, loaded);
            }
            emit_progress(
                &app,
                0,
                &format!("Download failed: {}", e),
                false,
                Some(e.to_string()),
            );
        }
    });
}

/// After a failed switch, point settings, the readiness flag, and the UI back
/// at the engine that's actually loaded.
fn resync_to_loaded(app: &tauri::AppHandle, state: &AppState, loaded: SttModel) {
    {
        let mut settings = state.settings.lock().unwrap_or_else(|e| e.into_inner());
        if settings.model != loaded {
            settings.model = loaded;
            if let Ok(path) = murmur_core::config::Settings::default_path() {
                let _ = settings.save(&path);
            }
        }
    }
    state
        .engine_loaded
        .store(true, std::sync::atomic::Ordering::Release);
    state
        .idle_unloaded
        .store(false, std::sync::atomic::Ordering::Release);
    crate::idle_unload::touch(state);
    let _ = app.emit(
        "model-changed",
        ModelChangedEvent {
            model_id: loaded.id().to_string(),
            model_name: loaded.name().to_string(),
            ready: true,
        },
    );
}

/// Whether `model` is still the selected model: download_and_init skips a load
/// the user already switched away from, so the engine never goes out of sync.
fn is_current_selection(app: &tauri::AppHandle, model: SttModel) -> bool {
    app.try_state::<AppState>()
        .map(|state| {
            state
                .settings
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .model
                == model
        })
        .unwrap_or(true)
}

fn emit_progress(
    app: &tauri::AppHandle,
    percent: u8,
    message: &str,
    done: bool,
    error: Option<String>,
) {
    let _ = app.emit(
        "model-download-progress",
        ModelDownloadProgress {
            percent,
            message: message.to_string(),
            done,
            error,
        },
    );
}

fn as_percent(downloaded: u64, total: Option<u64>) -> u8 {
    total
        .filter(|t| *t > 0)
        .map(|t| ((downloaded * 100) / t).min(100) as u8)
        .unwrap_or(0)
}

async fn download_and_init_model(
    app: &tauri::AppHandle,
    engine: &Arc<Mutex<Option<SttEngine>>>,
    model: SttModel,
) -> anyhow::Result<()> {
    let model_mgr = ModelManager::new(
        ModelManager::default_dir().context("Failed to determine models directory")?,
    );

    download_ort_if_needed(app, model).await?;
    download_vad_if_needed(app).await;
    download_model_files(app, &model_mgr, model).await?;

    // Skip the costly init if the user already switched away.
    if !is_current_selection(app, model) {
        tracing::info!(
            "Model selection changed before init; skipping {}",
            model.name()
        );
        return Ok(());
    }

    emit_progress(app, 100, "Loading model...", false, None);
    let stt = init_engine(&model_mgr, model).await?;

    // Re-check after init: a switch during init wins; don't commit a stale engine.
    if !is_current_selection(app, model) {
        tracing::info!(
            "Model selection changed during init; discarding {}",
            model.name()
        );
        return Ok(());
    }

    {
        let mut guard = engine.lock().unwrap_or_else(|e| e.into_inner());
        *guard = Some(stt);
    }
    if let Some(app_state) = app.try_state::<AppState>() {
        app_state
            .engine_loaded
            .store(true, std::sync::atomic::Ordering::Release);
        // A fresh engine starts a fresh idle window; clearing the marker also
        // prevents a later activation from kicking a duplicate load.
        app_state
            .idle_unloaded
            .store(false, std::sync::atomic::Ordering::Release);
        crate::idle_unload::touch(&app_state);
    }

    emit_progress(app, 100, "Model ready", true, None);
    let _ = app.emit(
        "model-changed",
        ModelChangedEvent {
            model_id: model.id().to_string(),
            model_name: model.name().to_string(),
            ready: true,
        },
    );
    tracing::info!("Model {} downloaded and engine initialized", model.name());
    Ok(())
}

/// The ONNX Runtime DLL is needed by both Silero VAD and Parakeet; fetch it
/// when either consumer is present.
///
/// Gated on `vad`: `murmur_core::stt::runtime` is only compiled when core has
/// `parakeet`, `vad`, or `help`, and in the app's feature graph those are
/// reachable only through `vad`/`full` (and `full` enables `vad`). Without them
/// there is no ONNX Runtime consumer, so the no-op variant below applies.
#[cfg(feature = "vad")]
async fn download_ort_if_needed(app: &tauri::AppHandle, model: SttModel) -> anyhow::Result<()> {
    #[cfg(feature = "full")]
    let needs_ort =
        model.backend() == Backend::Parakeet || !murmur_core::audio::vad::is_downloaded();
    #[cfg(not(feature = "full"))]
    let needs_ort = model.backend() == Backend::Parakeet;

    if !needs_ort || runtime::is_downloaded() {
        return Ok(());
    }

    emit_progress(app, 0, "Downloading ONNX Runtime...", false, None);
    let app_ref = app.clone();
    runtime::download_with_progress(move |downloaded, total| {
        let percent = as_percent(downloaded, total);
        emit_progress(
            &app_ref,
            percent,
            &format!("Downloading ONNX Runtime... {}%", percent),
            false,
            None,
        );
    })
    .await
    .context("ONNX Runtime download failed")?;
    Ok(())
}

/// No ONNX Runtime consumer (VAD/Parakeet) is compiled in, so there is nothing
/// to fetch.
#[cfg(not(feature = "vad"))]
async fn download_ort_if_needed(_app: &tauri::AppHandle, _model: SttModel) -> anyhow::Result<()> {
    Ok(())
}

/// Fetch the ~2MB Silero VAD model. Non-fatal: RMS detection still works.
async fn download_vad_if_needed(app: &tauri::AppHandle) {
    #[cfg(feature = "full")]
    {
        if !murmur_core::audio::vad::is_downloaded() {
            emit_progress(
                app,
                0,
                "Downloading voice activity detector...",
                false,
                None,
            );
            if let Err(e) = murmur_core::audio::vad::download().await {
                tracing::warn!("Silero VAD download failed ({}); will use RMS fallback", e);
            }
        }
    }
    #[cfg(not(feature = "full"))]
    let _ = app;
}

async fn download_model_files(
    app: &tauri::AppHandle,
    model_mgr: &ModelManager,
    model: SttModel,
) -> anyhow::Result<()> {
    if model_mgr.is_downloaded(model) {
        return Ok(());
    }

    let size_mb = model.size_mb();
    emit_progress(
        app,
        0,
        &format!("Downloading {} ({} MB, one-time)...", model.name(), size_mb),
        false,
        None,
    );
    let app_ref = app.clone();
    let name = model.name().to_string();
    model_mgr
        .download_with_progress(model, move |downloaded, total| {
            let percent = as_percent(downloaded, total);
            emit_progress(
                &app_ref,
                percent,
                &format!("Downloading {} ({} MB)... {}%", name, size_mb, percent),
                false,
                None,
            );
        })
        .await
        .context("Model download failed")?;
    Ok(())
}

/// Initialize the engine on a blocking thread (CPU-intensive), then run a
/// throwaway inference so thread pools and buffers are allocated now — the
/// first real transcription is otherwise noticeably slower than steady
/// state, which reads as "dictation didn't work".
async fn init_engine(model_mgr: &ModelManager, model: SttModel) -> anyhow::Result<SttEngine> {
    let model_path = model_mgr.model_path(model);
    let path_str = model_path
        .to_str()
        .context("Invalid model path (non-UTF-8)")?
        .to_string();
    let backend = model.backend();

    tokio::task::spawn_blocking(move || {
        let mut engine = match backend {
            Backend::Whisper => SttEngine::new_whisper(&path_str, 0),
            Backend::Parakeet => SttEngine::new_parakeet(&path_str),
        }?;
        engine.set_model(model);

        let warmup = vec![0.0_f32; 8_000];
        let warm_start = Instant::now();
        if let Err(e) = engine.transcribe(&warmup) {
            tracing::debug!("Engine warm-up inference failed (non-fatal): {}", e);
        } else {
            tracing::info!("Engine warmed up in {}ms", warm_start.elapsed().as_millis());
        }

        Ok::<SttEngine, anyhow::Error>(engine)
    })
    .await
    .context("Engine init task panicked")?
    .context("Failed to initialize STT engine")
}
