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

#[derive(Clone)]
pub(crate) struct StartParams {
    pub audio_device: Option<String>,
    pub rms_threshold: f32,
    pub vad_threshold: f32,
    pub phrase_pause: Duration,
    pub session_timeout: Duration,
    /// Emit periodic `PartialPhrase` snapshots for live preview.
    pub live_preview: bool,
}

enum Cmd {
    StartStreaming(StartParams),
    Stop,
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
    pub fn send_start(&self, params: StartParams) -> Result<(), String> {
        // Drain stale results so they cannot race the next handshake.
        if let Ok(rx) = self.result_rx.lock() {
            while rx.try_recv().is_ok() {}
        }

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
                run_session(&mut capture, &params, cmd_rx, result_tx);
            }
            Cmd::Stop => {
                tracing::debug!("Stop received outside monitoring loop, ignoring");
            }
        }
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
    if let Err(msg) = start_capture(capture, params.audio_device.as_deref()) {
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
        let msg = "Microphone opened but produced no audio samples. Check Windows microphone privacy permissions and selected input device.".to_string();
        tracing::error!("{}", msg);
        stop_capture(capture, "startup sample probe failure");
        let _ = result_tx.send(AudioResult::StartFailed(msg));
        return;
    }
    let _ = result_tx.send(AudioResult::Started);

    let mut analyzed_up_to = 0usize;
    let calibration = calibrate(
        &live_buf,
        &mut analyzed_up_to,
        native_channels,
        native_rate,
        params.rms_threshold,
    );
    keep_calibration_preroll(&live_buf, &mut analyzed_up_to, native_rate, native_channels);

    let session = build_session(params, &calibration, native_rate);

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
        consecutive_no_sample_ticks: 0,
        startup_deadline: Instant::now() + Duration::from_millis(1200),
        silence_deadline: Instant::now() + Duration::from_secs(3),
        live_preview: params.live_preview,
    }
    .run();
}

/// CPAL's native backend (WASAPI) can panic on some drivers â€” contain it.
fn start_capture(capture: &mut AudioCapture, device: Option<&str>) -> Result<(), String> {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| capture.start(device)));
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

/// Some WASAPI paths report success but never produce callbacks â€” wait for
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
    consecutive_no_sample_ticks: u32,
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
                tracing::warn!("StartStreaming received while a session is active â€” rejecting");
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
            self.analyzed_up_to -= dropped;
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
        if saw_new_samples && chunk_rms > 0.002 && !self.saw_signal {
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

        if !self.saw_signal
            && Instant::now() >= self.silence_deadline
            && saw_new_samples
            && chunk_rms <= 0.00005
            && self.effective_ambient <= 0.00005
        {
            return self.fail_no_signal(
                "Microphone stream is delivering digital silence. Check Windows microphone permissions, input device, mute switch, and input volume.",
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
                    if !audio.samples.is_empty() {
                        let _ = self.result_tx.send(AudioResult::PhraseReady(audio));
                    }
                }
                DictationEvent::SessionTimeout => {
                    tracing::info!("Streaming session timeout â€” no speech");
                    return self.finish_session("session timeout");
                }
            }
        }
        Flow::Continue
    }
}
