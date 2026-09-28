//! Isolate preparation from microphone capture and VAD with known speech.

use super::*;
use murmur_core::audio::silence::compute_rms;

#[test]
#[ignore = "requires local Parakeet model and explicit fixture/report paths; never downloads"]
fn compare_quiet_preparation() -> Result<()> {
    let manifest = std::env::var("MURMUR_STRESS_MANIFEST").context("set MURMUR_STRESS_MANIFEST")?;
    let model = std::env::var("MURMUR_STRESS_MODEL").context("set MURMUR_STRESS_MODEL")?;
    let report = std::env::var("MURMUR_STRESS_REPORT").context("set MURMUR_STRESS_REPORT")?;
    let selection = std::env::var("MURMUR_STRESS_PREPARATION").ok();
    let paths = ["pre_hardening", "current", "untrimmed"];
    anyhow::ensure!(
        selection
            .as_ref()
            .is_none_or(|s| paths.contains(&s.as_str())),
        "MURMUR_STRESS_PREPARATION must be pre_hardening, current, or untrimmed"
    );
    let mut engine = SttEngine::new_parakeet(&model)?;
    engine.set_language(Some("en".into()));
    let cases: Vec<Case> = serde_json::from_slice(&std::fs::read(&manifest)?)?;
    anyhow::ensure!(!cases.is_empty(), "manifest must contain cases");
    let parent = Path::new(&manifest).parent().context("manifest parent")?;
    let mut observations = Vec::new();
    for case in cases
        .iter()
        .filter(|c| c.identifier.is_none() && tokens(&c.expected).len() > 2)
    {
        let decoded = decode::decode(&parent.join(&case.file))?;
        let audio = AudioBuffer::from_raw(&decoded.samples, decoded.rate, decoded.channels);
        for scale in [1.0, 0.2, 0.05, 0.02, 0.01] {
            let samples: Vec<f32> = audio.samples.iter().map(|s| s * scale).collect();
            for path in paths
                .iter()
                .copied()
                .filter(|p| selection.as_ref().is_none_or(|s| s == p))
            {
                observations.push(observe(&mut engine, case, &samples, scale, path)?);
            }
        }
    }
    anyhow::ensure!(!observations.is_empty(), "no matching speech cases");
    std::fs::write(report, serde_json::to_vec_pretty(&observations)?)?;
    Ok(())
}

fn observe(
    engine: &mut SttEngine,
    case: &Case,
    samples: &[f32],
    scale: f32,
    path: &str,
) -> Result<serde_json::Value> {
    let prepared = match path {
        "pre_hardening" => pre_hardening(samples),
        "untrimmed" => Ok(normalized(samples)),
        _ => prepare_audio(samples, TranscriptionProfile::Relaxed).map(|p| p.samples),
    };
    let (actual, rejection, prepared_samples) = match prepared {
        Ok(audio) => {
            engine.set_initial_prompt(None);
            let result = engine.transcribe(&audio)?;
            (result.text, None, audio.len())
        }
        Err(reason) => (String::new(), Some(reason), 0),
    };
    let expected = tokens(&case.expected);
    Ok(serde_json::json!({
        "id": case.id, "scale": scale, "path": path,
        "raw_peak": peak(samples), "raw_rms": compute_rms(samples),
        "raw_samples": samples.len(), "prepared_samples": prepared_samples,
        "expected": case.expected, "actual": actual, "rejection": rejection,
        "word_errors": edit_distance(&expected, &tokens(&actual)),
        "reference_words": expected.len(),
    }))
}

fn peak(samples: &[f32]) -> f32 {
    samples.iter().map(|s| s.abs()).fold(0.0, f32::max)
}

fn normalized(samples: &[f32]) -> Vec<f32> {
    let peak = peak(samples);
    let gain = if peak > 0.0 && peak < 0.1 {
        (0.5 / peak).min(5.0)
    } else {
        1.0
    };
    samples
        .iter()
        .map(|s| (s * gain).clamp(-1.0, 1.0))
        .collect()
}

// Frozen relaxed preparation from before 7895eb7, for comparison only.
fn pre_hardening(samples: &[f32]) -> Result<Vec<f32>, &'static str> {
    if samples.len() < 1_920 {
        return Err("too_short_raw");
    }
    let frames: Vec<f32> = samples.chunks(512).map(compute_rms).collect();
    let first = frames.iter().position(|&rms| rms >= 0.003).unwrap_or(0);
    let last = frames
        .iter()
        .rposition(|&rms| rms >= 0.003)
        .unwrap_or(frames.len().saturating_sub(1));
    let start = first.saturating_sub(2) * 512;
    let end = ((last + 1) * 512).min(samples.len());
    let trimmed = &samples[start..end];
    if trimmed.len() < 1_920 {
        return Err("too_short_trimmed");
    }
    let audio = normalized(trimmed);
    if peak(&audio) < 0.008 || compute_rms(&audio) < 0.0008 {
        return Err("too_quiet");
    }
    Ok(audio)
}
