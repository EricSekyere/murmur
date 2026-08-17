//! About surface: version/build identity, curated external links, and a
//! copyable diagnostics report for bug reports.

use tauri::State;

use crate::native_actions::{NativeActions, SystemActions};
use crate::state::AppState;

/// Only these exact URLs can ever be opened from the About UI; the frontend
/// passes an id, never a URL, so a compromised renderer cannot launch
/// arbitrary targets through the shell.
const ABOUT_LINKS: &[(&str, &str)] = &[
    ("repo", "https://github.com/EricSekyere/murmur"),
    ("issues", "https://github.com/EricSekyere/murmur/issues"),
    (
        "license",
        "https://github.com/EricSekyere/murmur/blob/main/LICENSE",
    ),
    (
        "privacy",
        "https://github.com/EricSekyere/murmur/blob/main/PRIVACY.md",
    ),
    (
        "terms",
        "https://github.com/EricSekyere/murmur/blob/main/TERMS.md",
    ),
    (
        "model-parakeet-v2",
        "https://huggingface.co/nvidia/parakeet-tdt-0.6b-v2",
    ),
    (
        "model-parakeet-v3",
        "https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3",
    ),
    ("model-whisper", "https://github.com/openai/whisper"),
    ("model-silero", "https://github.com/snakers4/silero-vad"),
    (
        "model-sortformer",
        "https://huggingface.co/nvidia/diar_streaming_sortformer_4spk-v2",
    ),
    ("model-bge", "https://huggingface.co/BAAI/bge-small-en-v1.5"),
    ("model-qwen", "https://huggingface.co/Qwen/Qwen3-1.7B"),
];

fn link_url(id: &str) -> Option<&'static str> {
    ABOUT_LINKS
        .iter()
        .find(|(key, _)| *key == id)
        .map(|(_, url)| *url)
}

/// The whisper GPU backend this binary was compiled with.
fn gpu_backend() -> &'static str {
    if cfg!(feature = "vulkan") {
        "vulkan"
    } else if cfg!(feature = "cuda") {
        "cuda"
    } else {
        "none"
    }
}

/// Version and build identity for the About section. The version comes from
/// the runtime package info, which release CI stamps from the git tag before
/// building; in a dev build it is the workspace placeholder and the UI
/// already shows a "dev build" badge next to it.
#[tauri::command]
pub(crate) fn about_info(app: tauri::AppHandle) -> serde_json::Value {
    let config_dir = murmur_core::config::Settings::default_path()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_string_lossy().into_owned()));
    serde_json::json!({
        "version": app.package_info().version.to_string(),
        "tauri_version": tauri::VERSION,
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "debug_build": cfg!(debug_assertions),
        "gpu_backend": gpu_backend(),
        "config_dir": config_dir,
    })
}

/// A local build carries the `.dev` identifier so it can run beside an install.
pub(crate) fn is_dev_build(app: &tauri::AppHandle) -> bool {
    app.config().identifier.ends_with(".dev")
}

pub(crate) fn display_name(app: &tauri::AppHandle) -> &'static str {
    if is_dev_build(app) {
        "Murmur Dev"
    } else {
        "Murmur"
    }
}

/// Open one of the curated About links in the system browser.
#[tauri::command]
pub(crate) fn open_about_link(id: String) -> Result<(), String> {
    let url = link_url(&id).ok_or_else(|| format!("unknown link id '{id}'"))?;
    SystemActions.launch(url).map_err(|e| e.to_string())
}

/// A plaintext report the user can paste into a bug report: build identity
/// and non-sensitive configuration. Never includes transcripts, history, or
/// vocabulary contents.
#[tauri::command]
pub(crate) fn diagnostics_report(app: tauri::AppHandle, state: State<'_, AppState>) -> String {
    let settings = state
        .settings
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let model_ready = state
        .engine_loaded
        .load(std::sync::atomic::Ordering::Acquire);
    let build_kind = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    format!(
        "Murmur v{version} ({build_kind})\n\
         OS: {os} {arch}\n\
         Tauri: {tauri}\n\
         GPU backend: {gpu}\n\
         Model: {model} (ready: {model_ready})\n\
         Output mode: {output:?}\n\
         Activation: {activation} (key: {key})\n\
         Live preview: {preview}, caption: {caption}\n\
         Features: vad={vad} diarization={diar} llm={llm}\n",
        version = app.package_info().version,
        os = std::env::consts::OS,
        arch = std::env::consts::ARCH,
        tauri = tauri::VERSION,
        gpu = gpu_backend(),
        model = settings.model.id(),
        output = settings.output_mode,
        activation = settings.activation_mode,
        key = settings.double_tap_key,
        preview = settings.live_preview,
        caption = settings.caption_position,
        vad = cfg!(feature = "vad"),
        diar = cfg!(feature = "diarization"),
        llm = cfg!(feature = "llm"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_link_ids_resolve_to_https_urls() {
        for (id, _) in ABOUT_LINKS {
            let url = link_url(id).expect("id resolves");
            assert!(url.starts_with("https://"), "{id} must be https");
        }
    }

    #[test]
    fn unknown_link_ids_are_rejected() {
        assert_eq!(link_url("file:///etc/passwd"), None);
        assert_eq!(link_url(""), None);
        assert_eq!(link_url("REPO"), None);
    }
}
