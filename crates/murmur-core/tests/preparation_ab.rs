//! Pre-#135 preparation versus current preparation, through the real model.
//!
//! #135 rewrote how a phrase is trimmed, normalized and gated before it
//! reaches the engine. This decodes the same speech both ways and scores the
//! difference, so the question is answered by word errors rather than by
//! reading the diff.
//!
//! Run with the installed model:
//!   MURMUR_STRESS_MODEL=.../models/parakeet-tdt-0.6b-v3 \
//!   MURMUR_LIBRISPEECH=.../LibriSpeech/train-clean-100 \
//!   cargo test -p murmur-core --features full --test parakeet_padding -- --ignored --nocapture

#![cfg(feature = "parakeet")]

use murmur_core::audio::silence::compute_rms;
use std::path::{Path, PathBuf};

const RATE: usize = 16_000;
const FRAME: usize = 512;

/// Relaxed profile, the setting in the field config.
const TRIM_THRESHOLD: f32 = 0.003;
const MIN_PEAK: f32 = 0.008;
const MIN_RMS: f32 = 0.0008;

fn noise(len: usize, level: f32, seed: &mut u32) -> Vec<f32> {
    (0..len)
        .map(|_| {
            *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            ((*seed >> 8) as f32 / 8_388_608.0 - 1.0) * level
        })
        .collect()
}

fn normalize_peak(samples: &[f32], peak: f32) -> Vec<f32> {
    if peak >= 0.1 || peak <= 0.0 {
        return samples.to_vec();
    }
    let scale = (0.5 / peak).min(5.0);
    samples
        .iter()
        .map(|s| (s * scale).clamp(-1.0, 1.0))
        .collect()
}

/// Exactly the code #135 replaced: 2 frames of lead, no trailing pad, and the
/// signal gate applied to the normalized audio.
fn prepare_pre135(samples: &[f32]) -> Result<Vec<f32>, &'static str> {
    if samples.len() < FRAME {
        return Err("too_short_raw");
    }
    let frames: Vec<f32> = samples.chunks(FRAME).map(compute_rms).collect();
    let first = frames
        .iter()
        .position(|&r| r >= TRIM_THRESHOLD)
        .unwrap_or(0);
    let last = frames
        .iter()
        .rposition(|&r| r >= TRIM_THRESHOLD)
        .unwrap_or(frames.len().saturating_sub(1));
    let start = first.saturating_sub(2) * FRAME;
    let end = ((last + 1) * FRAME).min(samples.len());
    if start >= end {
        return Err("too_short_trimmed");
    }
    let trimmed = &samples[start..end];

    let peak = trimmed.iter().map(|s| s.abs()).fold(0.0_f32, f32::max);
    let out = normalize_peak(trimmed, peak);
    // The gate ran on the normalized signal, so the boost could rescue it.
    let npeak = out.iter().map(|s| s.abs()).fold(0.0_f32, f32::max);
    let nrms = compute_rms(&out);
    if npeak < MIN_PEAK || nrms < MIN_RMS {
        return Err("too_quiet");
    }
    Ok(out)
}

/// Current: relative trim floor, 5 frames of context each side, a minimum
/// audible-frame count, and the gate applied before normalization.
fn prepare_current(samples: &[f32]) -> Result<Vec<f32>, &'static str> {
    if samples.is_empty() {
        return Err("empty_audio");
    }
    let loudest = samples
        .chunks(FRAME)
        .map(compute_rms)
        .fold(0.0_f32, f32::max);
    if loudest == 0.0 {
        return Err("too_short_trimmed");
    }
    let threshold = TRIM_THRESHOLD.min(loudest * 0.1);
    let audible: Vec<usize> = samples
        .chunks(FRAME)
        .enumerate()
        .filter_map(|(i, f)| (compute_rms(f) >= threshold).then_some(i))
        .collect();
    if audible.len() < 2 {
        return Err("too_short_trimmed");
    }
    let start = audible[0].saturating_sub(5) * FRAME;
    let end = ((audible[audible.len() - 1] + 1 + 5) * FRAME).min(samples.len());
    let trimmed = &samples[start..end];

    // Gate on the raw signal, before any boost.
    let peak = trimmed.iter().map(|s| s.abs()).fold(0.0_f32, f32::max);
    let rms = compute_rms(trimmed);
    if peak < MIN_PEAK || rms < MIN_RMS {
        return Err("too_quiet");
    }
    Ok(normalize_peak(trimmed, peak))
}

fn normalize_text(text: &str) -> Vec<String> {
    text.to_lowercase()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == ' ' {
                c
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

fn word_errors(reference: &[String], hypothesis: &[String]) -> usize {
    let (n, m) = (reference.len(), hypothesis.len());
    let mut prev: Vec<usize> = (0..=m).collect();
    let mut cur = vec![0usize; m + 1];
    for i in 1..=n {
        cur[0] = i;
        for j in 1..=m {
            let sub = prev[j - 1] + usize::from(reference[i - 1] != hypothesis[j - 1]);
            cur[j] = sub.min(prev[j] + 1).min(cur[j - 1] + 1);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[m]
}

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

#[test]
#[ignore]
fn pre_135_preparation_versus_current() {
    let Ok(model_dir) = std::env::var("MURMUR_STRESS_MODEL") else {
        eprintln!("set MURMUR_STRESS_MODEL");
        return;
    };
    let Ok(corpus) = std::env::var("MURMUR_LIBRISPEECH") else {
        eprintln!("set MURMUR_LIBRISPEECH");
        return;
    };
    let clips = utterances(Path::new(&corpus), 20);
    assert!(!clips.is_empty(), "no utterances under {corpus}");

    let mut engine =
        murmur_core::stt::engine::SttEngine::new_parakeet(&model_dir).expect("load parakeet");

    // Speech RMS targets. 0.030 is the field median; the lower ones are the
    // soft-speech end where the two paths are expected to diverge.
    let levels = [0.060_f32, 0.030, 0.012, 0.005];
    println!(
        "\n{} utterances per level, room tone 5% of speech level",
        clips.len()
    );
    println!(
        "{:>8} {:>10} {:>9} {:>8} {:>9} {:>8}",
        "speech", "pre135 WER", "pre135 rej", "cur WER", "cur rej", "delta"
    );

    for level in levels {
        let mut agg = [(0usize, 0usize, 0usize); 2];
        for (path, reference) in &clips {
            let decoded = murmur_core::audio::decode::decode(path).expect("decode");
            let buffer = murmur_core::audio::AudioBuffer::from_raw(
                &decoded.samples,
                decoded.rate,
                decoded.channels,
            );
            let raw_rms = compute_rms(&buffer.samples).max(1e-9);
            let gain = level / raw_rms;
            let mut seed = 11u32;
            let floor = noise(buffer.samples.len() + 2 * RATE / 2, level * 0.05, &mut seed);

            // Half a second of room tone each side, as a real phrase carries.
            let mut audio = Vec::with_capacity(floor.len());
            audio.extend_from_slice(&floor[..RATE / 4]);
            audio.extend(buffer.samples.iter().map(|s| s * gain));
            audio.extend_from_slice(&floor[..RATE / 4]);
            for (i, s) in audio.iter_mut().enumerate() {
                *s += floor[i.min(floor.len() - 1)];
            }

            let reference = normalize_text(reference);
            for (slot, prepared) in [prepare_pre135(&audio), prepare_current(&audio)]
                .into_iter()
                .enumerate()
            {
                match prepared {
                    Ok(samples) => {
                        let text = engine.transcribe(&samples).expect("transcribe").text;
                        agg[slot].0 += word_errors(&reference, &normalize_text(&text));
                    }
                    Err(_) => {
                        agg[slot].0 += reference.len();
                        agg[slot].2 += 1;
                    }
                }
                agg[slot].1 += reference.len();
            }
        }
        let wer = |a: (usize, usize, usize)| 100.0 * a.0 as f32 / a.1.max(1) as f32;
        println!(
            "{:>8.3} {:>9.2}% {:>9} {:>7.2}% {:>8} {:>+7.2}",
            level,
            wer(agg[0]),
            agg[0].2,
            wer(agg[1]),
            agg[1].2,
            wer(agg[1]) - wer(agg[0]),
        );
    }
}
