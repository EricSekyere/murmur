//! Integration test for the Parakeet TDT 0.6B v3 model. Ignored by default:
//! it downloads roughly 670 MB of ONNX files on each run. Run with:
//! `cargo test -p murmur-core --features parakeet -- --ignored`
//!
//! The repo carries no speech .wav fixture, so this test does not assert a
//! non-empty transcription. It verifies that the download succeeds (every
//! file is checked against its pinned SHA256 by `ModelManager::download`),
//! that the engine constructs from the downloaded directory, and that a
//! smoke transcription over a quiet tone runs the ONNX sessions end to end.
#![cfg(feature = "parakeet")]

use murmur_core::stt::engine::SttEngine;
use murmur_core::stt::models::{ModelManager, SttModel};
use murmur_core::stt::runtime;

#[tokio::test]
#[ignore = "downloads ~670 MB of model files; run explicitly"]
async fn parakeet_v3_downloads_verifies_and_loads() {
    let dir = tempfile::tempdir().expect("tempdir");
    let manager = ModelManager::new(dir.path().to_path_buf());

    // download() bails on any SHA256 mismatch, so success proves the pins.
    let model_path = manager
        .download(SttModel::ParakeetTdt06bV3)
        .await
        .expect("model download with checksum verification");
    assert!(manager.is_downloaded(SttModel::ParakeetTdt06bV3));

    if !runtime::is_downloaded() {
        runtime::download().await.expect("ONNX Runtime download");
    }

    let path_str = model_path.to_str().expect("UTF-8 model path");
    let mut engine = SttEngine::new_parakeet(path_str).expect("engine load");

    // One second of a quiet 440 Hz tone: no speech, so the text content is
    // irrelevant; the call succeeding proves preprocessing, the encoder, and
    // the TDT decoder all accept the v3 export.
    let tone: Vec<f32> = (0..16_000)
        .map(|i| 0.01 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 16_000.0).sin())
        .collect();
    let result = engine.transcribe(&tone).expect("smoke transcription");
    println!(
        "parakeet v3 smoke transcription: {} chars in {} ms",
        result.text.chars().count(),
        result.processing_time_ms
    );
}
