use crate::audio::AudioBuffer;
use crate::audio::silence::compute_rms;
#[cfg(feature = "vad")]
use crate::audio::vad::{SILERO_FRAME_SAMPLES, VadState, VoiceActivityDetector};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy)]
pub struct DictationConfig {
    pub speech_threshold: f32,
    pub silence_hold: Duration,
    pub min_phrase: Duration,
    /// Soft target for phrase length during continuous speech. When the
    /// phrase buffer reaches this size, the session looks for an
    /// energy-minimum frame within `split_search` to break on; if it
    /// can't find one before `max_phrase + split_search`, it hard-cuts.
    pub max_phrase: Duration,
    /// Window searched for an energy-minimum frame around `max_phrase`.
    /// Larger window = more chance of finding a clean break, but more
    /// latency before flushing during continuous speech.
    pub split_search: Duration,
    pub preroll: Duration,
    pub session_timeout: Duration,
}

impl Default for DictationConfig {
    fn default() -> Self {
        Self {
            speech_threshold: 0.010,
            silence_hold: Duration::from_millis(700),
            min_phrase: Duration::from_millis(150),
            // Stay under whisper's 30s window. Long enough to keep
            // sentences intact without choppy 3s splits in the middle of
            // a thought.
            max_phrase: Duration::from_secs(20),
            // Search ~2s for a quiet frame to split on. Mirrors the
            // pattern used by transcribe-rs/transcriber/energy_adaptive_chunked.
            split_search: Duration::from_millis(2000),
            preroll: Duration::from_millis(350),
            session_timeout: Duration::from_secs(60),
        }
    }
}

/// Frame size used to score energy when searching for a split point.
/// 30ms @ 16 kHz; matches transcribe-rs's `frame_size = 480`.
const SPLIT_FRAME_MS: u64 = 30;

/// Trailing window of an in-progress phrase used for live preview snapshots.
const PREVIEW_WINDOW_SECS: u64 = 12;

#[derive(Debug, Clone)]
pub enum DictationEvent {
    Level(f32),
    ActivityDetected,
    PhraseReady(AudioBuffer),
    SessionTimeout,
}

/// A simpler streaming dictation state machine for live transcription.
///
/// It treats chunks above `speech_threshold` as active speech, keeps a small
/// preroll while idle, flushes phrases on sustained silence, and proactively
/// emits long phrases during continuous speech so dictation feels live.
pub struct DictationSession {
    config: DictationConfig,
    native_rate: u32,
    preroll_samples: usize,
    silence_hold_samples: usize,
    min_phrase_samples: usize,
    max_phrase_samples: usize,
    /// Width of the energy-minimum search window (in samples) used when the
    /// buffer exceeds `max_phrase` and we have to force a split.
    split_search_samples: usize,
    /// Frame size in samples for energy scoring during split-point search.
    split_frame_samples: usize,
    preroll: VecDeque<f32>,
    phrase_samples: Vec<f32>,
    in_speech: bool,
    silence_run_samples: usize,
    last_activity: Instant,
    signaled_activity: bool,
    /// Consecutive speech-positive chunks seen while idle. With VAD attached,
    /// a phrase only starts after `SPEECH_ONSET_CHUNKS` consecutive positives
    /// (~100ms of sustained speech) so a single sigh/breath/click frame can't
    /// open a phrase. The deferred audio stays in the preroll, so nothing is
    /// lost when the onset is confirmed.
    onset_streak: u32,
    /// Optional Silero VAD. When present, speech detection uses the model's
    /// probability instead of RMS thresholding — much more accurate, but
    /// adds ~1ms per 32ms frame on CPU. Falls back to RMS gracefully when
    /// `None` so the session still works without a model file.
    #[cfg(feature = "vad")]
    vad: Option<VoiceActivityDetector>,
    /// Buffer of resampled-to-16kHz mono audio that hasn't yet been chunked
    /// into Silero's 512-sample frames. Only populated when `vad` is Some.
    #[cfg(feature = "vad")]
    vad_pending: Vec<f32>,
}

impl DictationSession {
    pub fn new(config: DictationConfig, native_rate: u32) -> Self {
        let sample_count = |duration: Duration| -> usize {
            ((duration.as_secs_f64() * native_rate as f64).round() as usize).max(1)
        };

        let split_frame_samples = ((SPLIT_FRAME_MS * native_rate as u64) / 1000).max(1) as usize;

        Self {
            config,
            native_rate,
            preroll_samples: sample_count(config.preroll),
            silence_hold_samples: sample_count(config.silence_hold),
            min_phrase_samples: sample_count(config.min_phrase),
            max_phrase_samples: sample_count(config.max_phrase),
            split_search_samples: sample_count(config.split_search),
            split_frame_samples,
            preroll: VecDeque::new(),
            phrase_samples: Vec::new(),
            in_speech: false,
            silence_run_samples: 0,
            last_activity: Instant::now(),
            signaled_activity: false,
            onset_streak: 0,
            #[cfg(feature = "vad")]
            vad: None,
            #[cfg(feature = "vad")]
            vad_pending: Vec::new(),
        }
    }

    /// Attach a Silero VAD detector. When attached, the session uses the
    /// model's speech probability for phrase boundary detection instead of
    /// raw RMS — better at distinguishing breath/keyboard clicks from real
    /// speech. RMS is still used to drive the audio level UI.
    #[cfg(feature = "vad")]
    pub fn with_vad(mut self, vad: VoiceActivityDetector) -> Self {
        self.vad = Some(vad);
        self.vad_pending.reserve(SILERO_FRAME_SAMPLES * 4);
        self
    }

    /// Update the RMS speech threshold mid-session. Used by callers that
    /// refine their noise-floor estimate while the session runs (e.g. when
    /// startup calibration was contaminated by the user already speaking).
    /// Only affects the RMS fallback path; Silero VAD decisions are
    /// unaffected.
    pub fn set_speech_threshold(&mut self, threshold: f32) {
        self.config.speech_threshold = threshold;
    }

    pub fn ingest(&mut self, mono_samples: &[f32]) -> Vec<DictationEvent> {
        let mut events = Vec::new();
        if mono_samples.is_empty() {
            return events;
        }

        let rms = compute_rms(mono_samples);
        events.push(DictationEvent::Level(rms));

        // Speech decision: prefer Silero VAD when present, RMS otherwise.
        // Silero is much more robust to laptop-mic noise floors and to
        // transient noises (breaths, keyboard clicks) that fool RMS.
        #[cfg(feature = "vad")]
        let is_speech = if self.vad.is_some() {
            self.vad_chunk_is_speech(mono_samples).unwrap_or_else(|| {
                tracing::trace!("VAD returned no decision, falling back to RMS for this chunk");
                rms >= self.config.speech_threshold
            })
        } else {
            rms >= self.config.speech_threshold
        };

        #[cfg(not(feature = "vad"))]
        let is_speech = rms >= self.config.speech_threshold;

        if is_speech {
            self.last_activity = Instant::now();
        }

        if !self.in_speech {
            if is_speech {
                self.onset_streak += 1;
                if self.onset_streak >= self.onset_chunks_required() {
                    self.onset_streak = 0;
                    self.in_speech = true;
                    self.silence_run_samples = 0;
                    self.phrase_samples.extend(self.preroll.drain(..));
                    self.phrase_samples.extend_from_slice(mono_samples);
                    if !self.signaled_activity {
                        self.signaled_activity = true;
                        events.push(DictationEvent::ActivityDetected);
                    }
                } else {
                    // Possible onset, not yet confirmed — keep the audio in
                    // the preroll so it lands in the phrase if confirmed.
                    self.push_preroll(mono_samples);
                }
            } else {
                self.onset_streak = 0;
                self.push_preroll(mono_samples);
                if !self.config.session_timeout.is_zero()
                    && self.last_activity.elapsed() >= self.config.session_timeout
                {
                    events.push(DictationEvent::SessionTimeout);
                }
            }

            return events;
        }

        self.phrase_samples.extend_from_slice(mono_samples);

        if is_speech {
            self.silence_run_samples = 0;
            // Don't hard-cut at exactly max_phrase — a 3s cut almost always
            // lands mid-word. Wait until we've accumulated enough audio to
            // search for a low-energy frame inside `split_search`, then
            // split there. Hard-cut only as a last resort if speech keeps
            // coming for `max_phrase + split_search` samples.
            let target = self.max_phrase_samples + self.split_search_samples;
            if self.phrase_samples.len() >= target
                && let Some(buffer) = self.flush_phrase_at_split()
            {
                events.push(DictationEvent::PhraseReady(buffer));
            }
        } else {
            self.silence_run_samples += mono_samples.len();
            if self.silence_run_samples >= self.silence_hold_samples
                && let Some(buffer) = self.flush_phrase(true)
            {
                events.push(DictationEvent::PhraseReady(buffer));
            }
        }

        events
    }

    /// Force-flush a phrase that has run past `max_phrase` of continuous
    /// speech. Searches the last `split_search` window for the lowest-energy
    /// frame (a likely word boundary) and splits there. The audio after the
    /// split is carried forward as the start of the next phrase to avoid
    /// dropping speech.
    fn flush_phrase_at_split(&mut self) -> Option<AudioBuffer> {
        let len = self.phrase_samples.len();
        if len < self.min_phrase_samples {
            return None;
        }

        // Search window: from `max_phrase_samples - split_search/2` to end.
        // Clamp into a valid range and align to frame boundaries.
        let half_search = self.split_search_samples / 2;
        let search_start = self.max_phrase_samples.saturating_sub(half_search);
        let search_end = len.min(self.max_phrase_samples + half_search);

        let split_at = if search_end > search_start + self.split_frame_samples {
            find_split_point(
                &self.phrase_samples,
                search_start,
                search_end,
                self.split_frame_samples,
            )
        } else {
            // Window collapsed — fall back to hard cut at max_phrase.
            self.max_phrase_samples.min(len)
        };

        // Defensive: never split before min_phrase, never past end.
        let split_at = split_at.clamp(self.min_phrase_samples, len);

        let mut head = std::mem::take(&mut self.phrase_samples);
        let tail = head.split_off(split_at);

        // Reset state and seed the next phrase with the tail so we don't
        // drop the speech that came after the split.
        self.in_speech = !tail.is_empty();
        self.silence_run_samples = 0;
        self.phrase_samples = tail;
        self.preroll.clear();

        if head.len() < self.min_phrase_samples {
            return None;
        }
        Some(AudioBuffer::from_raw(&head, self.native_rate, 1))
    }

    /// How many consecutive speech-positive chunks are needed to open a
    /// phrase. With Silero VAD attached, require 2 (~100ms at the worker's
    /// 50ms tick) so an isolated breath/click frame doesn't start a phrase,
    /// while keeping detection responsive to normal speech. Hallucinations on
    /// noise are caught after transcription by the confidence and
    /// repeated-phrase filters, not by making the input gate strict (which
    /// would force the user to speak unnaturally loudly). The RMS fallback
    /// keeps single-chunk behaviour: its calibrated threshold is the gate.
    fn onset_chunks_required(&self) -> u32 {
        #[cfg(feature = "vad")]
        if self.vad.is_some() {
            return 2;
        }
        1
    }

    pub fn finish(&mut self) -> Option<AudioBuffer> {
        self.flush_phrase(false)
    }

    /// Whether a phrase is in progress: speech was detected and the phrase has
    /// not yet flushed. Silence inside this window is an expected pause (the
    /// user hasn't exceeded `silence_hold` yet), so watchdogs must not treat
    /// it as a dead input device.
    pub fn is_mid_phrase(&self) -> bool {
        self.in_speech
    }

    /// Snapshot of the in-progress phrase, resampled to 16 kHz mono, for live
    /// partial transcription. Returns `None` while idle or before the phrase
    /// has enough audio to be worth transcribing. Does not mutate state, so it
    /// is safe to call repeatedly while a phrase is still being spoken.
    ///
    /// Only the recent tail is returned. A preview just needs the latest words,
    /// and bounding it keeps the resample and the preview decode cheap on long
    /// phrases so they cannot starve the realtime tick or delay the final.
    pub fn current_phrase(&self) -> Option<AudioBuffer> {
        if !self.in_speech || self.phrase_samples.len() < self.min_phrase_samples {
            return None;
        }
        let window = (PREVIEW_WINDOW_SECS * self.native_rate as u64) as usize;
        let start = self.phrase_samples.len().saturating_sub(window);
        Some(AudioBuffer::from_raw(
            &self.phrase_samples[start..],
            self.native_rate,
            1,
        ))
    }

    fn flush_phrase(&mut self, trim_trailing_silence: bool) -> Option<AudioBuffer> {
        let mut samples = std::mem::take(&mut self.phrase_samples);
        let trailing = if trim_trailing_silence {
            self.silence_run_samples.min(samples.len())
        } else {
            0
        };

        if trailing > 0 {
            samples.truncate(samples.len() - trailing);
        }

        self.in_speech = false;
        self.silence_run_samples = 0;
        self.onset_streak = 0;

        if samples.len() < self.min_phrase_samples {
            self.preroll.clear();
            self.push_preroll(&samples);
            return None;
        }

        let buffer = AudioBuffer::from_raw(&samples, self.native_rate, 1);
        self.preroll.clear();

        // Reset VAD state between phrases. Silero's LSTM assumes a
        // continuous stream within an utterance; carrying state across
        // phrase boundaries causes false speech detection at the start of
        // the next phrase as the LSTM "expects" continuation.
        #[cfg(feature = "vad")]
        {
            if let Some(vad) = self.vad.as_mut() {
                vad.reset();
            }
            self.vad_pending.clear();
        }

        Some(buffer)
    }

    fn push_preroll(&mut self, samples: &[f32]) {
        for &sample in samples {
            self.preroll.push_back(sample);
            while self.preroll.len() > self.preroll_samples {
                let _ = self.preroll.pop_front();
            }
        }
    }

    /// Run the chunk through Silero VAD and return Some(true)/Some(false)
    /// if at least one full 512-sample frame was processed. Returns None
    /// when the chunk is too short to produce a complete frame (caller
    /// should fall back to RMS for the moment).
    ///
    /// We resample to 16 kHz internally because callers typically feed
    /// audio at the device's native rate (44.1k or 48k on most systems).
    /// Silero v5 only accepts 16 kHz at 512 samples per frame.
    #[cfg(feature = "vad")]
    fn vad_chunk_is_speech(&mut self, mono_samples: &[f32]) -> Option<bool> {
        let vad = self.vad.as_mut()?;

        let resampled: Vec<f32> = if self.native_rate == 16_000 {
            mono_samples.to_vec()
        } else {
            crate::audio::dsp::resample(mono_samples, self.native_rate, 16_000)
        };

        self.vad_pending.extend_from_slice(&resampled);

        // OR the per-frame results — if any frame in this chunk is speech,
        // treat the whole chunk as speech. This is intentionally generous
        // because phrase boundary detection downstream needs `silence_hold`
        // worth of consistent silence to flush, so a single speech frame
        // here just resets the silence counter.
        let mut decided: Option<bool> = None;
        while self.vad_pending.len() >= SILERO_FRAME_SAMPLES {
            let frame: Vec<f32> = self.vad_pending.drain(..SILERO_FRAME_SAMPLES).collect();
            match vad.process(&frame) {
                Ok(state) => {
                    let frame_speech = state == VadState::Speech;
                    decided = Some(decided.unwrap_or(false) || frame_speech);
                }
                Err(e) => {
                    tracing::warn!("VAD frame failed: {}", e);
                    return None;
                }
            }
        }

        decided
    }
}

/// Scan `samples[search_start..search_end]` in non-overlapping frames of
/// `frame_samples` and return the sample offset at the start of the
/// lowest-RMS frame. Used to pick a clean split point — quiet frames are
/// almost always word boundaries.
///
/// Mirrors the core idea of transcribe-rs's energy_adaptive_chunked
/// `find_split_point`: instead of cutting at a fixed offset, look around
/// the target for a natural pause.
fn find_split_point(
    samples: &[f32],
    search_start: usize,
    search_end: usize,
    frame_samples: usize,
) -> usize {
    debug_assert!(search_end <= samples.len());
    debug_assert!(search_end > search_start);
    debug_assert!(frame_samples > 0);

    let mut best_offset = search_start;
    let mut best_energy = f32::INFINITY;

    let mut offset = search_start;
    while offset + frame_samples <= search_end {
        let frame = &samples[offset..offset + frame_samples];
        // Use sum-of-squares (proportional to RMS²) — the sqrt + length
        // normalization don't change the argmin and we save the work.
        let energy: f32 = frame.iter().map(|&s| s * s).sum();
        if energy < best_energy {
            best_energy = energy;
            best_offset = offset;
        }
        offset += frame_samples;
    }

    best_offset
}

#[cfg(test)]
mod tests {
    use super::*;

    fn speech(len: usize) -> Vec<f32> {
        vec![0.08; len]
    }

    fn silence(len: usize) -> Vec<f32> {
        vec![0.001; len]
    }

    #[test]
    fn emits_phrase_after_silence_hold() {
        let rate = 16_000;
        let mut session = DictationSession::new(
            DictationConfig {
                speech_threshold: 0.01,
                silence_hold: Duration::from_millis(100),
                min_phrase: Duration::from_millis(50),
                max_phrase: Duration::from_secs(5),
                split_search: Duration::from_millis(500),
                preroll: Duration::from_millis(50),
                session_timeout: Duration::from_secs(5),
            },
            rate,
        );

        let _ = session.ingest(&speech(1600));
        let events = session.ingest(&silence(2000));
        assert!(
            events
                .iter()
                .any(|event| matches!(event, DictationEvent::PhraseReady(_)))
        );
    }

    #[test]
    fn current_phrase_snapshots_only_while_speaking() {
        let rate = 16_000;
        let mut session = DictationSession::new(
            DictationConfig {
                speech_threshold: 0.01,
                silence_hold: Duration::from_millis(100),
                min_phrase: Duration::from_millis(50),
                max_phrase: Duration::from_secs(5),
                split_search: Duration::from_millis(500),
                preroll: Duration::from_millis(50),
                session_timeout: Duration::from_secs(5),
            },
            rate,
        );

        // Idle: nothing in progress to preview.
        assert!(session.current_phrase().is_none());

        // Mid-phrase: a 16 kHz snapshot of the spoken audio so far.
        let _ = session.ingest(&speech(1600));
        let snapshot = session.current_phrase().expect("phrase in progress");
        assert_eq!(snapshot.sample_rate, 16_000);
        assert!(!snapshot.samples.is_empty());

        // Once the phrase flushes on silence, there's nothing in progress again.
        let _ = session.ingest(&silence(2000));
        assert!(session.current_phrase().is_none());
    }

    #[test]
    fn proactively_flushes_long_phrase() {
        let rate = 16_000;
        let mut session = DictationSession::new(
            DictationConfig {
                max_phrase: Duration::from_millis(100),
                split_search: Duration::from_millis(50),
                min_phrase: Duration::from_millis(50),
                ..DictationConfig::default()
            },
            rate,
        );

        // First chunk transitions !in_speech → in_speech and stages samples
        // (the state machine returns early on the speech-onset path).
        let _ = session.ingest(&speech(800));
        // Second chunk, while already in_speech, pushes the buffer past
        // `max_phrase + split_search` so the energy-aware split fires.
        let events = session.ingest(&speech(4000));
        assert!(
            events
                .iter()
                .any(|event| matches!(event, DictationEvent::PhraseReady(_)))
        );
    }

    #[test]
    fn split_preserves_all_audio() {
        let rate = 16_000;
        let mut session = DictationSession::new(
            DictationConfig {
                max_phrase: Duration::from_millis(100),
                split_search: Duration::from_millis(50),
                min_phrase: Duration::from_millis(20),
                ..DictationConfig::default()
            },
            rate,
        );

        // Stage an over-length phrase directly and force a split. At 16kHz the
        // flushed head isn't resampled, so head + carried-forward tail must add
        // back up to the original: the split never drops audio.
        let total = 4000;
        session.phrase_samples = vec![0.05_f32; total];
        session.in_speech = true;

        let head = session
            .flush_phrase_at_split()
            .expect("an over-length phrase must yield a head");
        assert!(head.samples.len() >= session.min_phrase_samples);
        assert_eq!(head.samples.len() + session.phrase_samples.len(), total);
    }

    #[test]
    fn split_picks_lowest_energy_frame() {
        // Build a buffer where samples[2400..2880] is silence (480 samples =
        // one frame at 16kHz/30ms) and the rest is loud. The split should
        // land at the start of the silent frame.
        let mut samples = vec![0.5_f32; 5000];
        for s in &mut samples[2400..2880] {
            *s = 0.0;
        }
        let split = find_split_point(&samples, 1000, 4000, 480);
        assert!(
            (2400..=2880).contains(&split),
            "expected split inside silence frame, got {}",
            split
        );
    }
}
