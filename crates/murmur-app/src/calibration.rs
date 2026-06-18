//! Startup noise-floor calibration and dictation session construction.

use murmur_core::audio::silence::{compute_rms, downmix_to_mono};
use murmur_core::dictation::{DictationConfig, DictationSession};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::audio_worker::StartParams;

pub(crate) struct Calibration {
    pub mic_gain: f32,
    pub threshold: f32,
    pub effective_ambient: f32,
}

/// Silero VAD floor for the echo-cancelled path. Communications-mode AGC
/// amplifies idle noise toward speaking level during silence, which the
/// default 0.30 reads as speech; real speech still spikes well past this.
#[cfg(feature = "vad")]
const ECHO_CANCEL_VAD_THRESHOLD: f32 = 0.50;

/// Estimate the noise floor from a short startup window.
///
/// Uses the MINIMUM chunk RMS, not the mean: users start talking immediately
/// after the hotkey, so the window often contains speech, and a mean would
/// inflate the estimate until nothing ever crosses the threshold. The
/// quietest 50ms chunk — an inter-word gap at worst — is the better proxy.
pub(crate) fn calibrate(
    live_buf: &Arc<Mutex<Vec<f32>>>,
    analyzed_up_to: &mut usize,
    native_channels: u16,
    native_rate: u32,
    configured_threshold: f32,
    echo_cancellation: bool,
) -> Calibration {
    let start = Instant::now();
    let mut chunk_levels = Vec::new();
    tracing::info!(
        "Calibrating noise floor ({} ch, {}Hz)...",
        native_channels,
        native_rate
    );
    while start.elapsed() < Duration::from_millis(250) {
        std::thread::sleep(Duration::from_millis(50));
        let buf = live_buf.lock().unwrap_or_else(|e| e.into_inner());
        if buf.len() > *analyzed_up_to {
            let mono = downmix_to_mono(&buf[*analyzed_up_to..], native_channels);
            let rms = compute_rms(&mono);
            if rms > 0.0 {
                chunk_levels.push(rms);
            }
            *analyzed_up_to = buf.len();
        }
    }

    let ambient_rms = chunk_levels.iter().copied().fold(f32::INFINITY, f32::min);
    let ambient_rms = if ambient_rms.is_finite() {
        ambient_rms
    } else {
        0.0
    };

    // Boost quiet mics so detection, the UI level meter, and STT all see
    // usable levels. Capped at 5x: more amplifies the noise floor into
    // whisper-hallucination territory. The echo-cancelled path is already
    // AGC-leveled by the OS, so a boost there would re-amplify the AGC's
    // silence-noise into that same territory — leave it at unity.
    let mic_gain = if echo_cancellation {
        1.0
    } else if ambient_rms > 0.0001 && ambient_rms < 0.02 {
        (0.02 / ambient_rms).min(5.0)
    } else if ambient_rms <= 0.0001 {
        3.0
    } else {
        1.0
    };

    let effective_ambient = ambient_rms * mic_gain;
    let threshold = if configured_threshold > 0.0 {
        configured_threshold
    } else if echo_cancellation {
        // Noise suppression keeps the idle floor near zero, so the
        // ambient-derived threshold would collapse to its minimum and stop
        // gating. Use a fixed floor; VAD does the real speech detection.
        0.010
    } else {
        (effective_ambient * 1.8).clamp(0.001, 0.015)
    };
    tracing::info!(
        "Calibrated: ambient RMS = {:.6}, mic_gain = {:.1}x, effective ambient = {:.6}, threshold = {:.6} (config = {:.6})",
        ambient_rms,
        mic_gain,
        effective_ambient,
        threshold,
        configured_threshold,
    );

    Calibration {
        mic_gain,
        threshold,
        effective_ambient,
    }
}

/// Keep up to ~1s of calibration audio and rewind `analyzed_up_to` so the
/// monitor re-reads it: the window often contains the start of the user's
/// utterance, and dropping it loses the first words of every session.
pub(crate) fn keep_calibration_preroll(
    live_buf: &Arc<Mutex<Vec<f32>>>,
    analyzed_up_to: &mut usize,
    native_rate: u32,
    native_channels: u16,
) {
    let mut buf = live_buf.lock().unwrap_or_else(|e| e.into_inner());
    trim_buffer_to_preroll(&mut buf, native_rate, native_channels);
    *analyzed_up_to = 0;
}

/// Drop everything older than ~1s, channel-aligned so interleaved stereo
/// frames stay in step for downmix. Returns the number of samples dropped.
pub(crate) fn trim_buffer_to_preroll(
    buf: &mut Vec<f32>,
    native_rate: u32,
    native_channels: u16,
) -> usize {
    let ch = native_channels.max(1) as usize;
    let preroll_samples = (native_rate as usize) * ch;
    if buf.len() <= preroll_samples {
        return 0;
    }
    let mut drop_count = buf.len() - preroll_samples;
    drop_count -= drop_count % ch;
    buf.drain(..drop_count);
    drop_count
}

pub(crate) fn build_session(
    params: &StartParams,
    calibration: &Calibration,
    native_rate: u32,
    echo_cancellation: bool,
) -> DictationSession {
    #[cfg(not(feature = "vad"))]
    let _ = (params.vad_threshold, echo_cancellation);

    #[cfg_attr(not(feature = "vad"), allow(unused_mut))]
    let mut session = DictationSession::new(
        DictationConfig {
            speech_threshold: calibration.threshold,
            silence_hold: params.phrase_pause,
            session_timeout: params.session_timeout,
            ..DictationConfig::default()
        },
        native_rate,
    );

    #[cfg(feature = "vad")]
    if let Some(vad) = load_vad(params.vad_threshold, echo_cancellation) {
        session = session.with_vad(vad);
    }

    session
}

/// Load Silero VAD when its model file is on disk; fall back to RMS
/// detection silently otherwise so the app keeps working.
#[cfg(feature = "vad")]
fn load_vad(
    vad_threshold: f32,
    echo_cancellation: bool,
) -> Option<murmur_core::audio::vad::VoiceActivityDetector> {
    use murmur_core::audio::vad as silero;

    if !silero::is_downloaded() {
        tracing::debug!("Silero VAD model not yet downloaded; using RMS detection");
        return None;
    }
    let path = match silero::model_path() {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("Could not resolve VAD model path: {}", e);
            return None;
        }
    };
    let base = if (0.05..=0.95).contains(&vad_threshold) {
        vad_threshold
    } else {
        silero::DEFAULT_THRESHOLD
    };
    // The echo-cancelled path needs a higher floor (AGC noise reads as speech
    // at the default), but never override a user who set something stricter.
    let threshold = if echo_cancellation {
        base.max(ECHO_CANCEL_VAD_THRESHOLD)
    } else {
        base
    };
    match silero::VoiceActivityDetector::new(&path.to_string_lossy(), threshold) {
        Ok(vad) => {
            tracing::info!("Attached Silero VAD to dictation session");
            Some(vad)
        }
        Err(e) => {
            tracing::warn!(
                "Failed to load Silero VAD: {}. Falling back to RMS detection.",
                e
            );
            None
        }
    }
}
