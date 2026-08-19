//! Dedicated audio thread: capture, calibration, and phrase detection.
//!
//! Owns the CPAL capture stream and a `DictationSession`, communicating with
//! the rest of the app over channels. Capture and inference must never share
//! a thread: CPAL callbacks are realtime, and whisper holds the engine for
//! the duration of an inference.

use murmur_core::audio::AudioBuffer;
use murmur_core::audio::capture::AudioCapture;
use murmur_core::audio::silence::{compute_rms, downmix_to_mono};
use murmur_core::dictation::{DictationEvent, DictationSession};
use std::collections::VecDeque;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::calibration::{
    build_session, calibrate, keep_calibration_preroll, trim_buffer_to_preroll,
};
use crate::gain::GainStage;
use crate::state::emit_hotkey_error;

/// Rolling window for the in-session noise-floor estimate: ~5s of 50ms ticks.
const FLOOR_WINDOW_TICKS: usize = 100;
const MONITOR_TICK: Duration = Duration::from_millis(50);
/// Emit a live-preview partial every this many ticks (~700ms). Spaced out so
/// interim transcriptions stay cheap and never starve the final-phrase path.
const PARTIAL_TICKS: u32 = 14;
/// End the session if the stream stops delivering samples for this many ticks
/// mid-recording (~3s). A connected mic always delivers silence samples, so a
/// sustained gap means the device disconnected or the driver stalled.
const MID_SESSION_STALL_TICKS: u32 = 60;
/// RMS floor above which a chunk counts as audible signal for the UI: the
/// one-shot `SignalDetected` event and the floor of the pill's activity
/// threshold (`SpeechThreshold`) both use it.
const SIGNAL_FLOOR_RMS: f32 = 0.002;

/// How long the start handshake waits for its `Started`/`StartFailed`.
const START_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
/// Longest gap tolerated between streaming results before the session is
/// treated as wedged.
const RESULT_TIMEOUT: Duration = Duration::from_secs(120);

/// Generation stamp for results that belong to no particular session (a
/// worker-level failure); every receiver accepts them. Session generations
/// start at 1 (see `session::handle_toggle`), so zero is free.
const ANY_GENERATION: u64 = 0;

/// What triggered a session. Wake starts get a silent-abort deadline (a
/// false wake trigger must not sit recording until session_timeout) and an
/// engine wait in the streaming worker.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum StartReason {
    /// Every explicit user start: hotkey, double-tap, UI, tray, MCP.
    Hotkey,
    WakeWord,
}

#[derive(Clone)]
pub(crate) struct StartParams {
    /// Session generation this start belongs to. Every result it produces is
    /// stamped with it, so a superseded session's queued results can never be
    /// consumed by the next one.
    pub generation: u64,
    pub start_reason: StartReason,
    pub audio_device: Option<String>,
    pub rms_threshold: f32,
    pub vad_threshold: f32,
    pub phrase_pause: Duration,
    pub session_timeout: Duration,
    /// Emit periodic `PartialPhrase` snapshots for live preview.
    pub live_preview: bool,
    /// Use the OS echo-cancelling capture path when available.
    pub echo_cancellation: bool,
    /// Keep the mic stream open between sessions (audio discarded while idle).
    pub mic_warm_start: bool,
}

/// Warm-start reconfiguration pushed from the settings path, so toggling the
/// setting (or changing the input device) takes effect immediately instead of
/// waiting for the next session.
#[derive(Clone)]
pub(crate) struct WarmParams {
    pub enabled: bool,
    pub audio_device: Option<String>,
    pub echo_cancellation: bool,
}

/// Arm the always-listening wake detector.
pub(crate) struct ArmParams {
    pub audio_device: Option<String>,
    /// Wake-probability threshold (from the sensitivity setting).
    pub threshold: f32,
    /// Worker → supervisor transitions. Dedicated channel: the result
    /// channel is consumed per-generation by streaming workers and a
    /// second drainer would contend for its receiver.
    pub wake_tx: mpsc::Sender<WakeEvent>,
    /// Scorer injected by the caller so tests script detections and the
    /// supervisor builds the real ONNX scorer off-thread.
    pub scorer: Box<dyn murmur_core::audio::wake::WakeScorer>,
}

/// Worker-originated armed-state transitions. These are the single source of
/// truth for the armed indicator: the UI shows "armed" only after `Armed`
/// arrives, and never after `Disarmed`.
pub(crate) enum WakeEvent {
    /// Stream open, detector fed — the moment the UI may show "armed".
    Armed,
    /// Wake phrase detected; the supervisor should start a session.
    Detected { score: f32 },
    /// Armed state ended. `failed` distinguishes a fault the supervisor may
    /// retry (mic loss, scorer failure) from an expected exit (disarm,
    /// session handoff), which must never trigger a retry.
    Disarmed { reason: String, failed: bool },
}

enum Cmd {
    StartStreaming(StartParams),
    Stop,
    SetWarm(WarmParams),
    // Only the wake-gated supervisor arms; the machinery itself is
    // feature-independent so its tests run on every build.
    #[cfg_attr(not(feature = "wake"), allow(dead_code))]
    Arm(ArmParams),
    Disarm,
}

pub(crate) enum AudioResult {
    Started,
    StartFailed(String),
    PhraseReady(AudioBuffer),
    /// Snapshot of the in-progress phrase for live preview, sent every
    /// ~700ms while speech continues. Transcribed for display only, never
    /// delivered to the target app.
    PartialPhrase(AudioBuffer),
    /// Periodic RMS level update, sent every ~100ms.
    AudioLevel(f32),
    SignalDetected,
    /// The RMS level the UI should treat as speech activity: the calibrated
    /// (and adaptively lowered) speech threshold, capped at
    /// [`SIGNAL_FLOOR_RMS`] so the pill is never less sensitive than the
    /// legacy fixed floor. Sent after calibration and on every adaptive
    /// change so the pill's dormancy logic tracks what the capture path
    /// actually counts as speech.
    SpeechThreshold(f32),
    NoSignal(String),
    StreamingDone,
}

/// A result plus the session generation that produced it.
struct Tagged {
    generation: u64,
    result: AudioResult,
}

/// Whether a receiver waiting on `expected` may consume a result stamped
/// `stamped`. Worker-level results ([`ANY_GENERATION`]) reach everyone.
fn accepts(stamped: u64, expected: u64) -> bool {
    stamped == expected || stamped == ANY_GENERATION
}

/// Sender that stamps every result with the session generation it belongs to.
struct ResultSender {
    tx: mpsc::Sender<Tagged>,
    generation: u64,
}

impl ResultSender {
    /// Send a result for this session. A closed channel means the app-side
    /// worker is gone, which every call site already treats as benign.
    fn send(&self, result: AudioResult) {
        self.send_as(self.generation, result);
    }

    /// Send on behalf of another generation: a rejected `StartStreaming` must
    /// answer the session that queued it, not the one rejecting it.
    fn send_as(&self, generation: u64, result: AudioResult) {
        let _ = self.tx.send(Tagged { generation, result });
    }
}

/// Send a result that belongs to no session (a worker-level failure).
fn broadcast(tx: &mpsc::Sender<Tagged>, result: AudioResult) {
    let _ = tx.send(Tagged {
        generation: ANY_GENERATION,
        result,
    });
}

pub(crate) struct Handle {
    cmd_tx: mpsc::Sender<Cmd>,
    result_rx: Mutex<mpsc::Receiver<Tagged>>,
}

impl Handle {
    pub fn spawn(app_handle: tauri::AppHandle) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel::<Cmd>();
        let (result_tx, result_rx) = mpsc::channel::<Tagged>();

        std::thread::spawn(move || {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_worker(&cmd_rx, &result_tx);
            }));
            if let Err(panic_info) = outcome {
                let msg = panic_message(panic_info, "unknown panic in audio worker thread");
                tracing::error!("Audio worker thread panicked: {}", msg);
                broadcast(
                    &result_tx,
                    AudioResult::StartFailed(format!("Audio worker crash: {}", msg)),
                );
                emit_hotkey_error(&app_handle, &format!("Audio driver crashed: {}", msg));
            }
        });

        Handle::new(cmd_tx, result_rx)
    }

    fn new(cmd_tx: mpsc::Sender<Cmd>, result_rx: mpsc::Receiver<Tagged>) -> Self {
        Handle {
            cmd_tx,
            result_rx: Mutex::new(result_rx),
        }
    }

    /// Queue a StartStreaming command. Non-blocking; called synchronously
    /// from the toggle handler so the command channel's FIFO ordering
    /// guarantees a later Stop toggle can never overtake the start.
    ///
    /// We do NOT drain the result channel here: a new streaming worker first
    /// joins the prior one (see `spawn_streaming_worker`), so the prior session
    /// has already consumed its own results — including its final `PhraseReady`.
    /// Draining here would race that and discard the user's last phrase.
    pub fn send_start(&self, params: StartParams) -> Result<(), String> {
        self.cmd_tx
            .send(Cmd::StartStreaming(params))
            .map_err(|e| format!("Audio worker channel closed: {}", e))
    }

    /// Block until the StartStreaming command queued for `generation` is
    /// acknowledged. Results from any other generation are discarded: a
    /// previous session that started late (past this handshake's timeout) may
    /// still have its own `Started` and phrases sitting in the channel, and
    /// consuming them here would splice its audio into this session.
    pub fn await_started(&self, generation: u64) -> Result<(), String> {
        let rx = self.result_rx.lock().unwrap_or_else(|e| e.into_inner());
        let deadline = Instant::now() + START_HANDSHAKE_TIMEOUT;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match rx.recv_timeout(remaining) {
                Ok(tagged) if !accepts(tagged.generation, generation) => {
                    tracing::debug!(
                        stamped = tagged.generation,
                        expected = generation,
                        "Discarding another session's audio result during start handshake"
                    );
                }
                Ok(Tagged {
                    result: AudioResult::Started,
                    ..
                }) => return Ok(()),
                Ok(Tagged {
                    result: AudioResult::StartFailed(e),
                    ..
                }) => return Err(e),
                Ok(_) => {
                    tracing::debug!("Ignoring stale audio worker result during start handshake");
                }
                Err(e) => return Err(format!("Audio worker response timeout: {}", e)),
            }
        }
    }

    /// Request the worker to stop recording. Non-blocking.
    pub fn request_stop(&self) -> Result<(), String> {
        self.cmd_tx
            .send(Cmd::Stop)
            .map_err(|e| format!("Audio worker channel closed: {}", e))
    }

    /// Push a warm-start reconfiguration to the worker. Non-blocking; applied
    /// between sessions (or, mid-session, the enabled flag alone so a disable
    /// releases the mic when the session ends).
    pub fn send_set_warm(&self, params: WarmParams) -> Result<(), String> {
        self.cmd_tx
            .send(Cmd::SetWarm(params))
            .map_err(|e| format!("Audio worker channel closed: {}", e))
    }

    /// Arm the wake detector. Non-blocking; the worker answers with
    /// `WakeEvent::Armed` (or `Disarmed`) on the params' wake channel.
    #[cfg_attr(not(feature = "wake"), allow(dead_code))]
    pub fn send_arm(&self, params: ArmParams) -> Result<(), String> {
        self.cmd_tx
            .send(Cmd::Arm(params))
            .map_err(|e| format!("Audio worker channel closed: {}", e))
    }

    /// Leave the armed state. Non-blocking; a no-op when not armed.
    pub fn send_disarm(&self) -> Result<(), String> {
        self.cmd_tx
            .send(Cmd::Disarm)
            .map_err(|e| format!("Audio worker channel closed: {}", e))
    }

    /// Blocking receive for the next streaming result of `generation`. Results
    /// stamped with another generation are discarded, never delivered: an
    /// orphaned session's buffered phrases must never be typed into the window
    /// this session is targeting.
    pub fn recv_result(&self, generation: u64) -> Result<AudioResult, String> {
        let rx = self.result_rx.lock().unwrap_or_else(|e| e.into_inner());
        let deadline = Instant::now() + RESULT_TIMEOUT;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let tagged = rx
                .recv_timeout(remaining)
                .map_err(|e| format!("Audio worker recv timeout: {}", e))?;
            if accepts(tagged.generation, generation) {
                return Ok(tagged.result);
            }
            tracing::debug!(
                stamped = tagged.generation,
                expected = generation,
                "Discarding another session's audio result"
            );
        }
    }
}

pub(crate) fn panic_message(panic_info: Box<dyn std::any::Any + Send>, fallback: &str) -> String {
    if let Some(s) = panic_info.downcast_ref::<String>() {
        s.clone()
    } else if let Some(s) = panic_info.downcast_ref::<&str>() {
        (*s).to_string()
    } else {
        fallback.to_string()
    }
}

fn run_worker(cmd_rx: &mpsc::Receiver<Cmd>, result_tx: &mpsc::Sender<Tagged>) {
    let mut capture = match AudioCapture::new() {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("Failed to create AudioCapture: {}", e);
            broadcast(result_tx, AudioResult::StartFailed(e.to_string()));
            return;
        }
    };

    while let Ok(cmd) = cmd_rx.recv() {
        match cmd {
            Cmd::StartStreaming(params) => {
                let sender = ResultSender {
                    tx: result_tx.clone(),
                    generation: params.generation,
                };
                // Contain a panic inside a single session (a driver or VAD bug)
                // so it ends that session instead of unwinding the whole worker
                // thread — which would wedge every future session with a closed
                // channel. The thread-level catch in Handle::spawn remains a
                // last resort for a panic outside a session.
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run_session(&mut capture, &params, cmd_rx, &sender);
                }));
                if let Err(panic_info) = outcome {
                    let msg = panic_message(panic_info, "panic in recording session");
                    tracing::error!("Recording session panicked, recovering: {}", msg);
                    // Best-effort cleanup, then unblock the app-side streaming
                    // worker so the UI returns to idle. Don't keep a stream
                    // that just panicked warm — drop it and let the next
                    // session rebuild from scratch (it re-enables warm mode
                    // from its params).
                    capture.set_warm_start(false);
                    stop_capture(&mut capture, "panic recovery");
                    sender.send(AudioResult::StreamingDone);
                }
            }
            Cmd::Stop => {
                tracing::debug!("Stop received outside monitoring loop, ignoring");
            }
            Cmd::SetWarm(params) => apply_warm(&mut capture, &params),
            Cmd::Arm(params) => {
                let wake_tx = params.wake_tx.clone();
                // Same per-session panic containment as StartStreaming: a
                // scorer or driver bug ends the armed period, not the worker.
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run_armed(&mut capture, params, cmd_rx)
                }));
                match outcome {
                    Ok(ArmedExit::ToIdle) => {}
                    Ok(ArmedExit::StartSession {
                        params: start_params,
                        ambient_floor,
                        detected_at,
                    }) => {
                        let sender = ResultSender {
                            tx: result_tx.clone(),
                            generation: start_params.generation,
                        };
                        let outcome =
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                match armed_start_route(start_params.start_reason) {
                                    ArmedStartRoute::ReusePassiveStream => run_session_prearmed(
                                        &mut capture,
                                        &start_params,
                                        cmd_rx,
                                        &sender,
                                        ambient_floor,
                                        detected_at,
                                    ),
                                    ArmedStartRoute::ReopenDevice => {
                                        // Close the passive stream, and with it
                                        // any pre-press audio; run_session then
                                        // starts over with the session's echo
                                        // cancellation setting, exactly as if
                                        // the app had been idle.
                                        stop_capture(&mut capture, "armed explicit-start handoff");
                                        run_session(&mut capture, &start_params, cmd_rx, &sender);
                                    }
                                }
                            }));
                        if let Err(panic_info) = outcome {
                            let msg =
                                panic_message(panic_info, "panic in session started while armed");
                            tracing::error!(
                                "Session started while armed panicked, recovering: {}",
                                msg
                            );
                            capture.set_warm_start(false);
                            stop_capture(&mut capture, "armed-start session panic recovery");
                            sender.send(AudioResult::StreamingDone);
                        }
                    }
                    Err(panic_info) => {
                        let msg = panic_message(panic_info, "panic in armed wake loop");
                        tracing::error!("Armed wake loop panicked, recovering: {}", msg);
                        stop_capture(&mut capture, "armed panic recovery");
                        let _ = wake_tx.send(WakeEvent::Disarmed {
                            reason: msg,
                            failed: true,
                        });
                    }
                }
            }
            Cmd::Disarm => {
                tracing::debug!("Disarm received while not armed, ignoring");
            }
        }
    }
}

/// Ticks (80 ms each) the armed loop tolerates without new samples before
/// treating the device as gone (~5 s).
const ARMED_STALL_TICKS: u32 = 60;
/// Armed loop cadence: one wake frame (80 ms at 16 kHz).
const WAKE_TICK: Duration = Duration::from_millis(80);
/// Ambient-floor window while armed: ~5 s of 80 ms ticks.
const ARMED_FLOOR_TICKS: usize = 63;
/// Consecutive scorer failures tolerated before the armed loop gives up.
const ARMED_MAX_SCORER_FAILURES: u32 = 10;

/// Whether the armed loop should treat a sample gap as device loss.
fn armed_should_give_up(consecutive_empty_ticks: u32) -> bool {
    consecutive_empty_ticks >= ARMED_STALL_TICKS
}

/// A wake session that hears no speech gives up after this window instead of
/// idling until `session_timeout` — the cost cap on every false trigger.
const WAKE_SILENT_ABORT: Duration = Duration::from_secs(4);

/// Rewind from detection time into the retained live audio: the head's score
/// peaks this long after the wake phrase ends, and without the rewind a
/// no-pause command loses its first syllables. Initial value; finalized by
/// the measurement task against real recordings.
const WAKE_LAG_COMPENSATION_MS: u64 = 240;

/// Whether a wake session's silent-abort deadline has expired.
fn wake_abort_due(deadline: Option<Instant>, speech_started: bool) -> bool {
    !speech_started && deadline.is_some_and(|d| Instant::now() >= d)
}

/// How much trailing audio a wake session keeps at start: the lag
/// compensation anchored at detection time, so the supervisor's handoff
/// latency never eats the command onset. Capped at the ~1 s the armed
/// live buffer retains.
fn wake_keep_duration(elapsed_since_detection: Duration) -> Duration {
    (elapsed_since_detection + Duration::from_millis(WAKE_LAG_COMPENSATION_MS))
        .min(Duration::from_secs(1))
}

/// Trailing sample count for [`wake_keep_duration`], channel-aligned so
/// interleaved frames stay in step for downmix.
fn wake_keep_samples(keep: Duration, native_rate: u32, native_channels: u16) -> usize {
    let ch = native_channels.max(1) as usize;
    let frames = (native_rate as f64 * keep.as_secs_f64()) as usize;
    frames * ch
}

/// How a start that lands while armed acquires its stream. A wake start must
/// stay on the open passive stream: the phrase was already spoken, so the
/// buffered audio is the only copy of the command onset. An explicit
/// (hotkey/UI) start reopens the device instead: the passive stream is raw
/// CPAL by design, and inheriting it silently dropped the echo-cancelled
/// voice path that hotkey dictation gets when not armed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArmedStartRoute {
    ReusePassiveStream,
    ReopenDevice,
}

fn armed_start_route(reason: StartReason) -> ArmedStartRoute {
    match reason {
        StartReason::WakeWord => ArmedStartRoute::ReusePassiveStream,
        StartReason::Hotkey => ArmedStartRoute::ReopenDevice,
    }
}

enum ArmedExit {
    ToIdle,
    /// A StartStreaming arrived while armed: run it on the open stream.
    /// `ambient_floor` is the armed loop's rolling noise estimate (replacing
    /// startup calibration) and `detected_at` anchors the lag rewind.
    StartSession {
        params: StartParams,
        ambient_floor: f32,
        detected_at: Option<Instant>,
    },
}

/// The always-listening loop: score mic audio in memory, discard it, and
/// report detections. Holds at most `WAKE_RING_SECS` of raw audio (for the
/// detection-lag rewind) plus the scorer's internal feature buffers —
/// nothing is written anywhere.
fn run_armed(
    capture: &mut AudioCapture,
    params: ArmParams,
    cmd_rx: &mpsc::Receiver<Cmd>,
) -> ArmedExit {
    let ArmParams {
        audio_device,
        threshold,
        wake_tx,
        scorer,
    } = params;

    if let Err(msg) = start_passive_capture(capture, audio_device.as_deref()) {
        let _ = wake_tx.send(WakeEvent::Disarmed {
            reason: msg,
            failed: true,
        });
        return ArmedExit::ToIdle;
    }
    let live_buf = capture.live_buffer();
    if !probe_initial_samples(&live_buf) {
        stop_capture(capture, "armed startup sample probe failure");
        let _ = wake_tx.send(WakeEvent::Disarmed {
            reason: "Microphone opened but produced no audio samples.".to_string(),
            failed: true,
        });
        return ArmedExit::ToIdle;
    }
    let native_rate = capture.native_rate();
    let native_channels = capture.native_channels();
    tracing::info!(native_rate, native_channels, "wake armed: stream open");
    let _ = wake_tx.send(WakeEvent::Armed);

    let mut detector = murmur_core::audio::wake::WakeWordDetector::new(scorer, threshold);
    let mut floor_window: VecDeque<f32> = VecDeque::with_capacity(ARMED_FLOOR_TICKS);
    let mut analyzed_up_to = 0usize;
    let mut empty_ticks = 0u32;
    let mut scorer_failures = 0u32;
    let mut last_detection: Option<Instant> = None;

    loop {
        match cmd_rx.try_recv() {
            Ok(Cmd::Disarm) => {
                stop_capture(capture, "wake disarm");
                let _ = wake_tx.send(WakeEvent::Disarmed {
                    reason: "disarm".to_string(),
                    failed: false,
                });
                return ArmedExit::ToIdle;
            }
            Ok(Cmd::StartStreaming(start_params)) => {
                let ambient_floor = floor_window.iter().copied().fold(f32::INFINITY, f32::min);
                let ambient_floor = if ambient_floor.is_finite() {
                    ambient_floor
                } else {
                    0.0
                };
                // The armed period ends here; the session takes the stream.
                // Not a failure: the supervisor re-arms at session end.
                let _ = wake_tx.send(WakeEvent::Disarmed {
                    reason: "session start".to_string(),
                    failed: false,
                });
                return ArmedExit::StartSession {
                    params: start_params,
                    ambient_floor,
                    detected_at: last_detection,
                };
            }
            Ok(Cmd::Arm(other)) => {
                // Answer the second arm's own channel so its supervisor
                // isn't left waiting; this armed period continues.
                let _ = other.wake_tx.send(WakeEvent::Disarmed {
                    reason: "already armed".to_string(),
                    failed: false,
                });
            }
            Ok(Cmd::SetWarm(p)) => capture.set_warm_start(p.enabled),
            Ok(Cmd::Stop) => {
                tracing::debug!("Stop received while armed (no session), ignoring");
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                stop_capture(capture, "wake channel close");
                let _ = wake_tx.send(WakeEvent::Disarmed {
                    reason: "command channel closed".to_string(),
                    failed: false,
                });
                return ArmedExit::ToIdle;
            }
        }

        // Snapshot new samples under the lock, then release before any
        // inference — the realtime CPAL callback must never wait on us.
        let raw = {
            let mut buf = live_buf.lock().unwrap_or_else(|e| e.into_inner());
            if buf.len() <= analyzed_up_to {
                None
            } else {
                let snapshot = buf[analyzed_up_to..].to_vec();
                analyzed_up_to = buf.len();
                let dropped = trim_buffer_to_preroll(&mut buf, native_rate, native_channels);
                analyzed_up_to = analyzed_up_to.saturating_sub(dropped);
                Some(snapshot)
            }
        };

        match raw {
            None => {
                empty_ticks += 1;
                if armed_should_give_up(empty_ticks) {
                    stop_capture(capture, "armed no samples");
                    let _ = wake_tx.send(WakeEvent::Disarmed {
                        reason: "Microphone stopped delivering audio while armed.".to_string(),
                        failed: true,
                    });
                    return ArmedExit::ToIdle;
                }
            }
            Some(raw) => {
                empty_ticks = 0;
                let mono16k =
                    murmur_core::audio::AudioBuffer::from_raw(&raw, native_rate, native_channels)
                        .samples;
                let rms = compute_rms(&mono16k);
                if rms > 0.0 {
                    if floor_window.len() >= ARMED_FLOOR_TICKS {
                        let _ = floor_window.pop_front();
                    }
                    floor_window.push_back(rms);
                }
                match detector.feed(&mono16k) {
                    Ok(Some(detection)) => {
                        scorer_failures = 0;
                        last_detection = Some(Instant::now());
                        tracing::info!(score = detection.score, "wake word detected");
                        let _ = wake_tx.send(WakeEvent::Detected {
                            score: detection.score,
                        });
                    }
                    Ok(None) => scorer_failures = 0,
                    Err(e) => {
                        scorer_failures += 1;
                        tracing::warn!(failures = scorer_failures, "wake scorer error: {}", e);
                        if scorer_failures >= ARMED_MAX_SCORER_FAILURES {
                            stop_capture(capture, "wake scorer failing");
                            let _ = wake_tx.send(WakeEvent::Disarmed {
                                reason: "Wake detector kept failing.".to_string(),
                                failed: true,
                            });
                            return ArmedExit::ToIdle;
                        }
                    }
                }
            }
        }
        std::thread::sleep(WAKE_TICK);
    }
}

/// Apply a warm-start reconfiguration while idle: flip the mode, then open
/// (or retarget) the idle pre-warm stream so even the next session of the run
/// skips the cold device open. Failure is non-fatal — the next session simply
/// cold-starts as before.
fn apply_warm(capture: &mut AudioCapture, params: &WarmParams) {
    capture.set_warm_start(params.enabled);
    if !params.enabled {
        return;
    }
    // CPAL's native backend can panic on some drivers; contain it like start.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        capture.prewarm(params.audio_device.as_deref(), params.echo_cancellation)
    }));
    match result {
        Ok(Ok(())) => {}
        Ok(Err(e)) => tracing::warn!("Mic pre-warm failed; next session will cold-start: {}", e),
        Err(panic_info) => tracing::warn!(
            "Mic pre-warm panicked; next session will cold-start: {}",
            panic_message(panic_info, "native audio panic")
        ),
    }
}

/// One recording session: start the stream, calibrate, then monitor until
/// stopped or timed out. Errors are reported through `result_tx`.
fn run_session(
    capture: &mut AudioCapture,
    params: &StartParams,
    cmd_rx: &mpsc::Receiver<Cmd>,
    result_tx: &ResultSender,
) {
    // Sync the warm mode with the setting each session, so a change made
    // while no SetWarm command was processed still applies here.
    capture.set_warm_start(params.mic_warm_start);
    if let Err(msg) = start_capture(
        capture,
        params.audio_device.as_deref(),
        params.echo_cancellation,
    ) {
        result_tx.send(AudioResult::StartFailed(msg));
        return;
    }

    let live_buf = capture.live_buffer();
    let native_rate = capture.native_rate();
    let native_channels = capture.native_channels();
    tracing::info!(
        "Audio stream started: native_rate={}Hz, native_channels={}",
        native_rate,
        native_channels,
    );

    if !probe_initial_samples(&live_buf) {
        let msg = "Microphone opened but produced no audio samples. Check your microphone permissions and the selected input device.".to_string();
        tracing::error!("{}", msg);
        stop_capture(capture, "startup sample probe failure");
        result_tx.send(AudioResult::StartFailed(msg));
        return;
    }
    result_tx.send(AudioResult::Started);

    let echo_cancellation = capture.echo_cancellation_active();
    let mut analyzed_up_to = 0usize;
    let calibration = calibrate(
        &live_buf,
        &mut analyzed_up_to,
        native_channels,
        native_rate,
        params.rms_threshold,
        echo_cancellation,
    );
    keep_calibration_preroll(&live_buf, &mut analyzed_up_to, native_rate, native_channels);

    let session = build_session(params, &calibration, native_rate, echo_cancellation);
    result_tx.send(AudioResult::SpeechThreshold(
        calibration.threshold.min(SIGNAL_FLOOR_RMS),
    ));

    Monitor {
        cmd_rx,
        result_tx,
        capture,
        live_buf,
        session,
        native_rate,
        native_channels,
        analyzed_up_to,
        gain: GainStage::new(calibration.mic_gain),
        effective_ambient: calibration.effective_ambient,
        auto_threshold: params.rms_threshold <= 0.0,
        current_threshold: calibration.threshold,
        floor_window: VecDeque::with_capacity(FLOOR_WINDOW_TICKS),
        level_tick: 0,
        saw_signal: false,
        had_phrase: false,
        consecutive_no_sample_ticks: 0,
        consecutive_silent_ticks: 0,
        startup_deadline: Instant::now() + Duration::from_millis(1200),
        silence_deadline: Instant::now() + Duration::from_secs(3),
        live_preview: params.live_preview,
        wake_abort_deadline: None,
    }
    .run();
}

/// A wake-word session on the already-open armed stream: it is verified
/// live, ambient was measured while armed (no calibration pause), the live
/// buffer is rewound to just past the wake phrase, and a silent-abort
/// deadline caps the cost of a false trigger. Only wake starts route here
/// (see [`armed_start_route`]); an explicit start that lands while armed
/// reopens the device through [`run_session`] so it keeps the
/// echo-cancelled path it would have had when idle.
fn run_session_prearmed(
    capture: &mut AudioCapture,
    params: &StartParams,
    cmd_rx: &mpsc::Receiver<Cmd>,
    result_tx: &ResultSender,
    ambient_floor: f32,
    detected_at: Option<Instant>,
) {
    capture.set_warm_start(params.mic_warm_start);
    let live_buf = capture.live_buffer();
    let native_rate = capture.native_rate();
    let native_channels = capture.native_channels();
    let echo_cancellation = capture.echo_cancellation_active();

    // Rewind: keep only audio from (detection - lag compensation) onward.
    // Everything earlier — the wake phrase itself — is dropped here, which
    // is what keeps "Hey Murmur" out of the delivered text.
    let keep = wake_keep_duration(detected_at.map(|t| t.elapsed()).unwrap_or(Duration::ZERO));
    let keep_samples = wake_keep_samples(keep, native_rate, native_channels);
    {
        let mut buf = live_buf.lock().unwrap_or_else(|e| e.into_inner());
        if buf.len() > keep_samples {
            let mut drop_count = buf.len() - keep_samples;
            drop_count -= drop_count % native_channels.max(1) as usize;
            buf.drain(..drop_count);
        }
    }
    tracing::info!(
        keep_ms = keep.as_millis() as u64,
        native_rate,
        native_channels,
        "wake session starting on the armed stream"
    );
    result_tx.send(AudioResult::Started);

    let calibration =
        crate::calibration::from_ambient(ambient_floor, params.rms_threshold, echo_cancellation);
    let session = build_session(params, &calibration, native_rate, echo_cancellation);
    result_tx.send(AudioResult::SpeechThreshold(
        calibration.threshold.min(SIGNAL_FLOOR_RMS),
    ));

    Monitor {
        cmd_rx,
        result_tx,
        capture,
        live_buf,
        session,
        native_rate,
        native_channels,
        analyzed_up_to: 0,
        gain: GainStage::new(calibration.mic_gain),
        effective_ambient: calibration.effective_ambient,
        auto_threshold: params.rms_threshold <= 0.0,
        current_threshold: calibration.threshold,
        floor_window: VecDeque::with_capacity(FLOOR_WINDOW_TICKS),
        level_tick: 0,
        saw_signal: false,
        had_phrase: false,
        consecutive_no_sample_ticks: 0,
        consecutive_silent_ticks: 0,
        startup_deadline: Instant::now() + Duration::from_millis(1200),
        silence_deadline: Instant::now() + Duration::from_secs(3),
        live_preview: params.live_preview,
        wake_abort_deadline: Some(Instant::now() + WAKE_SILENT_ABORT),
    }
    .run();
}

/// CPAL's native backend (WASAPI) can panic on some drivers; contain it.
fn start_capture(
    capture: &mut AudioCapture,
    device: Option<&str>,
    echo_cancellation: bool,
) -> Result<(), String> {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        capture.start(device, echo_cancellation)
    }));
    match result {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(e.to_string()),
        Err(panic_info) => {
            let msg = panic_message(panic_info, "native audio panic");
            tracing::error!("Audio capture panicked on start: {}", msg);
            Err(msg)
        }
    }
}

/// Passive (wake) variant of [`start_capture`]: raw mic only, AEC health
/// untouched. See `AudioCapture::start_passive` for why arming must not reach
/// the voice-capture path.
fn start_passive_capture(capture: &mut AudioCapture, device: Option<&str>) -> Result<(), String> {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        capture.start_passive(device)
    }));
    match result {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(e.to_string()),
        Err(panic_info) => {
            let msg = panic_message(panic_info, "native audio panic");
            tracing::error!("Passive audio capture panicked on start: {}", msg);
            Err(msg)
        }
    }
}

fn stop_capture(capture: &mut AudioCapture, context: &str) {
    if let Err(e) = capture.stop() {
        tracing::error!("Failed to stop audio capture ({}): {}", context, e);
    }
}

/// Some WASAPI paths report success but never produce callbacks; wait for
/// the first samples before declaring the session started.
fn probe_initial_samples(live_buf: &Arc<Mutex<Vec<f32>>>) -> bool {
    let deadline = Instant::now() + Duration::from_millis(1500);
    while Instant::now() < deadline {
        let has_samples = {
            let buf = live_buf.lock().unwrap_or_else(|e| e.into_inner());
            !buf.is_empty()
        };
        if has_samples {
            return true;
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    false
}

/// The per-session monitoring loop and its state.
struct Monitor<'a> {
    cmd_rx: &'a mpsc::Receiver<Cmd>,
    result_tx: &'a ResultSender,
    capture: &'a mut AudioCapture,
    live_buf: Arc<Mutex<Vec<f32>>>,
    session: DictationSession,
    native_rate: u32,
    native_channels: u16,
    analyzed_up_to: usize,
    gain: GainStage,
    effective_ambient: f32,
    auto_threshold: bool,
    current_threshold: f32,
    floor_window: VecDeque<f32>,
    level_tick: u32,
    saw_signal: bool,
    /// A non-empty phrase has been forwarded this session.
    had_phrase: bool,
    consecutive_no_sample_ticks: u32,
    /// Post-signal ticks where the stream delivers only digital silence.
    consecutive_silent_ticks: u32,
    startup_deadline: Instant,
    silence_deadline: Instant,
    live_preview: bool,
    /// Wake sessions only: give up if no speech starts before this. Cleared
    /// on the first sign of speech; a false trigger self-cancels here.
    wake_abort_deadline: Option<Instant>,
}

enum Flow {
    Continue,
    EndSession,
}

impl Monitor<'_> {
    fn run(mut self) {
        loop {
            if let Flow::EndSession = self.handle_command() {
                return;
            }

            let chunk = self.take_chunk();
            let (events, chunk_rms) = match &chunk {
                Some(mono) => (self.session.ingest(mono), compute_rms(mono)),
                None => (Vec::new(), 0.0),
            };

            // Mark the phrase BEFORE the watchdogs run: at the tick a
            // long-pause phrase flushes, the silent-tick count may already be
            // past the stall limit, and the watchdog must not preempt the
            // delivery it is about to allow.
            if events.iter().any(
                |e| matches!(e, DictationEvent::PhraseReady(audio) if !audio.samples.is_empty()),
            ) {
                self.had_phrase = true;
            }

            self.track_signal(chunk.is_some(), chunk_rms);
            if let Flow::EndSession = self.check_watchdogs(chunk.is_some(), chunk_rms) {
                return;
            }

            self.level_tick += 1;
            if self.level_tick.is_multiple_of(2) {
                self.result_tx.send(AudioResult::AudioLevel(chunk_rms));
            }
            self.adapt_threshold(chunk.is_some(), chunk_rms);

            if self.live_preview
                && self.level_tick.is_multiple_of(PARTIAL_TICKS)
                && let Some(partial) = self.session.current_phrase()
                && !partial.samples.is_empty()
            {
                self.result_tx.send(AudioResult::PartialPhrase(partial));
            }

            if let Flow::EndSession = self.dispatch_events(events) {
                return;
            }
            std::thread::sleep(MONITOR_TICK);
        }
    }

    /// Handle pending commands. Every variant is handled explicitly:
    /// silently dropping a StartStreaming would leave its handshake waiting
    /// until timeout while the UI thinks a session is starting.
    fn handle_command(&mut self) -> Flow {
        match self.cmd_rx.try_recv() {
            Ok(Cmd::StartStreaming(rejected)) => {
                tracing::warn!("StartStreaming received while a session is active; rejecting");
                // Answer the rejected start's own generation, so its handshake
                // fails instead of picking up this session's queued results.
                self.result_tx.send_as(
                    rejected.generation,
                    AudioResult::StartFailed("A recording session is already active".to_string()),
                );
                Flow::Continue
            }
            Err(mpsc::TryRecvError::Empty) => Flow::Continue,
            Err(mpsc::TryRecvError::Disconnected) => {
                tracing::warn!("Command channel closed; stopping capture");
                self.finish_session("channel close")
            }
            Ok(Cmd::Stop) => {
                self.flush_remaining();
                self.finish_session("manual stop")
            }
            Ok(Cmd::SetWarm(params)) => {
                // Mid-session only the mode flag applies (so a disable
                // releases the mic at session end); a device or echo-
                // cancellation change retargets on the next session start.
                self.capture.set_warm_start(params.enabled);
                Flow::Continue
            }
            Ok(Cmd::Arm(params)) => {
                // A session owns the mic; the supervisor re-arms (via its
                // session-end sync) once this session finishes.
                tracing::debug!("Arm received while a session is active; rejecting");
                let _ = params.wake_tx.send(WakeEvent::Disarmed {
                    reason: "A recording session is already active".to_string(),
                    failed: false,
                });
                Flow::Continue
            }
            Ok(Cmd::Disarm) => {
                // Nothing armed during a session; the mode flag the
                // supervisor consults at session end governs re-arming.
                tracing::debug!("Disarm received while a session is active, ignoring");
                Flow::Continue
            }
        }
    }

    /// Feed any unanalyzed samples through the dictation session so the
    /// final phrase boundary matches the live path, then flush it.
    fn flush_remaining(&mut self) {
        let remaining = {
            let mut buf = self.live_buf.lock().unwrap_or_else(|e| e.into_inner());
            let mono = (buf.len() > self.analyzed_up_to)
                .then(|| downmix_to_mono(&buf[self.analyzed_up_to..], self.native_channels));
            buf.clear();
            mono
        };

        if let Some(mono) = remaining.map(|m| self.gain.apply(m)) {
            for event in self.session.ingest(&mono) {
                if let DictationEvent::PhraseReady(audio) = event
                    && !audio.samples.is_empty()
                {
                    self.result_tx.send(AudioResult::PhraseReady(audio));
                }
            }
        }
        if let Some(audio) = self.session.finish()
            && !audio.samples.is_empty()
        {
            self.result_tx.send(AudioResult::PhraseReady(audio));
        }
    }

    fn finish_session(&mut self, context: &str) -> Flow {
        stop_capture(self.capture, context);
        self.result_tx.send(AudioResult::StreamingDone);
        Flow::EndSession
    }

    /// Snapshot new samples under the lock, then release it before any heavy
    /// work (downmix, gain, VAD inference): the CPAL callback pushes through
    /// the same mutex from a realtime thread and must never wait on us.
    fn take_chunk(&mut self) -> Option<Vec<f32>> {
        let raw = {
            let mut buf = self.live_buf.lock().unwrap_or_else(|e| e.into_inner());
            if buf.len() <= self.analyzed_up_to {
                return None;
            }
            let snapshot = buf[self.analyzed_up_to..].to_vec();
            self.analyzed_up_to = buf.len();
            let dropped = trim_buffer_to_preroll(&mut buf, self.native_rate, self.native_channels);
            self.analyzed_up_to = self.analyzed_up_to.saturating_sub(dropped);
            snapshot
        };
        let mono = downmix_to_mono(&raw, self.native_channels);
        Some(self.gain.apply(mono))
    }

    fn track_signal(&mut self, saw_new_samples: bool, chunk_rms: f32) {
        if saw_new_samples && chunk_rms > SIGNAL_FLOOR_RMS && !self.saw_signal {
            self.saw_signal = true;
            self.result_tx.send(AudioResult::SignalDetected);
        }
        if saw_new_samples {
            self.consecutive_no_sample_ticks = 0;
        } else {
            self.consecutive_no_sample_ticks += 1;
        }
    }

    /// Detect a stream that stopped delivering samples or only delivers
    /// digital silence (permissions, muted device) and end the session
    /// with an actionable message instead of listening forever.
    fn check_watchdogs(&mut self, saw_new_samples: bool, chunk_rms: f32) -> Flow {
        // Wake silent abort: a false trigger means an open mic with nobody
        // talking — end quietly (no error; the stop cue is the feedback).
        if self.wake_abort_deadline.is_some() {
            let speech_started = self.saw_signal && self.session.is_mid_phrase() || self.had_phrase;
            if speech_started {
                self.wake_abort_deadline = None;
            } else if wake_abort_due(self.wake_abort_deadline, speech_started) {
                tracing::info!("Wake session heard no speech; aborting");
                self.flush_remaining();
                return self.finish_session("wake silent abort");
            }
        }

        if !self.saw_signal
            && Instant::now() >= self.startup_deadline
            && self.consecutive_no_sample_ticks >= 20
        {
            return self.fail_no_signal("Microphone stream stopped delivering audio samples.");
        }

        // After we have already heard audio, a sustained gap means the device
        // disconnected mid-session. Without this the session would hang until
        // the inactivity timeout, silently capturing nothing.
        if self.saw_signal && self.consecutive_no_sample_ticks >= MID_SESSION_STALL_TICKS {
            return self.fail_no_signal(
                "Microphone stopped delivering audio. The input device may have been disconnected or changed.",
            );
        }

        // A driver delivering only digital silence after a disconnect/mute
        // evades the no-sample check above. Only enforced before the first
        // phrase and never inside an unflushed one: some drivers noise-gate
        // speech pauses to digital zero (calibration on such devices still
        // reads a small nonzero floor, so the ambient level cannot tell the
        // cases apart), so once real speech has flowed, sustained zeros are a
        // natural pause and the inactivity session_timeout owns the session
        // end. With the timeout disabled (hands-free), zeros from a gating
        // driver are indistinguishable from a dead device; the session stays
        // open by design and the timeout log below is the only breadcrumb.
        if self.saw_signal {
            if saw_new_samples && chunk_rms <= 0.00005 {
                self.consecutive_silent_ticks += 1;
            } else if saw_new_samples {
                self.consecutive_silent_ticks = 0;
            }
            if should_fail_on_digital_silence(
                self.had_phrase,
                self.session.is_mid_phrase(),
                self.consecutive_silent_ticks,
            ) {
                return self.fail_no_signal(
                    "Microphone is delivering only silence. The input device may have been disconnected, muted, or changed.",
                );
            }
        }

        if !self.saw_signal
            && Instant::now() >= self.silence_deadline
            && saw_new_samples
            && chunk_rms <= 0.00005
            && self.effective_ambient <= 0.00005
        {
            return self.fail_no_signal(
                "Microphone stream is delivering digital silence. Check your microphone permissions, input device, mute switch, and input volume.",
            );
        }
        Flow::Continue
    }

    fn fail_no_signal(&mut self, msg: &str) -> Flow {
        tracing::warn!("{}", msg);
        self.result_tx.send(AudioResult::NoSignal(msg.to_string()));
        self.finish_session("no signal")
    }

    /// Rolling noise-floor refinement (auto-threshold mode only): if startup
    /// calibration overshot because the user was already talking, the
    /// quietest recent chunks pull the threshold back down. Never raises it.
    fn adapt_threshold(&mut self, saw_new_samples: bool, chunk_rms: f32) {
        if !self.auto_threshold || !saw_new_samples || chunk_rms <= 0.0 {
            return;
        }
        if self.floor_window.len() >= FLOOR_WINDOW_TICKS {
            let _ = self.floor_window.pop_front();
        }
        self.floor_window.push_back(chunk_rms);

        if !self.level_tick.is_multiple_of(20) || self.floor_window.len() < 20 {
            return;
        }
        let floor = self
            .floor_window
            .iter()
            .copied()
            .fold(f32::INFINITY, f32::min);
        let candidate = (floor * 1.8).clamp(0.001, 0.015);
        if candidate < self.current_threshold {
            tracing::info!(
                "Lowering speech threshold {:.6} -> {:.6} (rolling noise floor {:.6})",
                self.current_threshold,
                candidate,
                floor
            );
            self.current_threshold = candidate;
            self.session.set_speech_threshold(candidate);
            self.result_tx.send(AudioResult::SpeechThreshold(
                candidate.min(SIGNAL_FLOOR_RMS),
            ));
        }
    }

    fn dispatch_events(&mut self, events: Vec<DictationEvent>) -> Flow {
        for event in events {
            match event {
                DictationEvent::Level(_) => {}
                DictationEvent::ActivityDetected => {
                    if !self.saw_signal {
                        self.saw_signal = true;
                        self.result_tx.send(AudioResult::SignalDetected);
                    }
                }
                DictationEvent::PhraseReady(audio) => {
                    // had_phrase is set in run() before the watchdogs, not here.
                    if !audio.samples.is_empty() {
                        self.result_tx.send(AudioResult::PhraseReady(audio));
                    }
                }
                DictationEvent::SessionTimeout => {
                    // Distinguish an ordinary quiet timeout from one whose
                    // tail was digital silence: on a non-gating device the
                    // latter means the mic died mid-session, which the
                    // post-phrase watchdog no longer reports directly.
                    if self.consecutive_silent_ticks >= MID_SESSION_STALL_TICKS {
                        tracing::warn!(
                            silent_ticks = self.consecutive_silent_ticks,
                            "Streaming session ended by inactivity timeout with the mic delivering digital silence; the device may be muted or disconnected, or its driver gates pauses to zero"
                        );
                    } else {
                        tracing::info!(
                            "Streaming session ended: no speech within the inactivity timeout"
                        );
                    }
                    return self.finish_session("session timeout");
                }
            }
        }
        Flow::Continue
    }
}

/// Whether sustained digital silence should tear the session down. Before any
/// phrase and outside one, it still means a dead or muted device (keep the
/// actionable error). Inside an unflushed phrase, silence is an expected pause
/// (a long `phrase_pause` setting can hold a phrase open past the stall limit,
/// and killing there would discard captured speech). After real speech has
/// flowed, drivers that noise-gate pauses to digital zero make it
/// indistinguishable from a natural pause, so the inactivity session_timeout
/// owns the session end instead.
fn should_fail_on_digital_silence(
    had_phrase: bool,
    mid_phrase: bool,
    consecutive_silent_ticks: u32,
) -> bool {
    !had_phrase && !mid_phrase && consecutive_silent_ticks >= MID_SESSION_STALL_TICKS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn handle_with_results() -> (Handle, mpsc::Sender<Tagged>) {
        let (cmd_tx, _cmd_rx) = mpsc::channel::<Cmd>();
        let (result_tx, result_rx) = mpsc::channel::<Tagged>();
        (Handle::new(cmd_tx, result_rx), result_tx)
    }

    fn send(tx: &mpsc::Sender<Tagged>, generation: u64, result: AudioResult) {
        tx.send(Tagged { generation, result }).expect("send");
    }

    fn empty_phrase() -> AudioResult {
        AudioResult::PhraseReady(AudioBuffer {
            samples: Vec::new(),
            sample_rate: AudioBuffer::SAMPLE_RATE,
        })
    }

    /// The orphaned-session bug: a start whose handshake timed out can still
    /// come up and queue `Started` plus phrases. A later session must not read
    /// that `Started` as its own acknowledgement.
    #[test]
    fn start_handshake_ignores_an_orphaned_sessions_results() {
        let (handle, tx) = handle_with_results();
        send(&tx, 1, AudioResult::Started);
        send(&tx, 1, empty_phrase());
        send(
            &tx,
            2,
            AudioResult::StartFailed("A recording session is already active".to_string()),
        );

        let err = handle.await_started(2).expect_err("stale Started accepted");
        assert!(err.contains("already active"));
    }

    /// ...and the orphan's queued audio must never be delivered to the new
    /// session, which would type it into whatever window is focused now.
    #[test]
    fn recv_result_never_yields_another_sessions_audio() {
        let (handle, tx) = handle_with_results();
        send(&tx, 1, empty_phrase());
        send(&tx, 1, AudioResult::StreamingDone);
        send(&tx, 2, AudioResult::SignalDetected);

        assert!(matches!(
            handle.recv_result(2).expect("recv"),
            AudioResult::SignalDetected
        ));
    }

    #[test]
    fn worker_level_failures_reach_every_session() {
        let (handle, tx) = handle_with_results();
        send(
            &tx,
            ANY_GENERATION,
            AudioResult::StartFailed("Audio worker crash".to_string()),
        );
        assert!(
            handle
                .await_started(7)
                .expect_err("crash not surfaced")
                .contains("crash")
        );

        assert!(accepts(ANY_GENERATION, 7));
        assert!(accepts(7, 7));
        assert!(!accepts(6, 7));
    }

    #[test]
    fn digital_silence_fails_only_before_first_phrase() {
        assert!(should_fail_on_digital_silence(
            false,
            false,
            MID_SESSION_STALL_TICKS
        ));
        assert!(should_fail_on_digital_silence(
            false,
            false,
            MID_SESSION_STALL_TICKS + 1
        ));
        // A gating driver zeroing a pause after real speech must not kill
        // the session; the inactivity timeout handles it.
        assert!(!should_fail_on_digital_silence(
            true,
            false,
            MID_SESSION_STALL_TICKS
        ));
        assert!(!should_fail_on_digital_silence(true, false, u32::MAX));
    }

    #[test]
    fn digital_silence_never_fails_inside_an_unflushed_phrase() {
        // A phrase_pause above the ~3s stall limit keeps the phrase open
        // through a longer gated pause; killing there would discard the
        // user's captured speech before it ever flushed.
        assert!(!should_fail_on_digital_silence(
            false,
            true,
            MID_SESSION_STALL_TICKS
        ));
        assert!(!should_fail_on_digital_silence(false, true, u32::MAX));
    }

    #[test]
    fn armed_watchdog_tolerates_brief_gaps_only() {
        assert!(!armed_should_give_up(0));
        assert!(!armed_should_give_up(ARMED_STALL_TICKS - 1));
        assert!(armed_should_give_up(ARMED_STALL_TICKS));
    }

    #[test]
    fn wake_abort_only_before_speech_and_after_deadline() {
        let past = Some(Instant::now() - Duration::from_millis(1));
        let future = Some(Instant::now() + Duration::from_secs(60));
        assert!(wake_abort_due(past, false));
        assert!(!wake_abort_due(past, true)); // speech started — never abort
        assert!(!wake_abort_due(future, false)); // window still open
        assert!(!wake_abort_due(None, false)); // hotkey session — no deadline
    }

    #[test]
    fn wake_rewind_window_covers_lag_plus_handoff_and_is_capped() {
        let lag = Duration::from_millis(WAKE_LAG_COMPENSATION_MS);
        // Instant handoff: keep exactly the lag compensation.
        assert_eq!(wake_keep_duration(Duration::ZERO), lag);
        // 100 ms supervisor round trip: the anchor stays at detection-lag.
        assert_eq!(
            wake_keep_duration(Duration::from_millis(100)),
            lag + Duration::from_millis(100)
        );
        // Never ask for more than the ~1 s the live buffer retains.
        assert_eq!(
            wake_keep_duration(Duration::from_secs(30)),
            Duration::from_secs(1)
        );
    }

    #[test]
    fn wake_rewind_sample_count_is_channel_aligned() {
        // 48 kHz stereo, 240 ms → 48000 * 0.24 = 11520 frames * 2 channels.
        let keep = wake_keep_samples(Duration::from_millis(240), 48_000, 2);
        assert_eq!(keep, 23_040);
        assert_eq!(keep % 2, 0);
        // Mono stays frame-exact.
        assert_eq!(
            wake_keep_samples(Duration::from_millis(240), 16_000, 1),
            3_840
        );
    }

    /// The armed-capture regression: hotkey sessions inherited the passive
    /// (raw CPAL) stream and lost echo cancellation. Explicit starts must
    /// reopen the device; only wake starts may reuse the stream.
    #[test]
    fn hotkey_start_while_armed_reopens_the_device() {
        assert_eq!(
            armed_start_route(StartReason::Hotkey),
            ArmedStartRoute::ReopenDevice
        );
    }

    #[test]
    fn wake_start_while_armed_keeps_the_passive_stream_for_preroll() {
        assert_eq!(
            armed_start_route(StartReason::WakeWord),
            ArmedStartRoute::ReusePassiveStream
        );
    }

    #[test]
    fn digital_silence_tolerated_below_stall_threshold() {
        assert!(!should_fail_on_digital_silence(
            false,
            false,
            MID_SESSION_STALL_TICKS - 1
        ));
        assert!(!should_fail_on_digital_silence(false, false, 0));
        assert!(!should_fail_on_digital_silence(true, false, 0));
    }
}
