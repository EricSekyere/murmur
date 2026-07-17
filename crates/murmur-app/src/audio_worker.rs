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

#[derive(Clone)]
pub(crate) struct StartParams {
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

enum Cmd {
    StartStreaming(StartParams),
    Stop,
    SetWarm(WarmParams),
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

pub(crate) struct Handle {
    cmd_tx: mpsc::Sender<Cmd>,
    result_rx: Mutex<mpsc::Receiver<AudioResult>>,
}

impl Handle {
    pub fn spawn(app_handle: tauri::AppHandle) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel::<Cmd>();
        let (result_tx, result_rx) = mpsc::channel::<AudioResult>();

        std::thread::spawn(move || {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_worker(&cmd_rx, &result_tx);
            }));
            if let Err(panic_info) = outcome {
                let msg = panic_message(panic_info, "unknown panic in audio worker thread");
                tracing::error!("Audio worker thread panicked: {}", msg);
                let _ = result_tx.send(AudioResult::StartFailed(format!(
                    "Audio worker crash: {}",
                    msg
                )));
                emit_hotkey_error(&app_handle, &format!("Audio driver crashed: {}", msg));
            }
        });

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

    /// Block until the queued StartStreaming command is acknowledged.
    pub fn await_started(&self) -> Result<(), String> {
        let rx = self.result_rx.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            match rx.recv_timeout(Duration::from_secs(5)) {
                Ok(AudioResult::Started) => return Ok(()),
                Ok(AudioResult::StartFailed(e)) => return Err(e),
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

    /// Blocking receive for the next streaming result.
    pub fn recv_result(&self) -> Result<AudioResult, String> {
        let rx = self.result_rx.lock().unwrap_or_else(|e| e.into_inner());
        rx.recv_timeout(Duration::from_secs(120))
            .map_err(|e| format!("Audio worker recv timeout: {}", e))
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

fn run_worker(cmd_rx: &mpsc::Receiver<Cmd>, result_tx: &mpsc::Sender<AudioResult>) {
    let mut capture = match AudioCapture::new() {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("Failed to create AudioCapture: {}", e);
            let _ = result_tx.send(AudioResult::StartFailed(e.to_string()));
            return;
        }
    };

    while let Ok(cmd) = cmd_rx.recv() {
        match cmd {
            Cmd::StartStreaming(params) => {
                // Contain a panic inside a single session (a driver or VAD bug)
                // so it ends that session instead of unwinding the whole worker
                // thread — which would wedge every future session with a closed
                // channel. The thread-level catch in Handle::spawn remains a
                // last resort for a panic outside a session.
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run_session(&mut capture, &params, cmd_rx, result_tx);
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
                    let _ = result_tx.send(AudioResult::StreamingDone);
                }
            }
            Cmd::Stop => {
                tracing::debug!("Stop received outside monitoring loop, ignoring");
            }
            Cmd::SetWarm(params) => apply_warm(&mut capture, &params),
        }
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
    result_tx: &mpsc::Sender<AudioResult>,
) {
    // Sync the warm mode with the setting each session, so a change made
    // while no SetWarm command was processed still applies here.
    capture.set_warm_start(params.mic_warm_start);
    if let Err(msg) = start_capture(
        capture,
        params.audio_device.as_deref(),
        params.echo_cancellation,
    ) {
        let _ = result_tx.send(AudioResult::StartFailed(msg));
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
        let _ = result_tx.send(AudioResult::StartFailed(msg));
        return;
    }
    let _ = result_tx.send(AudioResult::Started);

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
    let _ = result_tx.send(AudioResult::SpeechThreshold(
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
        mic_gain: calibration.mic_gain,
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
    result_tx: &'a mpsc::Sender<AudioResult>,
    capture: &'a mut AudioCapture,
    live_buf: Arc<Mutex<Vec<f32>>>,
    session: DictationSession,
    native_rate: u32,
    native_channels: u16,
    analyzed_up_to: usize,
    mic_gain: f32,
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
                let _ = self.result_tx.send(AudioResult::AudioLevel(chunk_rms));
            }
            self.adapt_threshold(chunk.is_some(), chunk_rms);

            if self.live_preview
                && self.level_tick.is_multiple_of(PARTIAL_TICKS)
                && let Some(partial) = self.session.current_phrase()
                && !partial.samples.is_empty()
            {
                let _ = self.result_tx.send(AudioResult::PartialPhrase(partial));
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
            Ok(Cmd::StartStreaming(_)) => {
                tracing::warn!("StartStreaming received while a session is active; rejecting");
                let _ = self.result_tx.send(AudioResult::StartFailed(
                    "A recording session is already active".to_string(),
                ));
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
            mono.map(|m| self.apply_gain(m))
        };

        if let Some(mono) = remaining {
            for event in self.session.ingest(&mono) {
                if let DictationEvent::PhraseReady(audio) = event
                    && !audio.samples.is_empty()
                {
                    let _ = self.result_tx.send(AudioResult::PhraseReady(audio));
                }
            }
        }
        if let Some(audio) = self.session.finish()
            && !audio.samples.is_empty()
        {
            let _ = self.result_tx.send(AudioResult::PhraseReady(audio));
        }
    }

    fn finish_session(&mut self, context: &str) -> Flow {
        stop_capture(self.capture, context);
        let _ = self.result_tx.send(AudioResult::StreamingDone);
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
        Some(self.apply_gain(mono))
    }

    fn apply_gain(&self, mono: Vec<f32>) -> Vec<f32> {
        if self.mic_gain <= 1.0 {
            return mono;
        }
        mono.iter()
            .map(|s| (s * self.mic_gain).clamp(-1.0, 1.0))
            .collect()
    }

    fn track_signal(&mut self, saw_new_samples: bool, chunk_rms: f32) {
        if saw_new_samples && chunk_rms > SIGNAL_FLOOR_RMS && !self.saw_signal {
            self.saw_signal = true;
            let _ = self.result_tx.send(AudioResult::SignalDetected);
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
        let _ = self.result_tx.send(AudioResult::NoSignal(msg.to_string()));
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
            let _ = self.result_tx.send(AudioResult::SpeechThreshold(
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
                        let _ = self.result_tx.send(AudioResult::SignalDetected);
                    }
                }
                DictationEvent::PhraseReady(audio) => {
                    // had_phrase is set in run() before the watchdogs, not here.
                    if !audio.samples.is_empty() {
                        let _ = self.result_tx.send(AudioResult::PhraseReady(audio));
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
