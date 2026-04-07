use crate::audio::AudioBuffer;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy)]
pub struct DictationConfig {
    pub speech_threshold: f32,
    pub silence_hold: Duration,
    pub min_phrase: Duration,
    pub max_phrase: Duration,
    pub preroll: Duration,
    pub session_timeout: Duration,
}

impl Default for DictationConfig {
    fn default() -> Self {
        Self {
            speech_threshold: 0.010,
            silence_hold: Duration::from_millis(700),
            min_phrase: Duration::from_millis(150),
            max_phrase: Duration::from_secs(3),
            preroll: Duration::from_millis(350),
            session_timeout: Duration::from_secs(30),
        }
    }
}

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
    preroll: VecDeque<f32>,
    phrase_samples: Vec<f32>,
    in_speech: bool,
    silence_run_samples: usize,
    last_activity: Instant,
    signaled_activity: bool,
}

impl DictationSession {
    pub fn new(config: DictationConfig, native_rate: u32) -> Self {
        let sample_count = |duration: Duration| -> usize {
            ((duration.as_secs_f64() * native_rate as f64).round() as usize).max(1)
        };

        Self {
            config,
            native_rate,
            preroll_samples: sample_count(config.preroll),
            silence_hold_samples: sample_count(config.silence_hold),
            min_phrase_samples: sample_count(config.min_phrase),
            max_phrase_samples: sample_count(config.max_phrase),
            preroll: VecDeque::new(),
            phrase_samples: Vec::new(),
            in_speech: false,
            silence_run_samples: 0,
            last_activity: Instant::now(),
            signaled_activity: false,
        }
    }

    pub fn ingest(&mut self, mono_samples: &[f32]) -> Vec<DictationEvent> {
        let mut events = Vec::new();
        if mono_samples.is_empty() {
            return events;
        }

        let rms = compute_rms(mono_samples);
        events.push(DictationEvent::Level(rms));
        let is_speech = rms >= self.config.speech_threshold;

        if is_speech {
            self.last_activity = Instant::now();
            if !self.signaled_activity {
                self.signaled_activity = true;
                events.push(DictationEvent::ActivityDetected);
            }
        }

        if !self.in_speech {
            if is_speech {
                self.in_speech = true;
                self.silence_run_samples = 0;
                self.phrase_samples.extend(self.preroll.drain(..));
                self.phrase_samples.extend_from_slice(mono_samples);
            } else {
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
            if self.phrase_samples.len() >= self.max_phrase_samples
                && let Some(buffer) = self.flush_phrase(false)
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

    pub fn finish(&mut self) -> Option<AudioBuffer> {
        self.flush_phrase(false)
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

        if samples.len() < self.min_phrase_samples {
            self.preroll.clear();
            self.push_preroll(&samples);
            return None;
        }

        let buffer = AudioBuffer::from_raw(&samples, self.native_rate, 1);
        self.preroll.clear();
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
}

fn compute_rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }

    let sum_sq: f32 = samples.iter().map(|&s| s * s).sum();
    (sum_sq / samples.len() as f32).sqrt()
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
    fn proactively_flushes_long_phrase() {
        let rate = 16_000;
        let mut session = DictationSession::new(
            DictationConfig {
                max_phrase: Duration::from_millis(100),
                ..DictationConfig::default()
            },
            rate,
        );

        let events = session.ingest(&speech(4000));
        assert!(
            events
                .iter()
                .any(|event| matches!(event, DictationEvent::PhraseReady(_)))
        );
    }
}
