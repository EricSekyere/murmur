//! Phrase transcription: preprocessing, quality gates, and hallucination
//! filtering between the audio worker and the STT engine.

use std::collections::HashSet;

use murmur_core::config::TranscriptionProfile;
use murmur_core::stt::engine::{Segment, TranscriptionResult};
use murmur_core::stt::postprocess::PostProcessor;
use tauri::Manager;

use crate::state::{AppState, emit_transcription_diagnostic, emit_transcription_error};

mod preparation;
use preparation::PreparedAudio;
pub(crate) use preparation::prepare_audio;

#[cfg(all(test, feature = "full"))]
mod stress;

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

/// The symbols an editor reported as on screen. A distinct type, not a bare
/// slice: [`assemble_vocabulary`] takes two symbol lists whose order decides
/// what survives the prompt budget, and with both as `&[String]` a transposed
/// call site would compile silently.
struct OnScreenSymbols<'a>(&'a [String]);

/// The project-wide index. See [`OnScreenSymbols`] for why this is a type.
struct ProjectIndex<'a>(&'a [String]);

/// Build the decoder glossary in priority order, most specific first.
///
/// The prompt budget is small and `cap_glossary` keeps the head, so position
/// decides what survives. The user's own list is first because it is explicit
/// intent, the editor's on-screen symbols follow because that set is small and
/// chosen by where the user actually is, and the broad project index comes
/// last so it is evicted before either. Getting this order wrong silently
/// undoes the feature, which is why it is a function with its own test rather
/// than three statements inline.
fn assemble_vocabulary(
    user_vocab: Vec<String>,
    on_screen: OnScreenSymbols<'_>,
    project: ProjectIndex<'_>,
) -> Vec<String> {
    merge_vocabulary(merge_vocabulary(user_vocab, on_screen.0), project.0)
}

/// Per-profile rejection thresholds.
struct ProfileLimits {
    no_speech_max: f32,
    min_conf: f32,
    short_min_conf: f32,
    short_max_no_speech: f32,
}

impl ProfileLimits {
    fn for_profile(profile: TranscriptionProfile) -> Self {
        match profile {
            TranscriptionProfile::Relaxed => Self {
                no_speech_max: 0.7,
                min_conf: 0.40,
                short_min_conf: 0.55,
                short_max_no_speech: 0.40,
            },
            TranscriptionProfile::Strict => Self {
                no_speech_max: 0.55,
                min_conf: 0.50,
                short_min_conf: 0.62,
                short_max_no_speech: 0.30,
            },
        }
    }
}

/// Whether a transcription belongs to the live dictation session (reads and
/// feeds the rolling decoder prompt) or is a detached re-run of retained
/// audio, which must leave all session state untouched.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum PromptContext {
    Session,
    Detached,
}

/// Transcribe an audio buffer and return (text, processing_time_ms), or
/// None when the chunk is rejected. Infrastructure failures emit
/// `transcription-error`; benign rejections only emit diagnostics.
pub(crate) fn transcribe_chunk(
    app: &tauri::AppHandle,
    audio: &murmur_core::audio::AudioBuffer,
) -> Option<(String, u64)> {
    transcribe_with(app, audio, PromptContext::Session)
}

/// Re-transcription entry point: the same engine, quality gates, vocabulary
/// correction, and post-processing as live dictation, but with no session
/// context read or written and nothing delivered anywhere.
pub(crate) fn transcribe_detached(
    app: &tauri::AppHandle,
    audio: &murmur_core::audio::AudioBuffer,
) -> Option<(String, u64)> {
    transcribe_with(app, audio, PromptContext::Detached)
}

fn transcribe_with(
    app: &tauri::AppHandle,
    audio: &murmur_core::audio::AudioBuffer,
    context: PromptContext,
) -> Option<(String, u64)> {
    let state = app.state::<AppState>();
    // A matched app profile can override developer mode for this session; a
    // detached run has no session, so it uses the global settings.
    let dev_override = match context {
        PromptContext::Session => *state
            .session_dev_mode
            .lock()
            .unwrap_or_else(|e| e.into_inner()),
        PromptContext::Detached => None,
    };
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
    let vocabulary = {
        // Copied out and the guard released before the next lock is taken.
        // Shadowing the guard would not drop it, leaving editor_context held
        // across the project_vocab acquisition for no reason.
        let on_screen: Vec<String> = {
            let guard = state
                .editor_context
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            guard.fresh().to_vec()
        };
        let project = state
            .project_vocab
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        assemble_vocabulary(
            user_vocab,
            OnScreenSymbols(&on_screen),
            ProjectIndex(project.as_slice()),
        )
    };
    // English-tuned gates over-reject accented non-English speech, so relax them
    // for non-English dictation — but only when the active model can actually
    // decode that language. English-only models are forced to "en" regardless of
    // the language setting, so their output is English and must keep the English
    // hallucination filters and confidence gate.
    let multilingual_model = state
        .engine
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .and_then(|engine| engine.model())
        .is_some_and(|model| model.is_multilingual());
    let non_english = is_non_english_language(&language) && multilingual_model;

    let prepared = preprocess(app, audio, profile)?;
    let result = run_engine(
        app,
        &state,
        &prepared,
        &vocabulary,
        &language,
        translate,
        context,
    )?;

    let text = match finish_text(
        &result,
        profile,
        prepared.duration_secs,
        non_english,
        developer_mode,
        clean_speech,
    ) {
        Ok(text) => text,
        Err(reason) => return reject(app, &state, reason, &prepared, Some(&result.text), context),
    };

    tracing::info!("Transcription accepted ({} chars)", text.chars().count());
    // Transcript only at trace, so debug-level diagnostics never log it.
    tracing::trace!("Accepted text: '{}'", text);
    emit_diag(app, "accepted", "accepted", &prepared);
    if context == PromptContext::Session {
        update_session_context(&state, &text);
    }
    Some((text, result.processing_time_ms))
}

/// Bound dictation latency, then check the signal before amplification.
fn preprocess(
    app: &tauri::AppHandle,
    audio: &murmur_core::audio::AudioBuffer,
    profile: TranscriptionProfile,
) -> Option<PreparedAudio> {
    let samples = if audio.samples.len() > MAX_AUDIO_SAMPLES {
        tracing::warn!(
            audio_secs = audio.samples.len() as f32 / SAMPLE_RATE,
            "Truncating dictation audio to 25s"
        );
        &audio.samples[..MAX_AUDIO_SAMPLES]
    } else {
        &audio.samples
    };
    match prepare_audio(samples, profile) {
        Ok(prepared) => {
            tracing::info!(
                raw_secs = samples.len() as f32 / SAMPLE_RATE,
                prepared_secs = prepared.duration_secs,
                peak = prepared.peak,
                rms = prepared.rms,
                "Prepared audio (signal levels before normalization)"
            );
            Some(prepared)
        }
        Err(reason) => {
            emit_transcription_diagnostic(app, "rejected", reason, None, None, None);
            None
        }
    }
}

/// Run inference with the running session transcript as decoder prompt
/// (whisper.cpp's streaming pattern for cross-phrase consistency).
#[allow(clippy::too_many_arguments)]
fn run_engine(
    app: &tauri::AppHandle,
    state: &AppState,
    prepared: &PreparedAudio,
    vocabulary: &[String],
    language: &str,
    translate: bool,
    context: PromptContext,
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
    // A detached re-run decodes without the live session's rolling prompt:
    // the utterance stands alone, exactly as a fresh session's first phrase.
    let prev = match context {
        PromptContext::Session => state
            .session_prev_text
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone(),
        PromptContext::Detached => String::new(),
    };
    engine.set_initial_prompt((!prev.is_empty()).then_some(prev));

    tracing::info!(
        "Transcribing {:.2}s audio (model: {})",
        prepared.duration_secs,
        engine.model_path()
    );
    let outcome = engine.transcribe(&prepared.samples);
    // An inference just ran, whatever its outcome: reset the idle-unload
    // clock so idleness is measured from the end of the last real work.
    crate::idle_unload::touch(state);
    match outcome {
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
        Ok(mut result) => {
            // Vocabulary correction runs on the output of BOTH engines:
            // whisper keeps its prompt biasing (this fixes what biasing
            // missed), and Parakeet has no biasing API at all, so this is
            // the only way the Personal Dictionary reaches it.
            let (corrected, corrections) =
                murmur_core::vocab_correct::correct_counted(&result.text, vocabulary);
            if corrections > 0 {
                // Count only: transcript content never appears above trace.
                tracing::debug!(corrections, "applied vocabulary corrections");
                result.text = corrected;
            }
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
            if e.downcast_ref::<murmur_core::stt::engine::InferencePanic>()
                .is_some()
            {
                // The panic unwound through the native whisper/ORT context,
                // which may now be corrupt: take the engine out under the
                // lock, release it, then drop outside it (teardown of a
                // wedged context can be slow), and mark it idle-unloaded so
                // the next session reloads a fresh engine through the same
                // on-demand path an idle unload uses.
                let broken = engine_guard.take();
                drop(engine_guard);
                drop(broken);
                state
                    .engine_loaded
                    .store(false, std::sync::atomic::Ordering::Release);
                state
                    .idle_unloaded
                    .store(true, std::sync::atomic::Ordering::Release);
                tracing::error!(
                    "Inference panicked; dropped the engine so the next session loads a fresh one"
                );
            }
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

pub(crate) fn preview_text(
    result: &TranscriptionResult,
    profile: TranscriptionProfile,
    duration_secs: f32,
    non_english: bool,
) -> Option<&str> {
    let limits = ProfileLimits::for_profile(profile);
    if quality_reject_reason(result, &limits, duration_secs, non_english).is_some() {
        return None;
    }
    let text = result.text.trim();
    (!text.is_empty() && !is_hallucination_text(text, profile, non_english)).then_some(text)
}

fn finish_text(
    result: &TranscriptionResult,
    profile: TranscriptionProfile,
    duration_secs: f32,
    non_english: bool,
    developer_mode: bool,
    clean_speech: bool,
) -> Result<String, &'static str> {
    let limits = ProfileLimits::for_profile(profile);
    if let Some(reason) = quality_reject_reason(result, &limits, duration_secs, non_english) {
        return Err(reason);
    }
    // Inspect recognized words before formatting: valid spoken symbols can
    // become punctuation-only or bracketed code, which is not a hallucination.
    if let Some(reason) = hallucination_reason(&result.text, profile, non_english) {
        return Err(reason);
    }
    let text = postprocess_text(result, developer_mode, clean_speech);
    if text.is_empty() {
        return Err("empty_after_postprocess");
    }
    if !developer_mode && let Some(reason) = hallucination_reason(&text, profile, non_english) {
        return Err(reason);
    }
    Ok(text)
}

fn postprocess_text(
    result: &TranscriptionResult,
    developer_mode: bool,
    clean_speech: bool,
) -> String {
    let raw = &result.text;
    // Contain any post-processing panic (e.g. a Unicode-slicing bug on a model's
    // native punctuation like "…") so a single phrase falls back to the raw
    // transcript instead of taking down the recording. The user's words land
    // either way.
    let processed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if developer_mode {
            // Developer mode runs the full pipeline (symbols, tech terms, casing).
            PostProcessor::process(raw)
        } else if clean_speech {
            // Ordinary dictation gets prose-safe cleanup only.
            PostProcessor::process_prose(raw)
        } else {
            raw.clone()
        }
    }));
    processed.unwrap_or_else(|_| {
        tracing::error!("Post-processing panicked; delivering the raw transcript");
        raw.clone()
    })
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
    context: PromptContext,
) -> Option<(String, u64)> {
    tracing::warn!("Rejected phrase ({})", reason);
    tracing::trace!("Rejected text ({}): {:?}", reason, text.unwrap_or(""));
    // A detached re-run owns no session state, so it must not clear the live
    // decoder context on rejection.
    if context == PromptContext::Session {
        state
            .session_prev_text
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Parakeet returns no segments, so the gate's limits are unreachable for
    /// the default backend. Fails if the metrics appear or the gate stops
    /// depending on them.
    #[test]
    fn the_confidence_gate_is_unreachable_without_segment_metrics() {
        let limits = ProfileLimits::for_profile(TranscriptionProfile::Strict);
        let whisper_shaped = TranscriptionResult {
            text: "invented words".into(),
            processing_time_ms: 0,
            segments: vec![Segment {
                text: "invented words".into(),
                start_cs: 0,
                end_cs: 100,
                no_speech_prob: Some(0.99),
                avg_token_prob: Some(0.01),
            }],
        };
        assert!(
            quality_reject_reason(&whisper_shaped, &limits, 1.0, false).is_some(),
            "this decode must be rejected while the metrics exist"
        );

        let parakeet_shaped = TranscriptionResult {
            segments: Vec::new(),
            ..whisper_shaped
        };
        assert_eq!(
            quality_reject_reason(&parakeet_shaped, &limits, 1.0, false),
            None,
            "identical text passes once the segments are empty"
        );
    }

    #[test]
    fn editor_context_outranks_the_project_index() {
        // Calls the function transcription calls, so swapping the merge order
        // there fails here. Performing the merges in the test body instead
        // would only prove the helper preserves order, which was never in
        // doubt, while leaving the actual decision untested.
        let user = vec!["MyTerm".to_string()];
        let on_screen = ["initializeServer".to_string(), "serverOptions".to_string()];
        let project: Vec<String> = (0..50).map(|i| format!("projectSym{i}")).collect();

        let merged = assemble_vocabulary(user, OnScreenSymbols(&on_screen), ProjectIndex(&project));

        assert_eq!(merged[0], "MyTerm", "user glossary must stay first");
        assert_eq!(
            &merged[1..3],
            &on_screen,
            "editor symbols must precede the index"
        );
        assert!(merged[3..].starts_with(&["projectSym0".to_string()]));
    }

    #[test]
    fn the_editor_set_survives_the_prompt_budget_ahead_of_the_index() {
        // What the ordering is for: the glossary is cut to a small budget from
        // the head, so a large index must not push the on-screen symbols out.
        let on_screen: Vec<String> = (0..20).map(|i| format!("onScreenSymbol{i}")).collect();
        let project: Vec<String> = (0..500).map(|i| format!("projectSymbol{i}")).collect();

        let merged = assemble_vocabulary(
            vec!["MyTerm".into()],
            OnScreenSymbols(&on_screen),
            ProjectIndex(&project),
        );
        let joined = merged.join(", ");
        let kept = &joined[..joined.len().min(400)];

        // Every on-screen symbol must survive the cut, and the index may only
        // use whatever budget is left after them. The index filling the
        // remainder is correct; the index arriving first is not.
        for i in 0..20 {
            assert!(
                kept.contains(&format!("onScreenSymbol{i}")),
                "on-screen symbol {i} was cut from the budget"
            );
        }
        let last_editor = kept
            .find("onScreenSymbol19")
            .expect("last on-screen symbol missing");
        if let Some(first_project) = kept.find("projectSymbol") {
            assert!(
                first_project > last_editor,
                "the index reached the budget ahead of the editor set"
            );
        }
    }

    #[test]
    fn a_symbol_in_both_the_editor_and_the_index_appears_once() {
        let merged = assemble_vocabulary(
            Vec::new(),
            OnScreenSymbols(&["Shared".to_string()]),
            ProjectIndex(&["shared".to_string(), "Other".to_string()]),
        );
        assert_eq!(merged, vec!["Shared", "Other"]);
    }

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
    fn spoken_symbols_survive_hallucination_checks() {
        for words in ["open paren x close paren", "open brace", "semicolon"] {
            let result = TranscriptionResult {
                text: words.into(),
                processing_time_ms: 0,
                segments: Vec::new(),
            };
            let text = finish_text(
                &result,
                TranscriptionProfile::Relaxed,
                2.0,
                false,
                true,
                false,
            )
            .expect("explicit spoken symbols are code");
            assert_eq!(text, PostProcessor::process(words));
            assert_ne!(text, words);
        }
    }

    #[test]
    fn raw_noise_annotations_remain_rejected() {
        let result = TranscriptionResult {
            text: "[music]".into(),
            processing_time_ms: 0,
            segments: Vec::new(),
        };
        assert!(
            finish_text(
                &result,
                TranscriptionProfile::Relaxed,
                2.0,
                false,
                true,
                false
            )
            .is_err()
        );
    }
    #[test]
    fn preview_rejects_low_confidence_words_that_final_delivery_rejects() {
        let mut result = TranscriptionResult {
            text: "invented words".into(),
            processing_time_ms: 0,
            segments: vec![Segment {
                text: "invented words".into(),
                start_cs: 0,
                end_cs: 100,
                no_speech_prob: Some(0.8),
                avg_token_prob: Some(0.2),
            }],
        };
        assert!(preview_text(&result, TranscriptionProfile::Relaxed, 1.0, false).is_none());
        result.segments[0].no_speech_prob = Some(0.01);
        result.segments[0].avg_token_prob = Some(0.95);
        assert_eq!(
            preview_text(&result, TranscriptionProfile::Relaxed, 1.0, false),
            Some("invented words")
        );
    }
    #[test]
    fn prose_cleanup_does_not_bypass_phrase_rejection() {
        let result = TranscriptionResult {
            text: "um thank you".into(),
            processing_time_ms: 0,
            segments: Vec::new(),
        };
        assert_eq!(
            finish_text(
                &result,
                TranscriptionProfile::Relaxed,
                2.0,
                false,
                false,
                true
            ),
            Err("hallucination_exact")
        );
    }
}
