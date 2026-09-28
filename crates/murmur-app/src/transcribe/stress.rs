//! Opt-in local recognition evaluation. See docs/transcription-quality.md.

use super::*;
use anyhow::{Context, Result};
use murmur_core::audio::{AudioBuffer, decode, vad::VoiceActivityDetector};
use murmur_core::command::{format_identifier, parse_case_command};
use murmur_core::dictation::{DictationConfig, DictationEvent, DictationSession};
use murmur_core::stt::{engine::SttEngine, models::SttModel};
use serde::{Deserialize, Serialize};
use std::{path::Path, time::Duration};

mod quiet;

#[derive(Deserialize)]
struct Case {
    id: String,
    file: String,
    expected: String,
    #[serde(default)]
    identifier: Option<String>,
}

#[derive(Serialize)]
struct Observation {
    id: String,
    condition: String,
    expected: String,
    actual: String,
    word_errors: usize,
    reference_words: usize,
    identifier: Option<String>,
    identifier_correct: Option<bool>,
    rejected: Vec<String>,
    inference_ms: u64,
}

#[test]
#[ignore = "requires local models and explicit fixture/report paths; never downloads"]
fn stress_local_transcription() -> Result<()> {
    let manifest = std::env::var("MURMUR_STRESS_MANIFEST").context("set MURMUR_STRESS_MANIFEST")?;
    let model_dir = std::env::var("MURMUR_STRESS_MODEL").context("set MURMUR_STRESS_MODEL")?;
    let vad_path = std::env::var("MURMUR_STRESS_VAD").context("set MURMUR_STRESS_VAD")?;
    let report = std::env::var("MURMUR_STRESS_REPORT").context("set MURMUR_STRESS_REPORT")?;
    let model_name =
        std::env::var("MURMUR_STRESS_BACKEND").unwrap_or_else(|_| "parakeet-v3".into());
    let mut engine = match model_name.as_str() {
        "parakeet-v2" | "parakeet-v3" => SttEngine::new_parakeet(&model_dir)?,
        "whisper-small" => SttEngine::new_whisper(&model_dir, 4)?,
        _ => anyhow::bail!("backend must be parakeet-v2, parakeet-v3, or whisper-small"),
    };
    engine.set_model(match model_name.as_str() {
        "parakeet-v2" => SttModel::ParakeetTdt06bV2,
        "parakeet-v3" => SttModel::ParakeetTdt06bV3,
        _ => SttModel::WhisperSmallEn,
    });
    engine.set_language(Some("en".into()));
    engine.set_vocabulary(&["Claude".into(), "Fresha".into()]);
    let cases: Vec<Case> = serde_json::from_slice(&std::fs::read(&manifest)?)?;
    anyhow::ensure!(!cases.is_empty(), "manifest must contain cases");
    let parent = Path::new(&manifest).parent().context("manifest parent")?;
    let mut observations = Vec::new();
    for case in cases {
        let decoded = decode::decode(&parent.join(&case.file))?;
        let audio = AudioBuffer::from_raw(&decoded.samples, decoded.rate, decoded.channels);
        for condition in [
            "clean",
            "quiet",
            "amplitude_5_percent",
            "amplitude_2_percent",
            "amplitude_1_percent",
            "noise",
            "padded",
        ] {
            let samples = perturb(&audio.samples, condition);
            observations.push(evaluate(
                &mut engine,
                &vad_path,
                &case,
                condition,
                &samples,
            )?);
        }
    }
    for (id, samples) in non_speech() {
        let case = Case {
            id: id.into(),
            file: String::new(),
            expected: String::new(),
            identifier: None,
        };
        observations.push(evaluate(
            &mut engine,
            &vad_path,
            &case,
            "non_speech",
            &samples,
        )?);
    }
    // Reports can contain supplied speech; only write to the explicit local path.
    std::fs::write(
        report,
        serde_json::to_vec_pretty(&serde_json::json!({
            "model": model_name,
            "profile": "relaxed",
            "vad_threshold": 0.5,
            "observations": observations,
        }))?,
    )?;
    Ok(())
}

fn evaluate(
    engine: &mut SttEngine,
    vad_path: &str,
    case: &Case,
    condition: &str,
    samples: &[f32],
) -> Result<Observation> {
    let vad = VoiceActivityDetector::new(vad_path, 0.5)?;
    let config = DictationConfig {
        silence_hold: Duration::from_millis(600),
        ..DictationConfig::default()
    };
    let mut session = DictationSession::new(config, 16_000).with_vad(vad);
    let mut phrases = Vec::new();
    for chunk in samples.chunks(800) {
        for event in session.ingest(chunk) {
            if let DictationEvent::PhraseReady(audio) = event {
                phrases.push(audio);
            }
        }
    }
    if let Some(audio) = session.finish() {
        phrases.push(audio);
    }
    let mut actual = String::new();
    let mut rejected = Vec::new();
    let mut inference_ms = 0;
    if phrases.is_empty() {
        rejected.push("vad_no_speech".into());
    }
    engine.set_initial_prompt(None);
    for audio in phrases {
        match recognize(engine, &audio.samples)? {
            Ok((text, ms)) => {
                if !actual.is_empty() {
                    actual.push(' ');
                }
                actual.push_str(&text);
                inference_ms += ms;
                engine.set_initial_prompt(Some(actual.clone()));
            }
            Err(reason) => {
                rejected.push(reason.into());
                engine.set_initial_prompt(None);
            }
        }
    }
    let identifier =
        parse_case_command(&actual).map(|(style, words)| format_identifier(&words, style));
    let reference = tokens(&case.expected);
    Ok(Observation {
        id: case.id.clone(),
        condition: condition.into(),
        expected: case.expected.clone(),
        word_errors: edit_distance(&reference, &tokens(&actual)),
        reference_words: reference.len(),
        identifier_correct: case
            .identifier
            .as_ref()
            .map(|expected| identifier.as_ref() == Some(expected)),
        actual,
        identifier,
        rejected,
        inference_ms,
    })
}

fn recognize(
    engine: &mut SttEngine,
    samples: &[f32],
) -> Result<Result<(String, u64), &'static str>> {
    let profile = TranscriptionProfile::Relaxed;
    let prepared = match prepare_audio(samples, profile) {
        Ok(prepared) => prepared,
        Err(reason) => return Ok(Err(reason)),
    };
    let mut result = engine.transcribe(&prepared.samples)?;
    result.text = murmur_core::vocab_correct::correct_counted(
        &result.text,
        &["Claude".into(), "Fresha".into()],
    )
    .0;
    Ok(
        finish_text(&result, profile, prepared.duration_secs, false, false, true)
            .map(|text| (text, result.processing_time_ms)),
    )
}

fn perturb(samples: &[f32], condition: &str) -> Vec<f32> {
    let mut random = 42_u32;
    let mut out: Vec<f32> = samples
        .iter()
        .map(|&s| match condition {
            "quiet" => s * 0.2,
            "amplitude_5_percent" => s * 0.05,
            "amplitude_2_percent" => s * 0.02,
            "amplitude_1_percent" => s * 0.01,
            "noise" => (s + noise(&mut random) * 0.025).clamp(-1.0, 1.0),
            _ => s,
        })
        .collect();
    if condition == "padded" {
        let mut padded = vec![0.0; 16_000];
        padded.append(&mut out);
        padded.resize(padded.len() + 32_000, 0.0);
        return padded;
    }
    out
}

fn noise(state: &mut u32) -> f32 {
    *state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
    (*state >> 8) as f32 / 8_388_608.0 - 1.0
}

fn non_speech() -> Vec<(&'static str, Vec<f32>)> {
    let mut random = 42;
    let mut cases = vec![("silence", vec![0.0; 48_000])];
    for (name, amplitude) in [("quiet_noise", 0.002), ("noise", 0.03), ("loud_noise", 0.2)] {
        cases.push((
            name,
            (0..48_000)
                .map(|_| noise(&mut random) * amplitude)
                .collect(),
        ));
    }
    for (name, hz) in [("hum", 60.0), ("tone", 440.0)] {
        cases.push((
            name,
            (0..48_000)
                .map(|i| 0.03 * (std::f32::consts::TAU * hz * i as f32 / 16_000.0).sin())
                .collect(),
        ));
    }
    let mut clicks = vec![0.0; 48_000];
    for i in (0..48_000).step_by(1_600) {
        clicks[i] = 0.8;
    }
    cases.push(("clicks", clicks));
    cases
}

fn tokens(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(str::to_lowercase)
        .collect()
}

fn edit_distance(reference: &[String], actual: &[String]) -> usize {
    let mut row: Vec<usize> = (0..=actual.len()).collect();
    for (i, expected) in reference.iter().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, received) in actual.iter().enumerate() {
            let above = row[j + 1];
            row[j + 1] = (diagonal + usize::from(expected != received))
                .min(above + 1)
                .min(row[j] + 1);
            diagonal = above;
        }
    }
    row[actual.len()]
}

#[test]
fn scoring_counts_insertions_deletions_and_substitutions() {
    assert_eq!(
        edit_distance(&tokens("one two three"), &tokens("one four")),
        2
    );
    assert_eq!(edit_distance(&[], &tokens("unexpected words")), 2);
    assert_eq!(
        edit_distance(&tokens("Hello, WORLD!"), &tokens("hello world")),
        0
    );
}
