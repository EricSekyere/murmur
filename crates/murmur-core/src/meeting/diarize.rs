//! Local speaker-diarization backend (roadmap feature 6).
//!
//! Wraps parakeet-rs's NVIDIA Sortformer v2 streaming diarizer. It runs on the
//! same ORT runtime the Parakeet STT path already initializes, so this adds no
//! new native library: parakeet-rs, ndarray, and rustfft are pure Rust over
//! `ort`. The Sortformer ONNX model is an on-demand, checksum-verified download,
//! mirroring the STT model pattern. Model weights are NVIDIA's (ONNX export by
//! the parakeet-rs author); the crate itself is MIT OR Apache-2.0.
//!
//! This is a one-shot batch diarizer, not the streaming meeting loop: build the
//! session, diarize the whole buffer, drop it. Meeting mode is not a hot path,
//! so re-loading per call keeps the code simple and the resident set small.

use std::panic::AssertUnwindSafe;
use std::path::PathBuf;

use anyhow::Context;
use futures_util::StreamExt;
use parakeet_rs::sortformer::{DiarizationConfig, Sortformer};
use sha2::{Digest, Sha256};

use super::SpeakerSegment;

/// Sortformer v2 requires 16 kHz audio; it resolves up to four speakers.
const DIAR_SAMPLE_RATE: u32 = 16_000;

/// On-demand Sortformer v2 ONNX export (NVIDIA diar_streaming_sortformer_4spk-v2),
/// hosted by the parakeet-rs author. About 469 MB.
const SORTFORMER_URL: &str = "https://huggingface.co/altunenes/parakeet-rs/resolve/main/diar_streaming_sortformer_4spk-v2.onnx";
/// Pinned SHA256 of the model above (HF LFS `oid sha256:`), verified on download.
const SORTFORMER_SHA256: &str = "cc520901a8cc25a8d7f7c2c8561a465709b67dd4f1df0572a97530087f3fbc73";
const SORTFORMER_FILENAME: &str = "diar_streaming_sortformer_4spk-v2.onnx";

/// Errors from the diarization backend.
#[derive(Debug, thiserror::Error)]
pub enum DiarizeError {
    /// Audio was not the 16 kHz Sortformer requires.
    #[error("diarization needs {expected} Hz audio, got {got} Hz")]
    UnsupportedSampleRate { expected: u32, got: u32 },
    /// The model file has not been downloaded yet.
    #[error("diarization model not found at {0}; download it first")]
    ModelMissing(PathBuf),
    /// The ORT runtime could not be initialized.
    #[error("ONNX Runtime init failed: {0}")]
    Runtime(String),
    /// The Sortformer session failed to build or run.
    #[error("Sortformer inference failed: {0}")]
    Inference(String),
    /// The ORT/ndarray inference path panicked; recovered so the app survives.
    #[error("Sortformer inference panicked: {0}")]
    Panic(String),
}

/// Directory that caches the diarization model.
pub fn model_dir() -> anyhow::Result<PathBuf> {
    let dir = dirs::data_dir()
        .ok_or_else(|| anyhow::anyhow!("Could not determine data directory"))?
        .join("murmur")
        .join("diarization");
    Ok(dir)
}

/// Filesystem path where the Sortformer model is cached.
pub fn model_path() -> anyhow::Result<PathBuf> {
    Ok(model_dir()?.join(SORTFORMER_FILENAME))
}

/// Whether the diarization model has been downloaded (present and non-empty).
pub fn is_downloaded() -> bool {
    model_path()
        .map(|p| p.metadata().map(|m| m.len() > 0).unwrap_or(false))
        .unwrap_or(false)
}

/// Download the Sortformer model once (about 469 MB), verifying its SHA256.
///
/// Streams to a temp file and atomically renames on success; a checksum
/// mismatch deletes the partial file rather than leaving a corrupt model.
/// Idempotent: returns immediately if the model is already present.
pub async fn download() -> anyhow::Result<PathBuf> {
    let dest = model_path()?;
    if is_downloaded() {
        return Ok(dest);
    }
    std::fs::create_dir_all(model_dir()?).context("create diarization model dir")?;
    let temp = dest.with_extension("partial");

    tracing::info!(
        "Downloading Sortformer diarization model (~469 MB) from {}",
        SORTFORMER_URL
    );
    let response = reqwest::Client::new()
        .get(SORTFORMER_URL)
        .send()
        .await
        .context("diarization model request failed")?
        .error_for_status()
        .context("diarization model download failed")?;

    let mut out = tokio::fs::File::create(&temp)
        .await
        .context("create temp diarization file")?;
    let mut hasher = Sha256::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("reading diarization model stream")?;
        hasher.update(&chunk);
        tokio::io::AsyncWriteExt::write_all(&mut out, &chunk)
            .await
            .context("writing diarization model")?;
    }
    tokio::io::AsyncWriteExt::flush(&mut out)
        .await
        .context("flushing diarization model")?;
    drop(out);

    let hash = format!("{:x}", hasher.finalize());
    if hash != SORTFORMER_SHA256 {
        let _ = tokio::fs::remove_file(&temp).await;
        anyhow::bail!(
            "SHA256 mismatch for diarization model: expected {}, got {}",
            SORTFORMER_SHA256,
            hash
        );
    }
    tokio::fs::rename(&temp, &dest)
        .await
        .context("finalize diarization model")?;
    tracing::info!("Diarization model ready at {}", dest.display());
    Ok(dest)
}

/// Diarize a mono PCM buffer (16 kHz `f32`) into per-speaker segments.
///
/// Returns speaker spans sorted by start time; an empty buffer yields no
/// segments without loading the model. Requires [`download`] to have run and
/// the ORT runtime DLL to be present.
pub fn diarize(samples: &[f32], sample_rate: u32) -> Result<Vec<SpeakerSegment>, DiarizeError> {
    if sample_rate != DIAR_SAMPLE_RATE {
        return Err(DiarizeError::UnsupportedSampleRate {
            expected: DIAR_SAMPLE_RATE,
            got: sample_rate,
        });
    }
    if samples.is_empty() {
        return Ok(Vec::new());
    }

    let path = model_path().map_err(|e| DiarizeError::Runtime(e.to_string()))?;
    if !path.exists() {
        return Err(DiarizeError::ModelMissing(path));
    }
    crate::stt::runtime::init_ort().map_err(|e| DiarizeError::Runtime(e.to_string()))?;

    // Reuse the shared low-memory ORT session options (CPU arena + memory
    // pattern off) so the model's activations do not stay pinned in the
    // resident set between runs, exactly as the Parakeet engine does.
    let exec = parakeet_rs::ExecutionConfig::default()
        .with_custom_configure(crate::stt::runtime::apply_low_memory);
    let mut sortformer = Sortformer::with_config(&path, Some(exec), DiarizationConfig::callhome())
        .map_err(|e| DiarizeError::Inference(e.to_string()))?;

    // Wrap inference in catch_unwind: the ORT/ndarray path can panic on
    // edge-case inputs, and that must surface as an error, not crash the app
    // (mirrors the STT engine's catch_unwind).
    let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| {
        sortformer.diarize(samples.to_vec(), sample_rate, 1)
    }));
    let segments = match outcome {
        Ok(Ok(segments)) => segments,
        Ok(Err(e)) => return Err(DiarizeError::Inference(e.to_string())),
        Err(panic) => {
            let msg = panic_message(panic.as_ref());
            tracing::error!("Sortformer inference panicked: {}", msg);
            return Err(DiarizeError::Panic(msg));
        }
    };

    tracing::info!(
        "Diarized {} samples ({:.1}s) into {} speaker segment(s)",
        samples.len(),
        samples.len() as f32 / DIAR_SAMPLE_RATE as f32,
        segments.len()
    );

    Ok(segments
        .into_iter()
        .map(|s| SpeakerSegment {
            start_secs: s.start,
            end_secs: s.end,
            speaker: s.speaker_id as u32,
        })
        .collect())
}

/// Best-effort message extraction from a caught panic payload.
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else {
        "unknown panic".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diarize_rejects_non_16khz_audio() {
        let err = diarize(&[0.0; 32], 8_000).expect_err("8 kHz must be rejected");
        assert!(matches!(
            err,
            DiarizeError::UnsupportedSampleRate {
                expected: 16_000,
                got: 8_000
            }
        ));
    }

    #[test]
    fn diarize_empty_input_returns_no_segments_without_a_model() {
        assert!(
            diarize(&[], DIAR_SAMPLE_RATE)
                .expect("empty is ok")
                .is_empty()
        );
    }

    #[test]
    fn model_path_lives_under_the_murmur_data_dir() {
        let path = model_path().expect("model path");
        assert!(path.ends_with(SORTFORMER_FILENAME));
        assert!(path.to_string_lossy().contains("diarization"));
    }

    #[test]
    fn sortformer_checksum_is_a_full_sha256() {
        assert_eq!(SORTFORMER_SHA256.len(), 64);
        assert!(SORTFORMER_SHA256.chars().all(|c| c.is_ascii_hexdigit()));
    }

    /// End-to-end smoke test. Downloads the ORT DLL and the ~469 MB Sortformer
    /// model, then runs one diarization pass on a short synthetic buffer. A pure
    /// tone has no speakers, so this asserts only that the pipeline runs without
    /// error, not a specific segmentation. Ignored by default (needs network,
    /// disk, and the ORT runtime). Run:
    ///   cargo test -p murmur-core --features diarization diarize_smoke -- --ignored --nocapture
    #[test]
    #[ignore]
    fn diarize_smoke_runs_on_synthetic_audio() {
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        if !crate::stt::runtime::is_downloaded() {
            rt.block_on(crate::stt::runtime::download())
                .expect("download ORT DLL");
        }
        if !is_downloaded() {
            rt.block_on(download()).expect("download Sortformer model");
        }

        // 4 s of a 220 Hz tone at 16 kHz mono.
        let sr = DIAR_SAMPLE_RATE;
        let samples: Vec<f32> = (0..sr as usize * 4)
            .map(|i| (i as f32 * 220.0 * std::f32::consts::TAU / sr as f32).sin() * 0.2)
            .collect();

        let segments = diarize(&samples, sr).expect("diarize runs");
        println!("diarize returned {} speaker segment(s)", segments.len());
    }
}
