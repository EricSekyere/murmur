//! Phrase transcription: preprocessing, quality gates, and hallucination
//! filtering between the audio worker and the STT engine.

use std::collections::HashSet;

use murmur_core::config::TranscriptionProfile;
use murmur_core::stt::engine::{Segment, TranscriptionResult};
use murmur_core::stt::postprocess::PostProcessor;
use tauri::Manager;

use crate::state::{AppState, emit_transcription_diagnostic, emit_transcription_error};

/// Whisper hallucination phrases produced on silence/noise.
const HALLUCINATIONS: &[&str] = &[
    "thank you",
    "thank you for watching",
    "thanks for watching",
    "thanks for listening",
    "please subscribe",
    "like and subscribe",
    "see you next time",
    "see you in the next video",
    "subtitles by",
    "subtitle",
    "share this video",
    "don't forget to subscribe",
    "the end",
];
const STRICT_EXTRA_HALLUCINATIONS: &[&str] = &["bye", "goodbye", "you", "so"];
/// Breath, sigh, and bare-filler artifacts, rejected only when they are the
/// ENTIRE phrase — "ugh, this is broken" or "okay, next step" pass untouched.
const INTERJECTIONS: &[&str] = &[
    "hmm", "hm", "mm", "mmm", "mm-hmm", "mhm", "uh", "um", "umm", "ugh", "ah", "aah", "oh", "ooh",
    "huh", "ha", "haha", "ha ha", "phew", "whew", "ahem", "heh", "pfft", "shh", "tsk", "whoo",
    "hoo", "argh", "eugh", "ew", "okay", "ok", "mkay",
];

/// 25s cap keeps inference latency bounded while leaving room for the
/// dictation splitter's worst case (~21s) inside Whisper's 30s window.
const MAX_AUDIO_SAMPLES: usize = 25 * 16_000;
const SAMPLE_RATE: f32 = 16_000.0;
const SHORT_CLIP_SECS: f32 = 1.5;

/// Whether an explicit non-English language is selected ("auto" counts as
/// possibly-English, so gates aren't relaxed when unsure).
pub(crate) fn is_non_english_language(language: &str) -> bool {
    let l = language.trim().to_lowercase();
    !l.is_empty() && l != "en" && l != "auto" && l != "english"
}

/// Append indexed codebase symbols to the user's glossary, keeping the user's
/// entries first (they win the prompt budget) and deduping case-insensitively.
fn merge_vocabulary(mut user_vocab: Vec<String>, project: &[String]) -> Vec<String> {
    if project.is_empty() {
        return user_vocab;
    }
    let mut seen: HashSet<String> = user_vocab.iter().map(|w| w.to_lowercase()).collect();
    for sym in project {
        if seen.insert(sym.to_lowercase()) {
            user_vocab.push(sym.clone());
        }
    }
    user_vocab
}

/// Per-profile rejection thresholds.
struct ProfileLimits {
    min_audio_secs: f32,
    trim_threshold: f32,
    min_peak: f32,
    min_rms: f32,
    no_speech_max: f32,
    min_conf: f32,
    short_min_conf: f32,
    short_max_no_speech: f32,
}

impl ProfileLimits {
    fn for_profile(profile: TranscriptionProfile) -> Self {
        match profile {
            TranscriptionProfile::Relaxed => Self {
                min_audio_secs: 0.12,
                trim_threshold: 0.003,
                min_peak: 0.008,
                min_rms: 0.0008,
                no_speech_max: 0.7,
                min_conf: 0.40,
                short_min_conf: 0.55,
                short_max_no_speech: 0.40,
            },
            TranscriptionProfile::Strict => Self {
                min_audio_secs: 0.15,
                trim_threshold: 0.005,
                min_peak: 0.012,
                min_rms: 0.0012,
                no_speech_max: 0.55,
                min_conf: 0.50,
                short_min_conf: 0.62,
                short_max_no_speech: 0.30,
            },
        }
    }
}

struct PreparedAudio {
    samples: Vec<f32>,
    peak: f32,
    rms: f32,
    duration_secs: f32,
}

/// Transcribe an audio buffer and return (text, processing_time_ms), or
/// None when the chunk is rejected. Infrastructure failures emit
/// `transcription-error`; benign rejections only emit diagnostics.
pub(crate) fn transcribe_chunk(
    app: &tauri::AppHandle,
    audio: &murmur_core::audio::AudioBuffer,
) -> Option<(String, u64)> {
    let state = app.state::<AppState>();
    // A matched app profile can override developer mode for this session.
    let dev_override = *state
        .session_dev_mode
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let (developer_mode, clean_speech, profile, user_vocab, language, translate) = {
        let settings = state.settings.lock().unwrap_or_else(|e| e.into_inner());
        (
            dev_override.unwrap_or(settings.developer_mode),
            settings.clean_speech,
            settings.transcription_profile,
            settings.custom_vocabulary.clone(),
            settings.language.clone(),
            settings.translate_to_english,
        )
    };
    // User glossary first (it keeps priority on the prompt budget), then the
    // indexed codebase symbols, deduped case-insensitively.
    let vocabulary = {
        let project = state
            .project_vocab
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        merge_vocabulary(user_vocab, project.as_slice())
    };
    let limits = ProfileLimits::for_profile(profile);
    // English-tuned gates over-reject accented non-English speech; relax them.
    let non_english = is_non_english_language(&language);

    let prepared = preprocess(app, audio, &limits)?;
    let result = run_engine(app, &state, &prepared, &vocabulary, &language, translate)?;

    if let Some(reason) =
        quality_reject_reason(&result, &limits, prepared.duration_secs, non_english)
    {
        return reject(app, &state, reason, &prepared, Some(&result.text));
    }

    let text = postprocess_text(&result, developer_mode, clean_speech);
    if text.is_empty() {
        emit_diag(app, "rejected", "empty_after_postprocess", &prepared);
        return None;
    }
    if let Some(reason) = hallucination_reason(&text, profile, non_english) {
        return reject(app, &state, reason, &prepared, Some(&text));
    }

    tracing::info!("Transcription accepted ({} chars)", text.chars().count());
    // Transcript only at trace, so debug-level diagnostics never log it.
    tracing::trace!("Accepted text: '{}'", text);
    emit_diag(app, "accepted", "accepted", &prepared);
    update_session_context(&state, &text);
    Some((text, result.processing_time_ms))
}

/// Validate length, trim silence, normalize, and gate on signal level.
fn preprocess(
    app: &tauri::AppHandle,
    audio: &murmur_core::audio::AudioBuffer,
    limits: &ProfileLimits,
) -> Option<PreparedAudio> {
    if audio.samples.is_empty() {
        emit_transcription_diagnostic(app, "rejected", "empty_audio", None, None, None);
        return None;
    }

    // Keep the FRONT when truncating: cutting the start loses the beginning
    // of the user's sentence on long continuous speech.
    let samples = if audio.samples.len() > MAX_AUDIO_SAMPLES {
        tracing::warn!(
            "Truncating audio from {:.1}s to 25s",
            audio.samples.len() as f32 / SAMPLE_RATE
        );
        &audio.samples[..MAX_AUDIO_SAMPLES]
    } else {
        &audio.samples
    };

    let raw_duration = samples.len() as f32 / SAMPLE_RATE;
    if raw_duration < limits.min_audio_secs {
        emit_transcription_diagnostic(
            app,
            "rejected",
            "too_short_raw",
            None,
            None,
            Some(raw_duration),
        );
        return None;
    }

    let trimmed = trim_silence(samples, limits.trim_threshold);
    let trimmed_duration = trimmed.len() as f32 / SAMPLE_RATE;
    if trimmed_duration < limits.min_audio_secs {
        emit_transcription_diagnostic(
            app,
            "rejected",
            "too_short_trimmed",
            None,
            None,
            Some(trimmed_duration),
        );
        return None;
    }

    let samples = normalize_peak(trimmed);
    let duration_secs = samples.len() as f32 / SAMPLE_RATE;
    let peak = samples.iter().map(|s| s.abs()).fold(0.0_f32, f32::max);
    let rms = {
        let sum_sq: f32 = samples.iter().map(|s| s * s).sum();
        (sum_sq / samples.len() as f32).sqrt()
    };
    tracing::info!(
        "Audio: {:.2}s raw -> {:.2}s prepared, peak={:.4}, rms={:.4}",
        raw_duration,
        duration_secs,
        peak,
        rms
    );

    let prepared = PreparedAudio {
        samples,
        peak,
        rms,
        duration_secs,
    };
    // Near-silent audio makes whisper grind on noise and hallucinate.
    if peak < limits.min_peak || rms < limits.min_rms {
        emit_diag(app, "rejected", "too_quiet", &prepared);
        return None;
    }
    Some(prepared)
}

/// Run inference with the running session transcript as decoder prompt
/// (whisper.cpp's streaming pattern for cross-phrase consistency).
fn run_engine(
    app: &tauri::AppHandle,
    state: &AppState,
    prepared: &PreparedAudio,
    vocabulary: &[String],
    language: &str,
    translate: bool,
) -> Option<TranscriptionResult> {
    let mut engine_guard = state.engine.lock().unwrap_or_else(|e| e.into_inner());
    let Some(engine) = engine_guard.as_mut() else {
        let msg = "STT engine not initialized — cannot transcribe";
        tracing::error!("{}", msg);
        emit_transcription_error(app, msg);
        emit_diag(app, "rejected", "engine_not_initialized", prepared);
        return None;
    };

    engine.set_vocabulary(vocabulary);
    engine.set_language(Some(language.to_string()));
    engine.set_translate(translate);
    let prev = state
        .session_prev_text
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    engine.set_initial_prompt((!prev.is_empty()).then_some(prev));

    tracing::info!(
        "Transcribing {:.2}s audio (model: {})",
        prepared.duration_secs,
        engine.model_path()
    );
    match engine.transcribe(&prepared.samples) {
        Ok(result) if result.text.is_empty() => {
            tracing::warn!(
                "Engine returned empty text ({}ms, peak={:.4}, rms={:.4})",
                result.processing_time_ms,
                prepared.peak,
                prepared.rms
            );
            emit_diag(app, "rejected", "engine_empty", prepared);
            None
        }
        Ok(result) => {
            tracing::info!(
                "Engine returned {} chars ({}ms, {} segments)",
                result.text.chars().count(),
                result.processing_time_ms,
                result.segments.len()
            );
            tracing::trace!("Engine text: {:?}", result.text);
            Some(result)
        }
        Err(e) => {
            let msg = format!("Transcription engine error: {:#}", e);
            tracing::error!("{}", msg);
            emit_transcription_error(app, &msg);
            emit_diag(app, "rejected", "engine_error", prepared);
            None
        }
    }
}

/// Duration-weighted mean of a per-segment metric, ignoring segments that
/// don't report it (counting them would silently dilute the average).
fn weighted_metric(segments: &[Segment], metric: impl Fn(&Segment) -> Option<f32>) -> Option<f32> {
    let values: Vec<(f32, f32)> = segments
        .iter()
        .filter_map(|s| metric(s).map(|m| (m, (s.end_cs - s.start_cs).max(0) as f32)))
        .collect();
    if values.is_empty() {
        return None;
    }

    let total: f32 = values.iter().map(|(_, w)| *w).sum();
    if total <= 0.0 {
        // Segments collapsed to zero duration (short clips near t=0). Fall
        // back to an unweighted mean so the confidence/no-speech gate is still
        // evaluated rather than silently skipped on the clips most prone to
        // hallucination.
        let mean = values.iter().map(|(m, _)| *m).sum::<f32>() / values.len() as f32;
        return Some(mean);
    }
    Some(values.iter().map(|(m, w)| m * (w / total)).sum())
}

/// Reject output the model itself isn't confident in. Sighs/breaths make
/// whisper guess: elevated no-speech probability and low token confidence,
/// almost always on short clips — so short clips get stricter limits.
fn quality_reject_reason(
    result: &TranscriptionResult,
    limits: &ProfileLimits,
    duration_secs: f32,
    non_english: bool,
) -> Option<&'static str> {
    let no_speech = weighted_metric(&result.segments, |s| s.no_speech_prob);
    let confidence = weighted_metric(&result.segments, |s| s.avg_token_prob);
    let is_short = duration_secs < SHORT_CLIP_SECS;
    // Non-English decodes run lower per-token confidence, so soften the gate.
    let conf_relax = if non_english { 0.85 } else { 1.0 };

    if let Some(p) = no_speech
        && p > limits.no_speech_max
    {
        tracing::warn!("Rejected: no_speech_prob {:.2}", p);
        return Some("no_speech_prob_high");
    }

    let conf_limit = conf_relax
        * if is_short {
            limits.short_min_conf
        } else {
            limits.min_conf
        };
    if let Some(c) = confidence
        && c < conf_limit
    {
        tracing::warn!("Rejected: decoder confidence {:.2} < {:.2}", c, conf_limit);
        return Some("low_confidence");
    }

    if is_short
        && let Some(p) = no_speech
        && p > limits.short_max_no_speech
    {
        tracing::warn!("Rejected: short clip no_speech_prob {:.2}", p);
        return Some("no_speech_short");
    }
    None
}

fn postprocess_text(
    result: &TranscriptionResult,
    developer_mode: bool,
    clean_speech: bool,
) -> String {
    if developer_mode {
        // Developer mode runs the full pipeline (symbols, tech terms, casing).
        PostProcessor::process(&result.text)
    } else if clean_speech {
        // Ordinary dictation gets prose-safe cleanup only.
        PostProcessor::process_prose(&result.text)
    } else {
        result.text.clone()
    }
}

/// Classify text-level hallucination patterns, or None for genuine speech. The
/// English word lists are skipped for non-English dictation; the structural
/// checks always apply.
/// Whether `text` is a hallucination/filler artifact rather than real speech.
/// Shared with the live preview so its caption doesn't flash fillers the
/// final-delivery path would reject.
pub(crate) fn is_hallucination_text(
    text: &str,
    profile: TranscriptionProfile,
    non_english: bool,
) -> bool {
    hallucination_reason(text, profile, non_english).is_some()
}

fn hallucination_reason(
    text: &str,
    profile: TranscriptionProfile,
    non_english: bool,
) -> Option<&'static str> {
    let normalized = text
        .trim()
        .trim_end_matches(['.', '!', '?', ','])
        .to_lowercase();
    let stripped = normalized.trim();

    let exact = !non_english
        && (HALLUCINATIONS.contains(&stripped)
            || INTERJECTIONS.contains(&stripped)
            || (matches!(profile, TranscriptionProfile::Strict)
                && STRICT_EXTRA_HALLUCINATIONS.contains(&stripped)));
    if exact {
        return Some("hallucination_exact");
    }

    // "*laughs*", "[music]", "(sighs)"
    let bracketed = (stripped.starts_with('*') && stripped.ends_with('*'))
        || (stripped.starts_with('[') && stripped.ends_with(']'))
        || (stripped.starts_with('(') && stripped.ends_with(')'));
    if bracketed {
        return Some("hallucination_bracketed");
    }

    if stripped
        .chars()
        .all(|c| c.is_ascii_punctuation() || c.is_whitespace())
    {
        return Some("hallucination_punctuation");
    }

    let words: Vec<&str> = stripped.split_whitespace().collect();
    if words.len() >= 3 && words.iter().all(|w| *w == words[0]) {
        return Some("hallucination_repeated_word");
    }

    // Repeated short phrases like "all right, all right, all right, all right"
    // are a classic whisper hallucination on silence/noise that the
    // single-word check above misses.
    if is_repetitive(&words) {
        return Some("hallucination_repetitive");
    }

    if stripped.len() == 1
        && stripped
            .chars()
            .next()
            .is_some_and(|c| !c.is_alphanumeric())
    {
        return Some("hallucination_single_char");
    }
    None
}

/// Detect text that is a short phrase repeated over and over, a classic
/// whisper hallucination on silence. Trips on either an exact 1-4 word
/// phrase repeated 3+ times, or six-plus words with very low lexical
/// diversity (each distinct word appearing 3+ times on average), which
/// catches near-repetitions. Words are compared with surrounding
/// punctuation stripped so commas in "all right, all right" don't hide it.
fn is_repetitive(words: &[&str]) -> bool {
    let clean: Vec<&str> = words
        .iter()
        .map(|w| w.trim_matches(|c: char| c.is_ascii_punctuation()))
        .filter(|w| !w.is_empty())
        .collect();
    let n = clean.len();
    if n < 4 {
        return false;
    }

    // Leading short-phrase repetition. A real hallucination is rarely a
    // perfect multiple ("yeah yeah yeah yeah no"), so allow a short trailing
    // remainder, but require an extra repeat in that case so genuine emphasis
    // ("very very very good") isn't rejected.
    for plen in 1..=(n / 2).min(4) {
        if n / plen < 3 {
            continue;
        }
        let head = &clean[..plen];
        let repeats = clean.chunks_exact(plen).take_while(|c| *c == head).count();
        let tail = n - repeats * plen;
        if repeats >= 3 && tail <= plen && (tail == 0 || repeats >= 4) {
            return true;
        }
    }

    // Low lexical diversity over a longer span.
    if n >= 6 {
        let mut distinct = clean.clone();
        distinct.sort_unstable();
        distinct.dedup();
        if distinct.len() * 3 <= n {
            return true;
        }
    }
    false
}

/// Reject a result and drop the running decoder context: a bad phrase fed
/// back as the prompt keeps inducing the same hallucination in later phrases.
fn reject(
    app: &tauri::AppHandle,
    state: &AppState,
    reason: &str,
    prepared: &PreparedAudio,
    text: Option<&str>,
) -> Option<(String, u64)> {
    tracing::warn!("Rejected phrase ({})", reason);
    tracing::trace!("Rejected text ({}): {:?}", reason, text.unwrap_or(""));
    state
        .session_prev_text
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clear();
    emit_diag(app, "rejected", reason, prepared);
    None
}

fn emit_diag(app: &tauri::AppHandle, kind: &str, reason: &str, prepared: &PreparedAudio) {
    emit_transcription_diagnostic(
        app,
        kind,
        reason,
        Some(prepared.peak),
        Some(prepared.rms),
        Some(prepared.duration_secs),
    );
}

/// Append accepted text to the running transcript used as the next prompt,
/// capped to ~200 chars (longer only burns prompt tokens).
fn update_session_context(state: &AppState, text: &str) {
    let mut prev = state
        .session_prev_text
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if !prev.is_empty() {
        prev.push(' ');
    }
    prev.push_str(text);
    // Cap by character count, not bytes, so multibyte languages get the same
    // ~200-character budget rather than being trimmed early.
    if prev.chars().count() > 200 {
        let start_byte = prev
            .char_indices()
            .rev()
            .nth(200)
            .map(|(i, _)| i)
            .unwrap_or(0);
        *prev = prev[start_byte..].trim_start().to_string();
    }
}

/// Scale very quiet audio so the engine sees usable levels; gain capped at
/// 5x to avoid amplifying the noise floor.
fn normalize_peak(samples: &[f32]) -> Vec<f32> {
    let peak = samples.iter().map(|s| s.abs()).fold(0.0_f32, f32::max);
    if peak >= 0.1 || peak <= 0.0 {
        return samples.to_vec();
    }
    let scale = (0.5 / peak).min(5.0);
    samples
        .iter()
        .map(|s| (s * scale).clamp(-1.0, 1.0))
        .collect()
}

/// Trim leading/trailing silence (dead air causes hallucinations), keeping
/// ~64ms of context before the first speech frame.
fn trim_silence(samples: &[f32], trim_threshold: f32) -> &[f32] {
    const FRAME_SIZE: usize = 512; // ~32ms at 16kHz
    const PREROLL_FRAMES: usize = 2;

    if samples.len() < FRAME_SIZE {
        return samples;
    }

    let frames: Vec<f32> = samples
        .chunks(FRAME_SIZE)
        .map(|chunk| {
            let sum_sq: f32 = chunk.iter().map(|&s| s * s).sum();
            (sum_sq / chunk.len() as f32).sqrt()
        })
        .collect();

    let first_speech = frames
        .iter()
        .position(|&rms| rms >= trim_threshold)
        .unwrap_or(0);
    let last_speech = frames
        .iter()
        .rposition(|&rms| rms >= trim_threshold)
        .unwrap_or(frames.len().saturating_sub(1));

    let start = first_speech.saturating_sub(PREROLL_FRAMES) * FRAME_SIZE;
    let end = ((last_speech + 1) * FRAME_SIZE).min(samples.len());
    if start >= end {
        return &samples[..0];
    }
    &samples[start..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_vocabulary_keeps_user_first_and_dedups() {
        let user = vec!["FooBar".to_string(), "alpha".to_string()];
        let project = vec![
            "alpha".to_string(), // dup of user (case-sensitive same)
            "ALPHA".to_string(), // case-insensitive dup
            "renderWidget".to_string(),
        ];
        let merged = merge_vocabulary(user, &project);
        assert_eq!(merged, vec!["FooBar", "alpha", "renderWidget"]);
    }

    #[test]
    fn merge_vocabulary_empty_project_returns_user() {
        let user = vec!["FooBar".to_string()];
        assert_eq!(merge_vocabulary(user.clone(), &[]), user);
    }

    #[test]
    fn interjections_rejected_only_as_whole_phrase() {
        assert!(hallucination_reason("Hmm.", TranscriptionProfile::Relaxed, false).is_some());
        assert!(
            hallucination_reason("Ugh, this is broken.", TranscriptionProfile::Relaxed, false)
                .is_none()
        );
        // Bare "okay"/"ok" are filler whisper emits on silence; filter them as
        // whole phrases but never when they lead a real sentence.
        assert!(hallucination_reason("Okay.", TranscriptionProfile::Relaxed, false).is_some());
        assert!(hallucination_reason("OK", TranscriptionProfile::Relaxed, false).is_some());
        assert!(
            hallucination_reason("Okay, next step.", TranscriptionProfile::Relaxed, false)
                .is_none()
        );
    }

    #[test]
    fn non_english_skips_english_word_lists() {
        // "you" is an English filler word, but as Spanish/French it is real
        // input ("you" -> French has no such word; treat the list as skipped).
        assert!(hallucination_reason("you", TranscriptionProfile::Strict, true).is_none());
        assert!(hallucination_reason("hmm", TranscriptionProfile::Relaxed, true).is_none());
        // Structural checks still apply regardless of language.
        assert!(hallucination_reason("(sighs)", TranscriptionProfile::Relaxed, true).is_some());
        assert!(hallucination_reason("the the the", TranscriptionProfile::Relaxed, true).is_some());
    }

    #[test]
    fn bracketed_artifacts_rejected() {
        for text in ["(sighs)", "[music]", "*laughs*"] {
            assert!(hallucination_reason(text, TranscriptionProfile::Relaxed, false).is_some());
        }
    }

    #[test]
    fn repeated_words_rejected() {
        assert!(
            hallucination_reason("the the the", TranscriptionProfile::Relaxed, false).is_some()
        );
        assert!(
            hallucination_reason("the dog barked", TranscriptionProfile::Relaxed, false).is_none()
        );
    }

    #[test]
    fn repeated_phrases_rejected() {
        // The idle-recording hallucination the single-word check misses.
        for text in [
            "all right, all right, all right, all right",
            "All right. All right. All right.",
            "you know, you know, you know",
            "I think I think I think",
            // Repetition with a non-matching trailing word.
            "yeah yeah yeah yeah no",
        ] {
            assert!(
                hallucination_reason(text, TranscriptionProfile::Relaxed, false).is_some(),
                "should reject: {text:?}"
            );
        }
    }

    #[test]
    fn real_sentences_with_some_repetition_kept() {
        for text in [
            "the cat sat on the mat",
            "no, I really can't do that today",
            "let me check the logs and get back to you",
            "very very good work on this",
        ] {
            assert!(
                hallucination_reason(text, TranscriptionProfile::Relaxed, false).is_none(),
                "should keep: {text:?}"
            );
        }
    }

    #[test]
    fn strict_profile_filters_more() {
        assert!(hallucination_reason("you", TranscriptionProfile::Strict, false).is_some());
        assert!(hallucination_reason("you", TranscriptionProfile::Relaxed, false).is_none());
    }

    #[test]
    fn trim_silence_keeps_speech_region() {
        let mut samples = vec![0.0_f32; 16_000];
        for s in &mut samples[6_000..10_000] {
            *s = 0.2;
        }
        let trimmed = trim_silence(&samples, 0.01);
        assert!(trimmed.len() < samples.len());
        assert!(trimmed.iter().any(|&s| s > 0.1));
    }

    #[test]
    fn normalize_peak_boosts_quiet_audio() {
        let samples = vec![0.02_f32; 1_000];
        let normalized = normalize_peak(&samples);
        assert!(normalized[0] > samples[0]);
    }
}
