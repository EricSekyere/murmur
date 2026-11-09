//! User-requested local exports of retained audio and reviewed transcripts.

#[cfg(feature = "full")]
use tauri::Manager;

#[cfg(feature = "full")]
fn root() -> Result<std::path::PathBuf, String> {
    let config = murmur_core::config::Settings::default_path().map_err(|e| e.to_string())?;
    let parent = config.parent().ok_or("Config directory is unavailable")?;
    Ok(parent.join("correction-examples"))
}

#[tauri::command]
pub(crate) async fn save_correction_example(
    app: tauri::AppHandle,
    entry_id: String,
    expected: String,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || save(&app, &entry_id, expected))
        .await
        .map_err(|e| format!("Saving example failed: {e}"))?
}

#[cfg(feature = "full")]
fn save(app: &tauri::AppHandle, entry_id: &str, expected: String) -> Result<String, String> {
    let state = app.state::<crate::state::AppState>();
    let recognized = state
        .history
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .entry_by_id(entry_id)
        .map(|entry| entry.text.clone())
        .ok_or("This history entry is no longer available")?;
    let (samples, rate) = {
        let retained = state
            .retained_audio
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let utterance = retained
            .get(entry_id)
            .ok_or("Audio is no longer in memory. Dictate the phrase again to save an example.")?;
        (utterance.samples.clone(), utterance.sample_rate)
    };
    let model = state
        .settings
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .model;
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).map_err(|e| format!("Cannot create example ID: {e}"))?;
    let id: String = random.iter().map(|b| format!("{b:02x}")).collect();
    let example = murmur_core::evaluation::Example {
        id,
        file: String::new(),
        expected,
        recognized,
        model_at_save: format!("{model:?}"),
    };
    murmur_core::evaluation::save(&root()?, &example, &samples, rate)
        .map(|path| path.to_string_lossy().into_owned())
        .map_err(|e| e.to_string())
}

#[cfg(not(feature = "full"))]
fn save(_app: &tauri::AppHandle, _entry_id: &str, _expected: String) -> Result<String, String> {
    Err("Correction examples require a full build".into())
}

#[tauri::command]
pub(crate) async fn list_correction_examples() -> Result<serde_json::Value, String> {
    #[cfg(feature = "full")]
    return tauri::async_runtime::spawn_blocking(|| {
        let root = root()?;
        let examples = murmur_core::evaluation::list(&root).map_err(|e| e.to_string())?;
        Ok(serde_json::json!({ "directory": root, "examples": examples }))
    })
    .await
    .map_err(|e| e.to_string())?;
    #[cfg(not(feature = "full"))]
    Err("Correction examples require a full build".into())
}

#[tauri::command]
pub(crate) async fn delete_correction_example(id: String) -> Result<(), String> {
    #[cfg(feature = "full")]
    return tauri::async_runtime::spawn_blocking(move || {
        murmur_core::evaluation::delete(&root()?, &id).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?;
    #[cfg(not(feature = "full"))]
    {
        let _ = id;
        Err("Correction examples require a full build".into())
    }
}
