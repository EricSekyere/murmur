//! Does retrying without the trailing silence recover an empty Parakeet decode?
//!
//! `transcribe.rs` drops the phrase when the engine returns nothing. NVIDIA's
//! own report (NeMo #15757) says trailing silence treated as valid audio
//! shifts the per-mel-bin normalization of the speech before it, and that the
//! fix is to retry with the tail removed. This measures whether that holds on
//! the model actually installed here.
//!
//!   MURMUR_STRESS_MODEL=.../models/parakeet-tdt-0.6b-v3 \
//!   MURMUR_LIBRISPEECH=.../LibriSpeech/train-clean-100 \
//!   cargo test -p murmur-core --features full --test parakeet_empty_retry -- --ignored --nocapture

#![cfg(feature = "parakeet")]

use murmur_core::audio::silence::compute_rms;
use std::path::{Path, PathBuf};

const RATE: usize = 16_000;
const FRAME: usize = 512;

fn utterances(root: &Path, want: usize) -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    let Ok(speakers) = std::fs::read_dir(root) else {
        return out;
    };
    let mut speakers: Vec<_> = speakers.filter_map(Result::ok).collect();
    speakers.sort_by_key(std::fs::DirEntry::path);
    for speaker in speakers {
        let Ok(chapters) = std::fs::read_dir(speaker.path()) else {
            continue;
        };
        let mut chapters: Vec<_> = chapters.filter_map(Result::ok).collect();
        chapters.sort_by_key(std::fs::DirEntry::path);
        for chapter in chapters {
            let dir = chapter.path();
            let Some(trans) = std::fs::read_dir(&dir).ok().and_then(|e| {
                e.filter_map(Result::ok)
                    .find(|f| f.file_name().to_string_lossy().ends_with(".trans.txt"))
            }) else {
                continue;
            };
            let Ok(text) = std::fs::read_to_string(trans.path()) else {
                continue;
            };
            for line in text.lines() {
                let Some((id, reference)) = line.split_once(' ') else {
                    continue;
                };
                let flac = dir.join(format!("{id}.flac"));
                if flac.exists() {
                    out.push((flac, reference.to_string()));
                }
                if out.len() >= want {
                    return out;
                }
            }
        }
    }
    out
}

/// The proposed retry: drop trailing frames that carry no speech.
fn trim_trailing(samples: &[f32]) -> &[f32] {
    let loudest = samples
        .chunks(FRAME)
        .map(compute_rms)
        .fold(0.0_f32, f32::max);
    if loudest <= 0.0 {
        return samples;
    }
    let floor = loudest * 0.02;
    let last = samples
        .chunks(FRAME)
        .rposition(|f| compute_rms(f) >= floor)
        .unwrap_or(0);
    &samples[..((last + 1) * FRAME).min(samples.len())]
}

#[test]
#[ignore]
fn retrying_without_the_tail_recovers_empty_decodes() {
    let Ok(model_dir) = std::env::var("MURMUR_STRESS_MODEL") else {
        eprintln!("set MURMUR_STRESS_MODEL");
        return;
    };
    let Ok(corpus) = std::env::var("MURMUR_LIBRISPEECH") else {
        eprintln!("set MURMUR_LIBRISPEECH");
        return;
    };
    let clips = utterances(Path::new(&corpus), 10);
    assert!(!clips.is_empty(), "no utterances under {corpus}");

    let mut engine =
        murmur_core::stt::engine::SttEngine::new_parakeet(&model_dir).expect("load parakeet");

    // Digital silence, which is what a gated driver and a zero-filled pad both
    // produce, and the condition NVIDIA's report describes.
    let tails_ms = [0usize, 200, 400, 800, 1600];
    let mut empties = 0;
    let mut recovered = 0;
    let mut still_empty = 0;

    println!(
        "\n{} utterances x {} tail lengths",
        clips.len(),
        tails_ms.len()
    );
    for (path, _) in &clips {
        let decoded = murmur_core::audio::decode::decode(path).expect("decode");
        let buffer = murmur_core::audio::AudioBuffer::from_raw(
            &decoded.samples,
            decoded.rate,
            decoded.channels,
        );
        // Short clips are where the report saw it; cap to ~2.5 s.
        let speech = &buffer.samples[..buffer.samples.len().min(RATE * 5 / 2)];

        for tail in tails_ms {
            let mut audio = speech.to_vec();
            audio.extend(std::iter::repeat_n(0.0_f32, tail * RATE / 1000));

            let first = engine.transcribe(&audio).expect("transcribe").text;
            if !first.trim().is_empty() {
                continue;
            }
            empties += 1;
            let retry = engine
                .transcribe(trim_trailing(&audio))
                .expect("retry")
                .text;
            if retry.trim().is_empty() {
                still_empty += 1;
            } else {
                recovered += 1;
                println!(
                    "  tail {tail:>4}ms: empty -> recovered {} chars",
                    retry.trim().len()
                );
            }
        }
    }

    println!("\nempty decodes: {empties}");
    println!("recovered by retry: {recovered}");
    println!("still empty after retry: {still_empty}");
    if empties == 0 {
        println!("(no empty decode reproduced at these tail lengths)");
    }
}
