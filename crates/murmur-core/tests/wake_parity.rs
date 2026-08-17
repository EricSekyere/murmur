//! The Rust scorer and the Python trainer must agree on the same audio.
//!
//! `audio/wake_onnx.rs` streams the openWakeWord chain in Rust; the shipped
//! thresholds were measured by `training/wake-word`, whose named operating
//! points sit 0.006 apart. Both release gates therefore rest on the two
//! implementations producing the same numbers, and until this test nothing
//! compared them: the only real-model test scored silence.
//!
//! The fixture is a committed 4.8 s WAV and the scores Python computed from
//! those exact bytes, both written by
//! `training/wake-word/tools/make_parity_fixture.py`. Regenerate them whenever
//! the head is retrained.
#![cfg(feature = "wake")]

use std::path::{Path, PathBuf};

use murmur_core::audio::wake::{self, WAKE_FRAME_SAMPLES, WakeScorer};
use murmur_core::audio::wake_onnx::OnnxWakeScorer;

/// Largest per-window disagreement accepted between the two implementations.
///
/// Measured, the two agree exactly: all 14 windows come out bit-identical
/// despite different ONNX Runtime builds (1.23 through the `ort` crate against
/// 1.21 in the training venv). The bound is not zero anyway, because f32
/// kernels across builds and platforms are entitled to differ in the last
/// place, which is ~1e-7 on a sigmoid output.
///
/// 1e-5 sits two orders above that noise. It is NOT comfortably inside the
/// head's decision margins, and the earlier claim that it was is wrong: the
/// nearest cached score to the shipped Medium threshold is 4.3e-6 away on the
/// positive side and 1.2e-5 on the background side, so a drift this test
/// tolerates can move one clip or one event across the line. What that cannot
/// do is change a verdict. One extra event takes the interval upper bound to
/// 0.222 against a 0.5 ceiling, and one lost clip takes the recall lower bound
/// to 0.9599 against a 0.9 floor, both far inside their gates.
///
/// Measured drift is 0.0 across every window, on two ONNX Runtime versions, so
/// the bound is slack rather than load-bearing today. Tighten it toward 1e-6
/// if that ever stops being true. Widen it only against a measured number, and
/// print the number.
const TOLERANCE: f32 = 1e-5;

/// Windows the fixture must produce, so the test cannot pass on one score.
const MIN_COMPARED_WINDOWS: usize = 10;

struct Expected {
    sample_rate: u32,
    scores: Vec<f32>,
    head_sha256: String,
    melspectrogram_sha256: String,
    embedding_sha256: String,
}

#[test]
fn rust_and_python_agree_on_the_fixture_scores() {
    let Some(paths) = present_models() else {
        eprintln!("wake models or ONNX Runtime not present; skipping");
        return;
    };
    let Some(expected) = load_expected() else {
        eprintln!("parity fixture not present; skipping");
        return;
    };
    assert_fixture_describes_these_models(&expected, &paths);

    let (sample_rate, samples) = read_pcm16_wav(&fixture_dir().join("wake_parity.wav"));
    assert_eq!(
        sample_rate, expected.sample_rate,
        "fixture WAV and fixture scores disagree on the sample rate"
    );

    let measured = collapse_repeats(&stream_scores(&paths, &samples));
    let reference = collapse_repeats(&expected.scores);
    assert_eq!(
        measured.len(),
        reference.len(),
        "Rust produced {} distinct window scores, Python {}",
        measured.len(),
        reference.len()
    );
    assert!(
        reference.len() >= MIN_COMPARED_WINDOWS,
        "fixture yields only {} windows; it cannot show agreement",
        reference.len()
    );

    let (worst, at) = worst_disagreement(&measured, &reference);
    assert!(
        worst <= TOLERANCE,
        "Rust and Python disagree by {worst:.3e} at window {at} \
         (Rust {:.9}, Python {:.9}); tolerance {TOLERANCE:.1e}",
        measured[at],
        reference[at]
    );
    eprintln!(
        "wake parity: {} windows, worst disagreement {worst:.3e} (tolerance {TOLERANCE:.1e})",
        reference.len()
    );
}

#[test]
fn the_fixture_crosses_the_shipped_decision_band() {
    let Some(expected) = load_expected() else {
        eprintln!("parity fixture not present; skipping");
        return;
    };
    // A fixture of silence would agree perfectly and prove nothing. The scores
    // have to straddle the thresholds the product actually ships with.
    let low = expected.scores.iter().copied().fold(f32::MAX, f32::min);
    let high = expected.scores.iter().copied().fold(f32::MIN, f32::max);
    assert!(low < 0.5, "fixture never scores low (minimum {low})");
    assert!(
        high >= wake::WakeSensitivity::Low.threshold(),
        "fixture never reaches the Low threshold (maximum {high})"
    );
}

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
}

fn present_models() -> Option<wake::WakeModelPaths> {
    if !murmur_core::stt::runtime::is_downloaded() {
        return None;
    }
    let paths = wake::resolved_model_paths().ok()?;
    let all = paths.melspectrogram.exists() && paths.embedding.exists() && paths.head.exists();
    all.then_some(paths)
}

fn load_expected() -> Option<Expected> {
    let raw = std::fs::read_to_string(fixture_dir().join("wake_parity_scores.json")).ok()?;
    let json: serde_json::Value = serde_json::from_str(&raw).expect("fixture scores are not JSON");
    let scores = json["scores"]
        .as_array()
        .expect("fixture has no scores array")
        .iter()
        .map(|v| v.as_f64().expect("score is not a number") as f32)
        .collect();
    Some(Expected {
        sample_rate: json["sample_rate"].as_u64().expect("sample_rate") as u32,
        scores,
        head_sha256: string_field(&json, "head_sha256"),
        melspectrogram_sha256: string_field(&json, "melspectrogram_sha256"),
        embedding_sha256: string_field(&json, "embedding_sha256"),
    })
}

fn string_field(json: &serde_json::Value, key: &str) -> String {
    json[key]
        .as_str()
        .unwrap_or_else(|| panic!("fixture is missing {key}"))
        .to_string()
}

/// A fixture generated from a different head cannot show parity for this one.
///
/// This fails rather than skipping on purpose: silently passing a comparison
/// that was never made is the failure mode the whole gate protocol exists to
/// avoid. Regenerate with `tools/make_parity_fixture.py` after retraining.
fn assert_fixture_describes_these_models(expected: &Expected, paths: &wake::WakeModelPaths) {
    for (label, path, want) in [
        ("head", &paths.head, &expected.head_sha256),
        (
            "melspectrogram",
            &paths.melspectrogram,
            &expected.melspectrogram_sha256,
        ),
        ("embedding", &paths.embedding, &expected.embedding_sha256),
    ] {
        let bytes = std::fs::read(path).expect("model file");
        let got = murmur_core::integrity::sha256_hex(&bytes);
        assert_eq!(
            &got, want,
            "the parity fixture was generated against a different {label} model \
             ({want}, this one is {got}); regenerate it with \
             training/wake-word/tools/make_parity_fixture.py"
        );
    }
}

/// Every score the streaming scorer emits, with the warm-up placeholders cut.
///
/// `OnnxWakeScorer::score` returns exactly 0.0 while fewer than 16 embeddings
/// have accumulated. The sigmoid head cannot return exactly 0.0 for a real
/// window, and the fixture's lowest genuine score is 0.0167, so the
/// placeholders are unambiguous here.
fn stream_scores(paths: &wake::WakeModelPaths, samples: &[f32]) -> Vec<f32> {
    let mut scorer = OnnxWakeScorer::new(paths).expect("wake scorer");
    samples
        .chunks_exact(WAKE_FRAME_SAMPLES)
        .map(|frame| scorer.score(frame).expect("score"))
        .filter(|score| *score != 0.0)
        .collect()
}

/// Drop consecutive repeats.
///
/// The Rust loop scores every 80 ms frame but the embedding window only
/// advances every 128 ms, so it re-scores an unchanged window and repeats the
/// bit-identical result. Python scores each window once. Collapsing both sides
/// compares windows to windows without either implementation knowing the
/// other's cadence.
fn collapse_repeats(scores: &[f32]) -> Vec<f32> {
    let mut out: Vec<f32> = Vec::with_capacity(scores.len());
    for &score in scores {
        if out.last() != Some(&score) {
            out.push(score);
        }
    }
    out
}

fn worst_disagreement(measured: &[f32], reference: &[f32]) -> (f32, usize) {
    let mut worst = 0.0_f32;
    let mut at = 0;
    for (index, (a, b)) in measured.iter().zip(reference).enumerate() {
        let diff = (a - b).abs();
        if diff > worst {
            worst = diff;
            at = index;
        }
    }
    (worst, at)
}

/// 16-bit PCM WAV to mono f32, dividing by 32768 exactly as the Python reader
/// does so both sides start from identical samples.
fn read_pcm16_wav(path: &Path) -> (u32, Vec<f32>) {
    let bytes = std::fs::read(path).expect("fixture WAV");
    assert_eq!(&bytes[0..4], b"RIFF", "not a RIFF file");
    assert_eq!(&bytes[8..12], b"WAVE", "not a WAVE file");
    let mut cursor = 12;
    let mut sample_rate = 0_u32;
    let mut channels = 0_u16;
    let mut bits = 0_u16;
    while cursor + 8 <= bytes.len() {
        let id = &bytes[cursor..cursor + 4];
        let size = u32::from_le_bytes(bytes[cursor + 4..cursor + 8].try_into().unwrap()) as usize;
        let body = &bytes[cursor + 8..(cursor + 8 + size).min(bytes.len())];
        match id {
            b"fmt " => {
                channels = u16::from_le_bytes(body[2..4].try_into().unwrap());
                sample_rate = u32::from_le_bytes(body[4..8].try_into().unwrap());
                bits = u16::from_le_bytes(body[14..16].try_into().unwrap());
            }
            b"data" => {
                assert_eq!(channels, 1, "fixture must be mono");
                assert_eq!(bits, 16, "fixture must be 16-bit PCM");
                let samples = body
                    .chunks_exact(2)
                    .map(|pair| i16::from_le_bytes(pair.try_into().unwrap()) as f32 / 32768.0)
                    .collect();
                return (sample_rate, samples);
            }
            _ => {}
        }
        cursor += 8 + size + (size % 2);
    }
    panic!("fixture WAV has no data chunk");
}
