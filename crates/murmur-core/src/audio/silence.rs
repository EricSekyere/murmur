use std::time::{Duration, Instant};

/// States of the silence detection state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SilenceState {
    /// Waiting for the user to start speaking (RMS below threshold).
    WaitingForSpeech,
    /// Active speech detected (RMS above threshold).
    SpeechDetected,
    /// Speech stopped, timing silence duration.
    SilenceAfterSpeech,
    /// Silence lasted longer than the timeout — auto-stop should trigger.
    Done,
}

/// RMS-based silence detector with a simple state machine.
///
/// Feed raw audio chunks via [`feed`]. The detector transitions through:
/// `WaitingForSpeech -> SpeechDetected -> SilenceAfterSpeech -> Done`
///
/// If speech resumes during `SilenceAfterSpeech`, it returns to `SpeechDetected`.
pub struct SilenceDetector {
    rms_threshold: f32,
    silence_timeout: Duration,
    state: SilenceState,
    silence_start: Option<Instant>,
}

impl SilenceDetector {
    pub fn new(rms_threshold: f32, silence_timeout: Duration) -> Self {
        Self {
            rms_threshold,
            silence_timeout,
            state: SilenceState::WaitingForSpeech,
            silence_start: None,
        }
    }

    /// Feed a chunk of raw audio samples and advance the state machine.
    ///
    /// Returns the current state after processing.
    pub fn feed(&mut self, samples: &[f32]) -> SilenceState {
        if self.state == SilenceState::Done {
            return SilenceState::Done;
        }

        if samples.is_empty() {
            return self.state;
        }

        let rms = compute_rms(samples);
        let is_speech = rms >= self.rms_threshold;

        match self.state {
            SilenceState::WaitingForSpeech => {
                if is_speech {
                    self.state = SilenceState::SpeechDetected;
                    self.silence_start = None;
                }
            }
            SilenceState::SpeechDetected => {
                if !is_speech {
                    self.state = SilenceState::SilenceAfterSpeech;
                    self.silence_start = Some(Instant::now());
                }
            }
            SilenceState::SilenceAfterSpeech => {
                if is_speech {
                    self.state = SilenceState::SpeechDetected;
                    self.silence_start = None;
                } else if let Some(start) = self.silence_start
                    && start.elapsed() >= self.silence_timeout
                {
                    self.state = SilenceState::Done;
                }
            }
            SilenceState::Done => {}
        }

        self.state
    }

    /// Reset the detector to its initial state.
    pub fn reset(&mut self) {
        self.state = SilenceState::WaitingForSpeech;
        self.silence_start = None;
    }

    /// Get the current state without advancing.
    pub fn state(&self) -> SilenceState {
        self.state
    }
}

// ─── PhraseDetector ──────────────────────────────────────────────────────────

/// States of the phrase detection state machine (streaming mode).
///
/// Unlike `SilenceDetector` which terminates at `Done`, `PhraseDetector`
/// cycles through phrases: `WaitingForSpeech → InSpeech → TrailingSilence →
/// PhraseComplete → (reset) → WaitingForSpeech`. `SessionTimeout` is terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhraseState {
    /// No speech detected yet (or between phrases after reset).
    WaitingForSpeech,
    /// Active speech detected.
    InSpeech,
    /// Speech paused — waiting to confirm phrase boundary.
    TrailingSilence,
    /// Phrase boundary confirmed — caller should drain & transcribe the chunk.
    PhraseComplete,
    /// No speech for `session_timeout` — session is done.
    SessionTimeout,
}

/// RMS-based phrase detector for streaming transcription.
///
/// Detects phrase boundaries via silence pauses (`phrase_pause`) and ends the
/// session after prolonged inactivity (`session_timeout`).
///
/// After handling `PhraseComplete`, call `reset_phrase()` to cycle back to
/// `WaitingForSpeech` for the next phrase. `SessionTimeout` is terminal.
pub struct PhraseDetector {
    rms_threshold: f32,
    phrase_pause: Duration,
    session_timeout: Duration,
    state: PhraseState,
    silence_start: Option<Instant>,
    last_speech: Instant,
}

impl PhraseDetector {
    pub fn new(rms_threshold: f32, phrase_pause: Duration, session_timeout: Duration) -> Self {
        Self {
            rms_threshold,
            phrase_pause,
            session_timeout,
            state: PhraseState::WaitingForSpeech,
            silence_start: None,
            last_speech: Instant::now(),
        }
    }

    /// Feed a chunk of raw audio samples and advance the state machine.
    pub fn feed(&mut self, samples: &[f32]) -> PhraseState {
        if self.state == PhraseState::SessionTimeout || self.state == PhraseState::PhraseComplete {
            return self.state;
        }

        if samples.is_empty() {
            return self.state;
        }

        let rms = compute_rms(samples);
        let is_speech = rms >= self.rms_threshold;

        tracing::trace!(
            rms = format!("{:.6}", rms),
            threshold = format!("{:.6}", self.rms_threshold),
            is_speech,
            state = ?self.state,
            "PhraseDetector::feed"
        );

        if is_speech {
            self.last_speech = Instant::now();
        }

        let prev_state = self.state;

        match self.state {
            PhraseState::WaitingForSpeech => {
                if is_speech {
                    self.state = PhraseState::InSpeech;
                    self.silence_start = None;
                } else if self.last_speech.elapsed() >= self.session_timeout {
                    self.state = PhraseState::SessionTimeout;
                }
            }
            PhraseState::InSpeech => {
                if !is_speech {
                    self.state = PhraseState::TrailingSilence;
                    self.silence_start = Some(Instant::now());
                }
            }
            PhraseState::TrailingSilence => {
                if is_speech {
                    self.state = PhraseState::InSpeech;
                    self.silence_start = None;
                } else if let Some(start) = self.silence_start
                    && start.elapsed() >= self.phrase_pause
                {
                    self.state = PhraseState::PhraseComplete;
                }
            }
            PhraseState::PhraseComplete | PhraseState::SessionTimeout => {}
        }

        if self.state != prev_state {
            tracing::debug!(
                rms = format!("{:.6}", rms),
                from = ?prev_state,
                to = ?self.state,
                "PhraseDetector state transition"
            );
        }

        self.state
    }

    /// Reset after handling `PhraseComplete` to start detecting the next phrase.
    pub fn reset_phrase(&mut self) {
        self.state = PhraseState::WaitingForSpeech;
        self.silence_start = None;
        // last_speech keeps its value so session timeout tracks total inactivity
    }

    /// Get the current state without advancing.
    pub fn state(&self) -> PhraseState {
        self.state
    }
}

/// Compute the root-mean-square of a slice of audio samples.
pub fn compute_rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f32 = samples.iter().map(|&s| s * s).sum();
    (sum_sq / samples.len() as f32).sqrt()
}

/// Downmix interleaved multi-channel audio to mono by averaging frames.
pub fn downmix_to_mono(samples: &[f32], channels: u16) -> Vec<f32> {
    if channels <= 1 {
        return samples.to_vec();
    }
    let ch = channels as usize;
    samples
        .chunks_exact(ch)
        .map(|frame| frame.iter().sum::<f32>() / ch as f32)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_speech(len: usize) -> Vec<f32> {
        vec![0.1; len]
    }

    fn make_silence(len: usize) -> Vec<f32> {
        vec![0.001; len]
    }

    #[test]
    fn starts_in_waiting() {
        let det = SilenceDetector::new(0.015, Duration::from_millis(500));
        assert_eq!(det.state(), SilenceState::WaitingForSpeech);
    }

    #[test]
    fn detects_speech() {
        let mut det = SilenceDetector::new(0.015, Duration::from_millis(500));
        let state = det.feed(&make_speech(160));
        assert_eq!(state, SilenceState::SpeechDetected);
    }

    #[test]
    fn stays_waiting_on_silence() {
        let mut det = SilenceDetector::new(0.015, Duration::from_millis(500));
        let state = det.feed(&make_silence(160));
        assert_eq!(state, SilenceState::WaitingForSpeech);
    }

    #[test]
    fn transitions_to_silence_after_speech() {
        let mut det = SilenceDetector::new(0.015, Duration::from_millis(500));
        det.feed(&make_speech(160));
        let state = det.feed(&make_silence(160));
        assert_eq!(state, SilenceState::SilenceAfterSpeech);
    }

    #[test]
    fn speech_resumes_from_silence() {
        let mut det = SilenceDetector::new(0.015, Duration::from_millis(500));
        det.feed(&make_speech(160));
        det.feed(&make_silence(160));
        let state = det.feed(&make_speech(160));
        assert_eq!(state, SilenceState::SpeechDetected);
    }

    #[test]
    fn reaches_done_after_timeout() {
        let mut det = SilenceDetector::new(0.015, Duration::from_millis(50));
        det.feed(&make_speech(160));
        det.feed(&make_silence(160));
        std::thread::sleep(Duration::from_millis(60));
        let state = det.feed(&make_silence(160));
        assert_eq!(state, SilenceState::Done);
    }

    #[test]
    fn stays_done_once_reached() {
        let mut det = SilenceDetector::new(0.015, Duration::from_millis(50));
        det.feed(&make_speech(160));
        det.feed(&make_silence(160));
        std::thread::sleep(Duration::from_millis(60));
        det.feed(&make_silence(160));
        let state = det.feed(&make_speech(160));
        assert_eq!(state, SilenceState::Done);
    }

    #[test]
    fn reset_works() {
        let mut det = SilenceDetector::new(0.015, Duration::from_millis(50));
        det.feed(&make_speech(160));
        det.reset();
        assert_eq!(det.state(), SilenceState::WaitingForSpeech);
    }

    #[test]
    fn empty_samples_no_change() {
        let mut det = SilenceDetector::new(0.015, Duration::from_millis(500));
        let state = det.feed(&[]);
        assert_eq!(state, SilenceState::WaitingForSpeech);
    }

    #[test]
    fn compute_rms_basic() {
        assert!((compute_rms(&[0.5, -0.5, 0.5, -0.5]) - 0.5).abs() < 0.001);
        assert_eq!(compute_rms(&[]), 0.0);
    }

    #[test]
    fn downmix_mono_passthrough() {
        let samples = vec![0.1, 0.2, 0.3];
        let mono = downmix_to_mono(&samples, 1);
        assert_eq!(mono, samples);
    }

    #[test]
    fn downmix_stereo_averages() {
        // Stereo: L=0.4, R=0.0, L=0.2, R=0.0
        let stereo = vec![0.4, 0.0, 0.2, 0.0];
        let mono = downmix_to_mono(&stereo, 2);
        assert_eq!(mono.len(), 2);
        assert!((mono[0] - 0.2).abs() < 0.001);
        assert!((mono[1] - 0.1).abs() < 0.001);
    }

    #[test]
    fn downmix_fixes_rms_for_single_channel_mic() {
        // Simulate a mono mic captured as stereo (one channel has audio, other silent)
        let speech_val = 0.001f32; // very quiet mic
        let stereo: Vec<f32> = (0..160).flat_map(|_| [speech_val, 0.0]).collect();

        let raw_rms = compute_rms(&stereo);
        let mono = downmix_to_mono(&stereo, 2);
        let mono_rms = compute_rms(&mono);

        // Mono RMS should be half (averaged), but raw RMS is diluted by zeros
        // Raw: sqrt(mean(speech² + 0²)) = speech / sqrt(2) ≈ 0.000707
        // Mono: sqrt(mean((speech/2)²)) = speech / 2 = 0.0005
        // The key point: both are small, but after downmix we get correct representation
        assert!(raw_rms > 0.0);
        assert!(mono_rms > 0.0);
        // Mono RMS of averaged frames = speech_val / 2
        assert!((mono_rms - speech_val / 2.0).abs() < 0.0001);
    }

    // ─── PhraseDetector Tests ────────────────────────────────────────────────

    #[test]
    fn phrase_starts_waiting() {
        let det = PhraseDetector::new(0.015, Duration::from_millis(500), Duration::from_secs(5));
        assert_eq!(det.state(), PhraseState::WaitingForSpeech);
    }

    #[test]
    fn phrase_detects_speech() {
        let mut det =
            PhraseDetector::new(0.015, Duration::from_millis(500), Duration::from_secs(5));
        let state = det.feed(&make_speech(160));
        assert_eq!(state, PhraseState::InSpeech);
    }

    #[test]
    fn phrase_trailing_silence() {
        let mut det =
            PhraseDetector::new(0.015, Duration::from_millis(500), Duration::from_secs(5));
        det.feed(&make_speech(160));
        let state = det.feed(&make_silence(160));
        assert_eq!(state, PhraseState::TrailingSilence);
    }

    #[test]
    fn phrase_complete_after_pause() {
        let mut det = PhraseDetector::new(0.015, Duration::from_millis(50), Duration::from_secs(5));
        det.feed(&make_speech(160));
        det.feed(&make_silence(160));
        std::thread::sleep(Duration::from_millis(60));
        let state = det.feed(&make_silence(160));
        assert_eq!(state, PhraseState::PhraseComplete);
    }

    #[test]
    fn phrase_reset_cycles() {
        let mut det = PhraseDetector::new(0.015, Duration::from_millis(50), Duration::from_secs(5));
        det.feed(&make_speech(160));
        det.feed(&make_silence(160));
        std::thread::sleep(Duration::from_millis(60));
        det.feed(&make_silence(160));
        assert_eq!(det.state(), PhraseState::PhraseComplete);

        det.reset_phrase();
        assert_eq!(det.state(), PhraseState::WaitingForSpeech);

        // Can detect a second phrase
        let state = det.feed(&make_speech(160));
        assert_eq!(state, PhraseState::InSpeech);
    }

    #[test]
    fn phrase_session_timeout() {
        let mut det =
            PhraseDetector::new(0.015, Duration::from_millis(500), Duration::from_millis(50));
        // No speech at all — wait for session timeout
        std::thread::sleep(Duration::from_millis(60));
        let state = det.feed(&make_silence(160));
        assert_eq!(state, PhraseState::SessionTimeout);
    }

    #[test]
    fn phrase_speech_resumes_from_trailing() {
        let mut det =
            PhraseDetector::new(0.015, Duration::from_millis(500), Duration::from_secs(5));
        det.feed(&make_speech(160));
        det.feed(&make_silence(160));
        assert_eq!(det.state(), PhraseState::TrailingSilence);

        let state = det.feed(&make_speech(160));
        assert_eq!(state, PhraseState::InSpeech);
    }
}
