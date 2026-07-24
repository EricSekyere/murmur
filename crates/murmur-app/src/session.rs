//! Recording session lifecycle: toggle handling and the streaming worker
//! that turns detected phrases into delivered text.

use std::time::{Duration, Instant};

use murmur_core::config::settings::AutoSubmit;
use murmur_core::output::OutputMode;
use murmur_core::voice_commands::{self, VoiceCommand};
use tauri::{Emitter, Manager};
use unicode_segmentation::UnicodeSegmentation;

use crate::audio_worker::{AudioResult, StartParams, panic_message};
use crate::state::{
    AppState, emit_hotkey_error, emit_recording_state, emit_transcription_diagnostic,
};
use crate::transcribe::transcribe_chunk;

/// Handle a recording toggle from any input source (hotkey, UI, double-tap,
/// click-to-stop), debounced so simultaneous sources fire once.
pub(crate) fn handle_toggle(app: &tauri::AppHandle) {
    let state = app.state::<AppState>();

    {
        let mut last = state.last_toggle.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        if now.duration_since(*last) < Duration::from_millis(500) {
            return;
        }
        *last = now;
    }

    let mut recording = state.recording.lock().unwrap_or_else(|e| e.into_inner());
    if *recording {
        *recording = false;
        drop(recording);
        stop_session(app, &state);
    } else {
        // Claim the start under the lock so two sources can never both start.
        *recording = true;
        // Bump the generation under the same lock as the flag, so a superseded
        // worker's gated release (see `release_if_current`) is ordered against
        // this start and can't clear the new session's flag.
        let generation = state
            .session_generation
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel)
            + 1;
        drop(recording);
        start_session(app, &state, generation);
    }
}

/// Push-to-talk: start recording if idle. Idempotent — a held key that
/// auto-repeats won't restart an active session.
pub(crate) fn begin_recording(app: &tauri::AppHandle) {
    let state = app.state::<AppState>();
    let generation = {
        let mut recording = state.recording.lock().unwrap_or_else(|e| e.into_inner());
        if *recording {
            return;
        }
        // Claim the start atomically so a concurrent toggle cannot also start.
        *recording = true;
        state
            .session_generation
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel)
            + 1
    };
    start_session(app, &state, generation);
}

/// Push-to-talk: stop recording if active. Idempotent.
pub(crate) fn end_recording(app: &tauri::AppHandle) {
    let state = app.state::<AppState>();
    let mut recording = state.recording.lock().unwrap_or_else(|e| e.into_inner());
    if !*recording {
        return;
    }
    *recording = false;
    drop(recording);
    stop_session(app, &state);
}

fn stop_session(app: &tauri::AppHandle, state: &AppState) {
    tracing::info!("Toggle: manual stop");
    // Reflect stopped state immediately; the streaming worker finishes
    // cleanup when it receives StreamingDone.
    emit_recording_state(app, false, false);

    let stop_result = match state.audio.get() {
        Some(audio) => audio.request_stop(),
        None => Err("Audio worker not initialized".to_string()),
    };
    if let Err(e) = stop_result {
        tracing::error!("Failed to send stop command: {}", e);
        emit_hotkey_error(app, &format!("Failed to stop recording: {}", e));
    }
}

fn start_session(app: &tauri::AppHandle, state: &AppState, generation: u64) {
    // The caller has already claimed the recording flag; release it on any
    // path that does not actually start a session.
    //
    // A live meeting owns the mic and the STT engine — dictation must wait.
    // (start_meeting checks `recording` under the same lock the callers used
    // to claim it, so the two modes can never both start.)
    if state
        .meeting_active
        .load(std::sync::atomic::Ordering::Acquire)
    {
        release_if_current(app, state, generation);
        emit_hotkey_error(
            app,
            "Stop the meeting first — dictation is paused while a meeting is recording",
        );
        return;
    }
    if !state
        .engine_loaded
        .load(std::sync::atomic::Ordering::Acquire)
    {
        // Idle-unloaded (or panic-dropped) engine: kick the reload now so the
        // "still loading" moment below ends by itself — the UX is exactly the
        // startup load (progress events, then model-changed ready). The swap
        // makes the kick one-shot even across rapid activations.
        if state
            .idle_unloaded
            .swap(false, std::sync::atomic::Ordering::AcqRel)
        {
            let model = state
                .settings
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .model;
            crate::model_setup::spawn_download_and_init(
                app.clone(),
                std::sync::Arc::clone(&state.engine),
                model,
            );
        }
        release_if_current(app, state, generation);
        emit_hotkey_error(app, "Model still loading, please wait");
        return;
    }

    #[cfg(windows)]
    crate::focus::save_output_target_window(state);

    tracing::info!("Toggle: start streaming");

    // Queue StartStreaming synchronously, before spawning the worker thread:
    // the command channel is FIFO, so a later stop toggle's Stop can never
    // overtake the start and leave the mic running while the UI shows idle.
    let params = {
        let settings = state.settings.lock().unwrap_or_else(|e| e.into_inner());
        StartParams {
            audio_device: settings.audio_device.clone(),
            rms_threshold: settings.silence_rms_threshold,
            vad_threshold: settings.vad_threshold,
            phrase_pause: Duration::from_secs_f32(settings.phrase_pause_secs),
            session_timeout: Duration::from_secs_f32(settings.session_timeout_secs),
            live_preview: settings.live_preview,
            echo_cancellation: settings.echo_cancellation,
            mic_warm_start: settings.mic_warm_start,
        }
    };
    let send_result = match state.audio.get() {
        Some(audio) => audio.send_start(params),
        None => Err("Audio worker not initialized".to_string()),
    };
    if let Err(e) = send_result {
        tracing::error!("Failed to queue start command: {}", e);
        release_if_current(app, state, generation);
        emit_hotkey_error(app, &format!("Failed to start recording: {}", e));
        return;
    }

    emit_recording_state(app, true, false);
    spawn_streaming_worker(app.clone(), state, generation);
}

/// Clear the recording flag and emit the idle state, but only if `generation`
/// is still the current session. A superseded worker (the user stopped and
/// restarted) must not stomp the live session's flag/UI. Returns whether it
/// acted, so callers can skip their own user-facing emits when stale.
fn release_if_current(app: &tauri::AppHandle, state: &AppState, generation: u64) -> bool {
    // Check the generation and clear the flag under the same lock that a start
    // uses to set it, so the decision and the write are atomic against a start.
    let mut recording = state.recording.lock().unwrap_or_else(|e| e.into_inner());
    if generation
        != state
            .session_generation
            .load(std::sync::atomic::Ordering::Acquire)
    {
        return false;
    }
    *recording = false;
    drop(recording);
    emit_recording_state(app, false, false);
    true
}

fn spawn_streaming_worker(app: tauri::AppHandle, state: &AppState, generation: u64) {
    // Serialize behind any still-finishing prior worker: two workers draining
    // the one audio result channel would split or drop phrases. The new worker
    // joins its immediate predecessor on its own thread, so the input thread
    // never blocks. Hold the slot lock across take+spawn+store so a concurrent
    // start can't lose a handle and break the chain.
    let mut slot = state
        .streaming_worker
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let prev = slot.take();
    let handle = std::thread::spawn(move || {
        if let Some(prev) = prev {
            let _ = prev.join();
        }
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            streaming_worker(&app, generation);
        }));
        if let Err(panic_info) = outcome {
            let msg = panic_message(panic_info, "unknown panic in streaming worker");
            tracing::error!("Streaming worker panicked: {}", msg);
            if let Some(state) = app.try_state::<AppState>()
                && release_if_current(&app, &state, generation)
            {
                emit_hotkey_error(&app, &format!("Recording crashed: {}", msg));
            }
        }
    });
    *slot = Some(handle);
}

/// Per-session bookkeeping used to pick the right end-of-session message.
#[derive(Default)]
struct SessionStats {
    had_transcription: bool,
    saw_signal: bool,
    had_phrase_audio: bool,
    saw_no_signal: bool,
    /// A phrase was delivered by typing or paste (not clipboard-only/stdout),
    /// so an auto-submit at session end has something to send.
    delivered_to_target: bool,
    /// The worker loop ended with StreamingDone (auto-stop or hotkey stop),
    /// not a streaming error.
    ended_normally: bool,
}

/// Session-constant delivery parameters, bundled so the phrase pipeline
/// doesn't thread each one separately.
#[derive(Clone, Copy)]
struct DeliveryContext {
    output_mode: OutputMode,
    /// Profile-configured submit keystroke fired once at session end.
    auto_submit: Option<AutoSubmit>,
    /// Caption anchor when delivered phrases show a translated caption.
    translated_caption: Option<crate::caption::CaptionAnchor>,
    /// Window dictation started in — the delivery target (Windows).
    #[cfg(windows)]
    previous_hwnd: usize,
}

/// Whether a delivery is ordinary dictation, eligible for punctuation
/// junction repair, or special output (literal escape, commit line, snippet
/// expansion, clipboard substitution) that must never be repaired and blocks
/// repairing the following phrase against it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PhraseKind {
    PlainDictation,
    Special,
}

/// Record of the previous delivery in this session, consulted before typing
/// the next phrase to decide a punctuation junction repair. Worker-local:
/// delivery is strictly sequential inside `streaming_worker`, and a fresh
/// worker per session gives session isolation for free.
struct LastDelivery {
    /// The delivered text, trimmed (the on-screen text minus the trailing
    /// auto-space). Held in memory only — never logged.
    text: String,
    delivered_at: Instant,
    /// Whether the delivery typed keystrokes (Auto/Keyboard modes).
    typed: bool,
    kind: PhraseKind,
    /// Target window the delivery went to (Windows).
    #[cfg(windows)]
    target_hwnd: usize,
}

/// Background thread: receive phrases from the audio worker, transcribe
/// each, and deliver the text to the focused application.
fn streaming_worker(app: &tauri::AppHandle, generation: u64) {
    let state = app.state::<AppState>();
    // Resolve any per-app profile for the foreground app up front, so its
    // overrides apply for the whole session.
    let target_app = current_app_name();
    let (
        output_mode,
        auto_submit,
        sound_feedback,
        live_preview,
        caption_at_window,
        show_translated_caption,
        translate_to_english,
    ) = {
        let settings = state.settings.lock().unwrap_or_else(|e| e.into_inner());
        let profile = target_app
            .as_deref()
            .and_then(|app| settings.app_profiles.iter().find(|p| p.matches(app)));
        let dev_override = profile.and_then(|p| p.developer_mode);
        *state
            .session_dev_mode
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = dev_override;
        if let Some(p) = profile {
            tracing::info!(
                "Applied app profile '{}' for {:?}",
                p.app,
                target_app.as_deref().unwrap_or("?")
            );
        }
        (
            profile
                .and_then(|p| p.output_mode)
                .unwrap_or(settings.output_mode),
            profile.and_then(|p| p.auto_submit),
            settings.sound_feedback,
            settings.live_preview,
            settings.caption_position == "window",
            settings.show_translated_caption,
            settings.translate_to_english,
        )
    };

    // Tell the widget which caption mode is active so it only grows its own
    // caption when the preview is meant to live under the pill.
    tracing::debug!(
        "Live caption position: {}",
        if caption_at_window { "window" } else { "pill" }
    );
    let _ = app.emit(
        "caption-mode",
        serde_json::json!({ "at_window": caption_at_window }),
    );

    #[cfg(windows)]
    let previous_hwnd = *state
        .previous_foreground
        .lock()
        .unwrap_or_else(|e| e.into_inner());

    // Fresh decoder context: the first phrase of this session must not be
    // biased by the last phrase of the previous one.
    state
        .session_prev_text
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clear();
    *state
        .last_delivered_len
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = 0;

    let Some(audio) = state.audio.get() else {
        tracing::error!("Audio worker not initialized in streaming_worker");
        release_if_current(app, &state, generation);
        return;
    };
    if let Err(e) = audio.await_started() {
        tracing::error!("Failed to start streaming: {}", e);
        if release_if_current(app, &state, generation) {
            emit_hotkey_error(app, &format!("Failed to start recording: {}", e));
        }
        return;
    }

    if sound_feedback {
        crate::sound::play_start();
    }

    // When the caption should roam to the active window, capture that window
    // and (best-effort) the focused input's rect so the preview worker anchors
    // the caption by the text field. A zero hwnd means no external window was
    // captured, so leave the caption under the pill.
    #[cfg(windows)]
    let caption_target =
        (caption_at_window && previous_hwnd != 0).then(|| crate::caption::CaptionAnchor {
            hwnd: previous_hwnd,
            focus: crate::caption::focused_input_rect(previous_hwnd),
        });
    #[cfg(not(windows))]
    let caption_target: Option<crate::caption::CaptionAnchor> = None;

    // Live preview re-decodes the growing phrase every ~0.7s, so it's only
    // viable on a backend fast enough to keep up. A CPU Whisper decode pays the
    // fixed ~30s mel-encoder cost per call and would run 6-7x per phrase,
    // starving the final decode (the root cause of slow Whisper dictation), so
    // skip preview for it. Parakeet and GPU-accelerated Whisper stay previewed.
    let (engine_can_preview, multilingual_model) = {
        let guard = state.engine.lock().unwrap_or_else(|e| e.into_inner());
        let engine = guard.as_ref();
        (
            engine.is_some_and(|e| e.supports_realtime_preview()),
            engine
                .and_then(|e| e.model())
                .is_some_and(|m| m.is_multilingual()),
        )
    };
    // Live translated captions: with the opt-in on and the whisper translate
    // task actually running, each delivered phrase's English rendering replaces
    // the roaming caption instead of clearing it (see deliver_text). This is
    // the only live feedback when the backend is too slow to preview.
    let translated_caption = crate::caption::translated_caption_active(
        show_translated_caption,
        translate_to_english,
        multilingual_model,
    )
    .then_some(caption_target)
    .flatten();
    // Live preview runs on its own thread so interim decodes never stall the
    // delivery of finished phrases. `None` when the feature is off or the
    // backend is too slow to preview without delaying delivery.
    let mut preview = (live_preview && engine_can_preview)
        .then(|| crate::preview::spawn(app.clone(), caption_target));

    let ctx = DeliveryContext {
        output_mode,
        auto_submit,
        translated_caption,
        #[cfg(windows)]
        previous_hwnd,
    };
    let mut stats = SessionStats::default();
    let mut last_delivery: Option<LastDelivery> = None;
    loop {
        match audio.recv_result() {
            Ok(AudioResult::PhraseReady(buffer)) => {
                stats.had_phrase_audio = true;
                handle_phrase(app, &state, &buffer, &ctx, &mut stats, &mut last_delivery);
            }
            Ok(AudioResult::PartialPhrase(buffer)) => {
                if let Some((tx, _)) = &preview {
                    let _ = tx.send(buffer);
                }
            }
            Ok(AudioResult::StreamingDone) => {
                tracing::info!("Streaming session ended");
                stats.ended_normally = true;
                break;
            }
            Ok(AudioResult::AudioLevel(rms)) => {
                let _ = app.emit("audio-level", rms);
            }
            Ok(AudioResult::SignalDetected) => {
                stats.saw_signal = true;
                let _ = app.emit("audio-signal-detected", serde_json::json!({}));
            }
            Ok(AudioResult::SpeechThreshold(threshold)) => {
                let _ = app.emit("speech-threshold", threshold);
            }
            Ok(AudioResult::NoSignal(message)) => {
                stats.saw_no_signal = true;
                emit_transcription_diagnostic(app, "rejected", "no_signal", None, None, None);
                crate::state::emit_transcription_error(app, &message);
            }
            Ok(AudioResult::Started) => {}
            Ok(AudioResult::StartFailed(e)) => {
                tracing::error!("Unexpected StartFailed during streaming: {}", e);
                break;
            }
            Err(e) => {
                tracing::error!("Streaming recv error: {}", e);
                emit_hotkey_error(app, &format!("Streaming error: {}", e));
                break;
            }
        }
    }

    // Stop the preview worker and wait for any in-flight decode to finish, so
    // a late partial can't overwrite the final text on screen.
    if let Some((tx, handle)) = preview.take() {
        drop(tx);
        let _ = handle.join();
    }

    finish_streaming(app, &state, &stats, &ctx, sound_feedback, generation);
}

/// Transcribe one phrase and deliver the result. Phrases that arrive after
/// a manual stop are still delivered — the speech already happened.
fn handle_phrase(
    app: &tauri::AppHandle,
    state: &AppState,
    buffer: &murmur_core::audio::AudioBuffer,
    ctx: &DeliveryContext,
    stats: &mut SessionStats,
    last_delivery: &mut Option<LastDelivery>,
) {
    tracing::info!(
        "Phrase ready: {} samples ({:.1}s)",
        buffer.samples.len(),
        buffer.samples.len() as f32 / 16_000.0
    );

    let still_recording = *state.recording.lock().unwrap_or_else(|e| e.into_inner());
    emit_recording_state(app, still_recording, true);

    if let Some((text, processing_time_ms)) = transcribe_chunk(app, buffer) {
        stats.had_transcription = true;
        // Display-only mode (onboarding test): show the words, deliver nothing.
        if state
            .suppress_output
            .load(std::sync::atomic::Ordering::Acquire)
        {
            // No coherent previous delivery to join onto after skipped output.
            *last_delivery = None;
            let _ = app.emit(
                "streaming-phrase",
                serde_json::json!({ "text": text, "processing_time_ms": processing_time_ms }),
            );
            crate::caption::hide(app);
            let still_recording = *state.recording.lock().unwrap_or_else(|e| e.into_inner());
            if still_recording {
                emit_recording_state(app, true, false);
            }
            return;
        }
        // Command mode: a spoken phrase here is a command, not dictation.
        // Route the transcript to the action executor instead of typing it.
        // The frontend invokes run_command and shows the physical-confirm
        // dialog for gated actions; there is no voice path to confirmation
        // (design Section 5). Command mode is its own activation channel,
        // toggled by a distinct hotkey and shown by a visible badge.
        if state
            .command_mode
            .load(std::sync::atomic::Ordering::Acquire)
        {
            // A command may edit arbitrary text; don't repair across it.
            *last_delivery = None;
            crate::caption::hide(app);
            let _ = app.emit("command-transcript", serde_json::json!({ "text": &text }));
            let still_recording = *state.recording.lock().unwrap_or_else(|e| e.into_inner());
            if still_recording {
                emit_recording_state(app, true, false);
            }
            return;
        }
        // Literal escape ("literally <command>"): deliver the words verbatim.
        let literal = {
            let settings = state.settings.lock().unwrap_or_else(|e| e.into_inner());
            voice_commands::literal_escape(&text, &settings.snippets)
        };
        if let Some(literal_text) = literal {
            deliver_text(
                app,
                state,
                &literal_text,
                ctx,
                processing_time_ms,
                PhraseKind::Special,
                stats,
                last_delivery,
            );
        } else {
            match voice_commands::parse(&text) {
                VoiceCommand::Text => {
                    // Spoken Conventional Commit ("commit feat scope core add
                    // x") delivers the formatted line as-is; snippet expansion
                    // and clipboard substitution are skipped so nothing can
                    // rewrite it. Text only — git is never run.
                    if let Some(commit_line) = murmur_core::commit::format_commit(&text) {
                        deliver_text(
                            app,
                            state,
                            &commit_line,
                            ctx,
                            processing_time_ms,
                            PhraseKind::Special,
                            stats,
                            last_delivery,
                        );
                    } else {
                        // A user snippet expands to its replacement text; otherwise
                        // the spoken phrase is delivered verbatim.
                        let (expansion, placeholders) = {
                            let settings = state.settings.lock().unwrap_or_else(|e| e.into_inner());
                            (
                                voice_commands::match_snippet(&text, &settings.snippets)
                                    .map(str::to_string),
                                settings.clipboard_placeholders.clone(),
                            )
                        };
                        let delivered = expansion.as_deref().unwrap_or(text.as_str());
                        // Spoken clipboard placeholder: splice the clipboard text
                        // into the final delivery string (a text substitution,
                        // never a paste keystroke). Runs after snippet expansion
                        // and is skipped on the literal_escape path above, which
                        // stays verbatim by design.
                        let substituted =
                            voice_commands::substitute_clipboard(delivered, &placeholders, || {
                                match murmur_core::output::clipboard::read() {
                                    Ok(clip) => Some(clip),
                                    Err(e) => {
                                        tracing::debug!(
                                            "Clipboard read for placeholder failed: {e}"
                                        );
                                        None
                                    }
                                }
                            });
                        // Snippet/clipboard rewrites are not the user's spoken
                        // sentence, so junction repair skips them; a spoken
                        // emoji substitution keeps the sentence intact.
                        let kind = if expansion.is_none() && substituted.is_none() {
                            PhraseKind::PlainDictation
                        } else {
                            PhraseKind::Special
                        };
                        let delivered = substituted.as_deref().unwrap_or(delivered);
                        // Spoken emoji ("emoji fire" -> 🔥) composes after
                        // clipboard substitution; None means no emoji spoken.
                        let with_emoji = murmur_core::emoji::substitute_emoji(delivered);
                        deliver_text(
                            app,
                            state,
                            with_emoji.as_deref().unwrap_or(delivered),
                            ctx,
                            processing_time_ms,
                            kind,
                            stats,
                            last_delivery,
                        );
                    }
                }
                command => {
                    // A command isn't phrase text, so there is nothing to caption.
                    crate::caption::hide(app);
                    execute_command(app, state, command);
                    // The command edited the target (enter, backspaces, ...):
                    // the previous delivery no longer ends at the caret.
                    *last_delivery = None;
                }
            }
        }
    } else {
        // Nothing was delivered; clear any lingering interim caption.
        crate::caption::hide(app);
    }

    let still_recording = *state.recording.lock().unwrap_or_else(|e| e.into_inner());
    if still_recording {
        emit_recording_state(app, true, false);
    }
}

/// Deliver a normal text phrase to the focused window and record how many
/// characters landed so "scratch that" can undo exactly this phrase.
#[allow(clippy::too_many_arguments)]
fn deliver_text(
    app: &tauri::AppHandle,
    state: &AppState,
    text: &str,
    ctx: &DeliveryContext,
    processing_time_ms: u64,
    kind: PhraseKind,
    stats: &mut SessionStats,
    last_delivery: &mut Option<LastDelivery>,
) {
    // A repaired junction has already backspaced the previous phrase's mark
    // and space; the phrase then lands lowercased with joining spaces.
    let repaired = attempt_junction_repair(state, text, ctx, kind, last_delivery.as_ref());
    let join = match repaired {
        Some(_) => crate::focus::TextJoin::Joining,
        None => crate::focus::TextJoin::Phrase,
    };
    let text: &str = repaired.as_deref().unwrap_or(text);

    deliver_output(app, state, text, ctx, join);

    // Focused modes append a trailing space (dispatch_output) — and a leading
    // one on a repaired junction; clipboard-only doesn't type, so there is
    // nothing to scratch. Count grapheme clusters, not scalar values or UTF-16
    // units, so one backspace per visible character: emoji, combining marks,
    // and newlines in a snippet expansion each erase as one. This is correct
    // for grapheme-aware targets (browsers, Electron editors, terminals — the
    // developer audience). A legacy Win32 Edit control deletes one UTF-16 unit
    // per backspace, so it would under-delete a multi-unit grapheme and leave
    // a visible stray glyph — the safer failure mode than over-deleting real
    // text the user did not intend to remove.
    let spaces = if repaired.is_some() { 2 } else { 1 };
    let delivered = if matches!(ctx.output_mode, OutputMode::Clipboard | OutputMode::Stdout) {
        0
    } else {
        // A typed/pasted phrase landed in the target; the session's
        // auto-submit (if configured) now has something to send.
        stats.delivered_to_target = true;
        text.trim().graphemes(true).count() + spaces
    };
    *state
        .last_delivered_len
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = delivered;

    // History, events, and captions all mirror what actually landed on
    // screen, so a repaired junction records the lowercased text.
    record_history(state, text);

    let _ = app.emit(
        "streaming-phrase",
        serde_json::json!({ "text": text, "processing_time_ms": processing_time_ms }),
    );

    *last_delivery = Some(LastDelivery {
        text: text.trim().to_string(),
        delivered_at: Instant::now(),
        typed: matches!(ctx.output_mode, OutputMode::Auto | OutputMode::Keyboard),
        kind,
        #[cfg(windows)]
        target_hwnd: ctx.previous_hwnd,
    });

    // The phrase has landed in the target. Normally the caption clears until
    // the next phrase's first partial; with translated captions active it
    // instead shows the final English rendering for a reading-time hold.
    match &ctx.translated_caption {
        Some(anchor) => crate::caption::show_final(app, anchor, text),
        None => crate::caption::hide(app),
    }
}

/// Decide a punctuation junction repair against the previous delivery and,
/// when it applies, perform the backspace half (removing the stale terminal
/// mark and its trailing space). Returns the replacement text to deliver
/// with joining spaces, or None to deliver unchanged (every gate failure
/// falls back to today's behavior). See `murmur_core::dictation_junction`.
fn attempt_junction_repair(
    state: &AppState,
    text: &str,
    ctx: &DeliveryContext,
    kind: PhraseKind,
    prev: Option<&LastDelivery>,
) -> Option<String> {
    use murmur_core::dictation_junction::{JunctionAction, JunctionGates, junction_action};

    let prev = prev?;
    let enabled = state
        .settings
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .smart_punctuation;

    // The repair backspaces into text we typed earlier, so it must only run
    // when the caret is still where we left it: same target window, still in
    // the foreground. v1 can verify that only on Windows; elsewhere the gate
    // is unsatisfiable and delivery proceeds unchanged.
    #[cfg(windows)]
    let same_target = prev.target_hwnd != 0
        && prev.target_hwnd == ctx.previous_hwnd
        && crate::focus::foreground_window() == prev.target_hwnd
        && !crate::focus::is_own_window(prev.target_hwnd);
    #[cfg(not(windows))]
    let same_target = false;

    let gates = JunctionGates {
        enabled,
        prev_typed: prev.typed,
        prev_plain_dictation: prev.kind == PhraseKind::PlainDictation,
        next_plain_dictation: kind == PhraseKind::PlainDictation,
        typing_mode: matches!(ctx.output_mode, OutputMode::Auto | OutputMode::Keyboard),
        same_target,
    };
    let action = junction_action(&prev.text, text, prev.delivered_at.elapsed(), &gates);
    let JunctionAction::Repair {
        backspaces,
        replacement,
    } = action
    else {
        return None;
    };

    // Booleans/counts only — phrase text never reaches the log.
    tracing::debug!(backspaces, "repairing punctuation junction");
    if let Err(e) = murmur_core::output::keyboard::press_backspace(backspaces) {
        tracing::warn!("Junction backspace failed; delivering unchanged: {}", e);
        return None;
    }
    Some(replacement)
}

/// Append a delivered phrase to the persistent history and per-day insights
/// aggregate, saving both. Best effort: a failed write is logged, never
/// surfaced to the user. Skipped entirely when the user has turned history off.
fn record_history(state: &AppState, text: &str) {
    if !state
        .settings
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .save_history
    {
        return;
    }
    let app_name = current_app_name();
    {
        let mut history = state.history.lock().unwrap_or_else(|e| e.into_inner());
        history.add(text, app_name);
        if let Err(e) = history.save(&state.history_path) {
            tracing::warn!("Failed to save history: {}", e);
        }
    }
    // Same epoch-ms clock the history entry was stamped with. Taken after the
    // history lock is released so the two locks are never held together.
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let mut insights = state.insights.lock().unwrap_or_else(|e| e.into_inner());
    insights.record(text, now_ms);
    if let Err(e) = insights.save(&state.insights_path) {
        tracing::warn!("Failed to save insights: {}", e);
    }
}

/// Name of the foreground application receiving the text, when available.
#[cfg(windows)]
fn current_app_name() -> Option<String> {
    murmur_core::output::keyboard::foreground_window_info_public()
        .and_then(|info| info.process_name)
}

#[cfg(not(windows))]
fn current_app_name() -> Option<String> {
    None
}

/// Run a spoken editing command (new line, new paragraph, scratch that).
fn execute_command(app: &tauri::AppHandle, state: &AppState, command: VoiceCommand) {
    use murmur_core::output::keyboard;

    let result = match command {
        VoiceCommand::NewLine => keyboard::press_enter(1),
        VoiceCommand::NewParagraph => keyboard::press_enter(2),
        VoiceCommand::ScratchThat => {
            let count = *state
                .last_delivered_len
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            // A correction invalidates the decoder context — start fresh so
            // the model doesn't keep biasing toward deleted words.
            state
                .session_prev_text
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clear();
            keyboard::press_backspace(count)
        }
        VoiceCommand::Copy => keyboard::copy(),
        VoiceCommand::Undo => keyboard::undo(),
        VoiceCommand::Redo => keyboard::redo(),
        VoiceCommand::Tab => keyboard::press_tab(),
        VoiceCommand::Escape => keyboard::press_escape(),
        VoiceCommand::Text => return,
    };
    if let Err(e) = result {
        tracing::error!("Voice command failed: {}", e);
        emit_hotkey_error(app, &format!("Voice command failed: {}", e));
    }

    // After any command, there is no longer a coherent "last phrase" to
    // scratch (a second "scratch that" should not run).
    *state
        .last_delivered_len
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = 0;

    let label = match command {
        VoiceCommand::NewLine => "new line",
        VoiceCommand::NewParagraph => "new paragraph",
        VoiceCommand::ScratchThat => "scratch that",
        VoiceCommand::Copy => "copy",
        VoiceCommand::Undo => "undo",
        VoiceCommand::Redo => "redo",
        VoiceCommand::Tab => "tab",
        VoiceCommand::Escape => "escape",
        VoiceCommand::Text => "",
    };
    tracing::info!("Executed voice command: {}", label);
    let _ = app.emit("voice-command", serde_json::json!({ "command": label }));
}

fn deliver_output(
    app: &tauri::AppHandle,
    state: &AppState,
    text: &str,
    ctx: &DeliveryContext,
    join: crate::focus::TextJoin,
) {
    #[cfg(not(windows))]
    let _ = state;
    #[cfg(windows)]
    let last_external_hwnd = *state
        .last_external_foreground
        .lock()
        .unwrap_or_else(|e| e.into_inner());

    // enigo/clipboard interact with OS APIs that can panic — contain it.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        crate::focus::output_text(
            text,
            ctx.output_mode,
            join,
            #[cfg(windows)]
            ctx.previous_hwnd,
            #[cfg(windows)]
            last_external_hwnd,
        )
    }));
    match result {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            tracing::error!("Failed to output text: {}", e);
            emit_hotkey_error(app, &format!("Failed to output text: {}", e));
        }
        Err(panic_info) => {
            let msg = panic_message(panic_info, "unknown panic in output_text");
            tracing::error!("output_text panicked: {}", msg);
            emit_hotkey_error(app, &format!("Output crashed: {}", msg));
        }
    }
}

fn finish_streaming(
    app: &tauri::AppHandle,
    state: &AppState,
    stats: &SessionStats,
    ctx: &DeliveryContext,
    sound_feedback: bool,
    generation: u64,
) {
    // A session just ended: restart the model idle-unload clock from here.
    crate::idle_unload::touch(state);
    // A superseded worker (the user already stopped and restarted) must not play
    // the stop cue, surface this session's diagnostics, or clear the live
    // session's flag/UI. release_if_current clears the flag iff still current.
    if !release_if_current(app, state, generation) {
        return;
    }

    // Only the current session may submit: a superseded worker returning above
    // must never press Enter into whatever the new session is typing.
    maybe_auto_submit(ctx, stats);

    if sound_feedback {
        crate::sound::play_stop();
    }
    if !stats.had_transcription && !stats.saw_no_signal {
        let msg = if stats.saw_signal || stats.had_phrase_audio {
            "Speech was detected, but transcription failed. Try speaking a bit slower/closer to the mic, or switch to a larger model."
        } else {
            "No speech detected — check your microphone input"
        };
        emit_hotkey_error(app, msg);
    }

    // Drop any per-app override so the next session starts from the globals.
    *state
        .session_dev_mode
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = None;
    // With translated captions on, the last phrase usually finishes decoding
    // just before the session ends; let it live out its reading hold (the
    // caption page blanks itself) instead of yanking it away here.
    if ctx.translated_caption.is_none() {
        crate::caption::hide(app);
    }
    let _ = app.emit("streaming-done", serde_json::json!({}));
}

/// Fire the profile-configured submit keystroke once at session end, gated by
/// `murmur_core::dictation_submit`: the session must have ended normally,
/// delivered at least one phrase by typing or paste, and the window it
/// delivered into must still be in the foreground. Any failed gate skips
/// silently at debug level.
fn maybe_auto_submit(ctx: &DeliveryContext, stats: &SessionStats) {
    use murmur_core::dictation_submit::{SubmitGates, submit_action};

    // Same-hwnd discipline as junction repair: only press keys where the
    // session's text went. v1 can verify that only on Windows; elsewhere the
    // gate is unsatisfiable and nothing is pressed.
    #[cfg(windows)]
    let same_target = ctx.previous_hwnd != 0
        && crate::focus::foreground_window() == ctx.previous_hwnd
        && !crate::focus::is_own_window(ctx.previous_hwnd);
    #[cfg(not(windows))]
    let same_target = false;

    let gates = SubmitGates {
        ended_normally: stats.ended_normally,
        delivered_phrase: stats.delivered_to_target,
        places_text: !matches!(ctx.output_mode, OutputMode::Clipboard | OutputMode::Stdout),
        same_target,
    };
    let Some(key) = submit_action(ctx.auto_submit, &gates) else {
        if ctx.auto_submit.is_some() {
            tracing::debug!(?gates, "auto-submit skipped");
        }
        return;
    };
    let result = match key {
        AutoSubmit::Enter => murmur_core::output::keyboard::press_enter(1),
        AutoSubmit::CtrlEnter => murmur_core::output::keyboard::press_ctrl_enter(),
    };
    match result {
        Ok(()) => tracing::debug!(?key, "auto-submit keystroke sent"),
        Err(e) => tracing::warn!("Auto-submit keystroke failed: {}", e),
    }
}
