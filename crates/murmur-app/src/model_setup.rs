//! Model/runtime downloads and STT engine initialization.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::Context;
use murmur_core::stt::engine::SttEngine;
use murmur_core::stt::models::{Backend, ModelManager, SttModel};
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

    emit_progress(app, 100, "Loading model...", false, None);
    let stt = init_engine(&model_mgr, model).await?;

    {
        let mut guard = engine.lock().unwrap_or_else(|e| e.into_inner());
        *guard = Some(stt);
    }
    if let Some(app_state) = app.try_state::<AppState>() {
        app_state
            .engine_loaded
            .store(true, std::sync::atomic::Ordering::Release);
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

    emit_progress(
        app,
        0,
        &format!("Downloading {}...", model.name()),
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
                &format!("Downloading {}... {}%", name, percent),
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
