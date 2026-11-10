//! Opt-in local recognition evaluation. See docs/transcription-quality.md.

use super::*;
use anyhow::{Context, Result};
use murmur_core::audio::{AudioBuffer, decode, vad::VoiceActivityDetector};
use murmur_core::command::{format_identifier, parse_case_command};
use murmur_core::dictation::{DictationConfig, DictationEvent, DictationSession};
use murmur_core::stt::{engine::SttEngine, models::SttModel};
use serde::{Deserialize, Serialize};
use std::{path::Path, time::Duration};

mod config;
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
    raw_actual: String,
    corrected_actual: String,
    raw_word_errors: usize,
    corrected_word_errors: usize,
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
    let model = config::model(&model_name)?;
    let mut engine = match model.backend() {
        murmur_core::stt::models::Backend::Parakeet => SttEngine::new_parakeet(&model_dir)?,
        murmur_core::stt::models::Backend::Whisper => SttEngine::new_whisper(&model_dir, 4)?,
    };
    engine.set_model(model);
    engine.set_language(Some("en".into()));
    let vocabulary = config::vocabulary()?;
    let conditions = config::conditions()?;
    engine.set_vocabulary(&vocabulary);
    let cases: Vec<Case> = serde_json::from_slice(&std::fs::read(&manifest)?)?;
    anyhow::ensure!(!cases.is_empty(), "manifest must contain cases");
    let parent = Path::new(&manifest).parent().context("manifest parent")?;
    let mut observations = Vec::new();
    for case in cases {
        let decoded = decode::decode(&parent.join(&case.file))?;
        let audio = AudioBuffer::from_raw(&decoded.samples, decoded.rate, decoded.channels);
        for condition in &conditions {
            let samples = perturb(&audio.samples, condition);
            observations.push(evaluate(
                &mut engine,
                &vad_path,
                &case,
                condition,
                &samples,
                &vocabulary,
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
            &vocabulary,
        )?);
    }
    // Reports can contain supplied speech; only write to the explicit local path.
    std::fs::write(
        report,
        serde_json::to_vec_pretty(&serde_json::json!({
            "model": model_name,
            "profile": "relaxed",
            "vad_threshold": 0.5,
            "vocabulary": vocabulary,
            "conditions": conditions,
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
    vocabulary: &[String],
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
    let mut raw_actual = String::new();
    let mut corrected_actual = String::new();
    let mut rejected = Vec::new();
    let mut inference_ms = 0;
    if phrases.is_empty() {
        rejected.push("vad_no_speech".into());
    }
    engine.set_initial_prompt(None);
    for audio in phrases {
        let recognized = recognize(engine, &audio.samples, vocabulary)?;
        append_phrase(&mut raw_actual, &recognized.raw);
        append_phrase(&mut corrected_actual, &recognized.corrected);
        inference_ms += recognized.inference_ms;
        match recognized.delivered {
            Ok(text) => {
                if !actual.is_empty() {
                    actual.push(' ');
                }
                actual.push_str(&text);
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
        raw_word_errors: edit_distance(&reference, &tokens(&raw_actual)),
        corrected_word_errors: edit_distance(&reference, &tokens(&corrected_actual)),
        raw_actual,
        corrected_actual,
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

struct Recognition {
    raw: String,
    corrected: String,
    delivered: Result<String, &'static str>,
    inference_ms: u64,
}

fn append_phrase(target: &mut String, phrase: &str) {
    if phrase.is_empty() {
        return;
    }
    if !target.is_empty() {
        target.push(' ');
    }
    target.push_str(phrase);
}

fn recognize(
    engine: &mut SttEngine,
    samples: &[f32],
    vocabulary: &[String],
) -> Result<Recognition> {
    let profile = TranscriptionProfile::Relaxed;
    let prepared = match prepare_audio(samples, profile) {
        Ok(prepared) => prepared,
        Err(reason) => {
            return Ok(Recognition {
                raw: String::new(),
                corrected: String::new(),
                delivered: Err(reason),
                inference_ms: 0,
            });
        }
    };
    let mut result = engine.transcribe(&prepared.samples)?;
    let raw = result.text.clone();
    result.text = murmur_core::vocab_correct::correct_counted(&result.text, vocabulary).0;
    Ok(Recognition {
        raw,
        corrected: result.text.clone(),
        delivered: finish_text(&result, profile, prepared.duration_secs, false, false, true),
        inference_ms: result.processing_time_ms,
    })
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
