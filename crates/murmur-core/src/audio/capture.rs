use super::AudioBuffer;
use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, SupportedStreamConfig};
use std::sync::{Arc, Mutex};

/// Manages microphone capture via CPAL.
///
/// Enumerates the device's supported configs, picks the best one,
/// then converts to 16 kHz mono f32 in `stop()`.
pub struct AudioCapture {
    buffer: Arc<Mutex<Vec<f32>>>,
    stream: Option<cpal::Stream>,
    /// Active Windows voice-capture session (echo cancellation), when used.
    #[cfg(windows)]
    voice: Option<super::wasapi::WasapiVoiceCapture>,
    /// Active Linux voice-capture session (echo cancellation), when used.
    #[cfg(target_os = "linux")]
    pulse: Option<super::pulse::PulseVoiceCapture>,
    native_rate: u32,
    native_channels: u16,
}

impl AudioCapture {
    /// Create a new AudioCapture using the default input device.
    pub fn new() -> Result<Self> {
        Ok(Self {
            buffer: Arc::new(Mutex::new(Vec::new())),
            stream: None,
            #[cfg(windows)]
            voice: None,
            #[cfg(target_os = "linux")]
            pulse: None,
            native_rate: AudioBuffer::SAMPLE_RATE,
            native_channels: 1,
        })
    }

    /// Start recording. With `echo_cancellation` on, prefer the OS voice-capture
    /// path (Windows AEC) on the default mic; otherwise, or on failure, use the
    /// raw CPAL microphone.
    pub fn start(&mut self, preferred_device: Option<&str>, echo_cancellation: bool) -> Result<()> {
        self.stream = None;
        #[cfg(windows)]
        {
            self.voice = None;
        }
        #[cfg(target_os = "linux")]
        {
            self.pulse = None;
        }

        let on_default = preferred_device
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .is_none();
        if echo_cancellation && on_default && self.try_start_voice_capture().is_some() {
            return Ok(());
        }

        self.start_cpal(preferred_device)
    }

    /// Try the OS echo-cancelling capture path. Returns `Some(())` on success.
    /// Windows uses WASAPI communications mode; Linux uses the PulseAudio /
    /// PipeWire echo-cancel module. macOS has no implementation yet (returns
    /// `None`, so the caller falls back to the raw mic). Plan: the
    /// VoiceProcessingIO AudioUnit on macOS.
    fn try_start_voice_capture(&mut self) -> Option<()> {
        #[cfg(windows)]
        {
            let max_samples = self.prepare_voice_buffer();
            let result = super::wasapi::open_voice_capture(Arc::clone(&self.buffer), max_samples);
            let cap = self.voice_started(result)?;
            self.voice = Some(cap);
            Some(())
        }
        #[cfg(target_os = "linux")]
        {
            let max_samples = self.prepare_voice_buffer();
            let result = super::pulse::open_voice_capture(Arc::clone(&self.buffer), max_samples);
            let cap = self.voice_started(result)?;
            self.pulse = Some(cap);
            Some(())
        }
        #[cfg(not(any(windows, target_os = "linux")))]
        {
            None
        }
    }

    /// Clear and pre-size the shared buffer for ~30s of 48kHz stereo voice
    /// audio so the capture thread never reallocates mid-session, and return
    /// the 60s hard cap. Shared by every echo-cancelling capture backend.
    #[cfg(any(windows, target_os = "linux"))]
    fn prepare_voice_buffer(&self) -> usize {
        const MAX_SAMPLES: usize = 60 * 48_000 * 2;
        if let Ok(mut buf) = self.buffer.lock() {
            buf.clear();
            let want = 30 * 48_000 * 2;
            let cap = buf.capacity();
            if cap < want {
                buf.reserve(want - cap);
            }
        }
        MAX_SAMPLES
    }

    /// Record the negotiated format and log on success, or warn and fall back
    /// on failure. Returns the backend handle for the caller to store.
    #[cfg(any(windows, target_os = "linux"))]
    fn voice_started<T>(&mut self, result: Result<(T, u32, u16)>) -> Option<T> {
        match result {
            Ok((cap, rate, channels)) => {
                self.native_rate = rate;
                self.native_channels = channels;
                tracing::info!(
                    "Voice capture (echo cancellation) active: {}Hz, {} channel(s)",
                    rate,
                    channels
                );
                Some(cap)
            }
            Err(e) => {
                tracing::warn!("Voice capture unavailable ({e}); using raw microphone");
                None
            }
        }
    }

    /// Raw microphone capture via CPAL (no echo cancellation).
    fn start_cpal(&mut self, preferred_device: Option<&str>) -> Result<()> {
        let host = cpal::default_host();
        let device = select_input_device(&host, preferred_device)?;

        tracing::info!("Using input device: {}", device.name()?);

        let supported = choose_input_config(&device)?;
        let sample_format = supported.sample_format();
        let native_rate = supported.sample_rate().0;
        let native_channels = supported.channels();

        // Pre-reserve enough capacity for ~30 s of audio at the chosen
        // config so the realtime cpal callback never reallocates during
        // `extend_from_slice`. A reallocating Vec on the audio thread is
        // the textbook cause of audible dropouts. The consumer drains the
        // buffer routinely, so this is a high-water mark, not steady state.
        const RESERVE_SECS: usize = 30;
        let reserve_samples = RESERVE_SECS * native_rate as usize * native_channels.max(1) as usize;
        if let Ok(mut buf) = self.buffer.lock() {
            buf.clear();
            let current_cap = buf.capacity();
            if current_cap < reserve_samples {
                buf.reserve(reserve_samples - current_cap);
            }
        }

        tracing::info!(
            "Selected config: {}Hz, {} channel(s), format: {:?}",
            native_rate,
            native_channels,
            sample_format
        );

        self.native_rate = native_rate;
        self.native_channels = native_channels;

        let config = supported.config();

        // Hard cap on the live buffer so a stalled consumer can never grow it
        // without bound and reallocate (or OOM) on the realtime thread. Well
        // above the 30s reserve, so normal operation never reaches it.
        const MAX_BUFFER_SECS: usize = 60;
        let max_samples = MAX_BUFFER_SECS * native_rate as usize * native_channels.max(1) as usize;

        let stream = build_input_stream_for_format(
            &device,
            &config,
            sample_format,
            &self.buffer,
            max_samples,
        )?;

        stream.play()?;
        self.stream = Some(stream);
        tracing::info!("Audio capture started");
        Ok(())
    }

    /// Stop recording and return the captured audio buffer (16 kHz mono).
    pub fn stop(&mut self) -> Result<AudioBuffer> {
        self.stream = None;
        #[cfg(windows)]
        {
            self.voice = None;
        }
        #[cfg(target_os = "linux")]
        {
            self.pulse = None;
        }
        let mut samples = self.buffer.lock().map_err(|e| anyhow::anyhow!("{}", e))?;
        let raw = std::mem::take(&mut *samples);

        tracing::info!(
            "Raw capture: {} samples at {}Hz, {} ch",
            raw.len(),
            self.native_rate,
            self.native_channels
        );

        let mono = if self.native_channels > 1 {
            let ch = self.native_channels as usize;
            raw.chunks_exact(ch)
                .map(|frame| frame.iter().sum::<f32>() / ch as f32)
                .collect::<Vec<f32>>()
        } else {
            raw
        };

        let target_rate = AudioBuffer::SAMPLE_RATE;
        let resampled = if self.native_rate != target_rate {
            resample(&mono, self.native_rate, target_rate)
        } else {
            mono
        };

        let captured = AudioBuffer {
            samples: resampled,
            sample_rate: target_rate,
        };

        tracing::info!(
            "Audio capture stopped, {} samples ({:.2}s)",
            captured.samples.len(),
            captured.duration_secs()
        );

        Ok(captured)
    }

    /// Get a clone of the live audio buffer Arc for external monitoring.
    pub fn live_buffer(&self) -> Arc<Mutex<Vec<f32>>> {
        Arc::clone(&self.buffer)
    }

    /// Get the native sample rate of the current/last capture session.
    pub fn native_rate(&self) -> u32 {
        self.native_rate
    }

    /// Get the native channel count of the current/last capture session.
    pub fn native_channels(&self) -> u16 {
        self.native_channels
    }

    /// Check if currently recording.
    pub fn is_recording(&self) -> bool {
        #[cfg(windows)]
        if self.voice.is_some() {
            return true;
        }
        #[cfg(target_os = "linux")]
        if self.pulse.is_some() {
            return true;
        }
        self.stream.is_some()
    }

    /// Whether the OS echo-cancelling capture path is the active source (as
    /// opposed to the raw CPAL mic it falls back to). Calibration uses this to
    /// skip the gain boost the raw mic needs: the voice stream is already
    /// echo-cancelled and leveled, so boosting it just re-amplifies idle noise.
    pub fn echo_cancellation_active(&self) -> bool {
        #[cfg(windows)]
        {
            self.voice.is_some()
        }
        #[cfg(target_os = "linux")]
        {
            self.pulse.is_some()
        }
        #[cfg(not(any(windows, target_os = "linux")))]
        {
            false
        }
    }
}

fn select_input_device(host: &cpal::Host, preferred_device: Option<&str>) -> Result<cpal::Device> {
    if let Some(preferred_name) = preferred_device
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        let devices = host
            .input_devices()
            .context("Failed to enumerate input devices")?;

        for device in devices {
            let Ok(name) = device.name() else {
                continue;
            };
            if name == preferred_name {
                return Ok(device);
            }
        }

        tracing::warn!(
            "Preferred input device {:?} not found, falling back to default input device",
            preferred_name
        );
    }

    host.default_input_device()
        .ok_or_else(|| anyhow::anyhow!("No input device available"))
}

fn choose_input_config(device: &cpal::Device) -> Result<SupportedStreamConfig> {
    #[cfg(windows)]
    {
        if let Ok(default) = device.default_input_config() {
            tracing::info!(
                "Using Windows default input config: {}Hz, {} channel(s), format: {:?}",
                default.sample_rate().0,
                default.channels(),
                default.sample_format()
            );
            return Ok(default);
        }

        tracing::warn!("default_input_config failed on Windows, falling back to scanned configs");
    }

    pick_best_config(device)
}

fn build_input_stream_for_format(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    sample_format: SampleFormat,
    buffer: &Arc<Mutex<Vec<f32>>>,
    max_samples: usize,
) -> Result<cpal::Stream> {
    // Zero-center for unsigned formats: silence sits at the midpoint, not 0.
    match sample_format {
        SampleFormat::F32 => {
            build_typed_stream::<f32, _>(device, config, buffer, max_samples, |s| s)
        }
        SampleFormat::I16 => {
            build_typed_stream::<i16, _>(device, config, buffer, max_samples, |s| {
                s as f32 / 32768.0
            })
        }
        SampleFormat::U16 => {
            build_typed_stream::<u16, _>(device, config, buffer, max_samples, |s| {
                (s as f32 - 32768.0) / 32768.0
            })
        }
        SampleFormat::I32 => {
            build_typed_stream::<i32, _>(device, config, buffer, max_samples, |s| {
                // Convert through f64: an i32 has more precision than f32's
                // mantissa, so dividing in f32 would lose low bits.
                (s as f64 / i32::MAX as f64) as f32
            })
        }
        SampleFormat::U32 => {
            build_typed_stream::<u32, _>(device, config, buffer, max_samples, |s| {
                ((s as f64 - 2_147_483_648.0) / 2_147_483_648.0) as f32
            })
        }
        SampleFormat::I8 => build_typed_stream::<i8, _>(device, config, buffer, max_samples, |s| {
            s as f32 / i8::MAX as f32
        }),
        SampleFormat::U8 => build_typed_stream::<u8, _>(device, config, buffer, max_samples, |s| {
            (s as f32 - 128.0) / 128.0
        }),
        format => anyhow::bail!("Unsupported sample format: {:?}", format),
    }
}

/// Build an input stream that converts each sample to f32 and appends it to
/// the live buffer, dropping new audio once the buffer hits `max_samples` so a
/// stalled consumer cannot grow it without bound on the realtime thread.
fn build_typed_stream<T, F>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    buffer: &Arc<Mutex<Vec<f32>>>,
    max_samples: usize,
    convert: F,
) -> Result<cpal::Stream>
where
    T: cpal::SizedSample + Send + 'static,
    F: Fn(T) -> f32 + Send + 'static,
{
    let buffer = Arc::clone(buffer);
    let err_fn = |err| tracing::error!("Audio stream error: {}", err);
    let stream = device.build_input_stream(
        config,
        move |data: &[T], _: &cpal::InputCallbackInfo| {
            if let Ok(mut buf) = buffer.lock()
                && buf.len() < max_samples
            {
                // Sanitize at the boundary: a NaN/inf from a misbehaving driver
                // would otherwise poison RMS scoring and the resampler.
                buf.extend(data.iter().map(|&s| {
                    let v = convert(s);
                    if v.is_finite() { v } else { 0.0 }
                }));
            }
        },
        err_fn,
        None,
    )?;
    Ok(stream)
}

/// Score a sample format — lower is better.
/// F32 is native for our pipeline, I16 is common and cheap to convert, I32 is rare.
fn format_score(fmt: SampleFormat) -> u32 {
    match fmt {
        SampleFormat::F32 => 0,
        SampleFormat::I16 => 1,
        SampleFormat::I32 => 2,
        _ => 10,
    }
}

/// Score a channel count — lower is better. Mono is ideal (no downmix needed).
fn channel_score(ch: u16) -> u32 {
    match ch {
        1 => 0,
        2 => 1,
        _ => ch as u32,
    }
}

/// Score a sample rate — lower is better. Prefer rates close to common standards.
fn rate_score(rate: u32) -> u32 {
    match rate {
        16000 => 0, // perfect: no resample
        48000 => 1, // clean 3:1
        44100 => 2, // common
        _ => 5,
    }
}

/// Pick the best supported input config from the device.
///
/// Enumerates all supported config ranges, scores them by format, channels,
/// and sample rate, and returns the best concrete config.
fn pick_best_config(device: &cpal::Device) -> Result<SupportedStreamConfig> {
    let configs: Vec<_> = device
        .supported_input_configs()
        .context("Failed to enumerate supported input configs")?
        .collect();

    if configs.is_empty() {
        anyhow::bail!("No supported input configs found for device");
    }

    tracing::debug!("Found {} supported input config range(s):", configs.len());
    for (i, c) in configs.iter().enumerate() {
        tracing::debug!(
            "  [{}] {:?}, {} ch, {}–{}Hz",
            i,
            c.sample_format(),
            c.channels(),
            c.min_sample_rate().0,
            c.max_sample_rate().0
        );
    }

    // For each config range, pick the best concrete sample rate and score it.
    let mut best: Option<(u32, SupportedStreamConfig)> = None;

    for range in &configs {
        let fmt = range.sample_format();
        let ch = range.channels();

        let preferred_rates = [16000u32, 48000, 44100];
        let rate = preferred_rates
            .iter()
            .copied()
            .find(|&r| r >= range.min_sample_rate().0 && r <= range.max_sample_rate().0)
            .unwrap_or(range.max_sample_rate().0);

        let score = format_score(fmt) * 100 + channel_score(ch) * 10 + rate_score(rate);

        let concrete = (*range).with_sample_rate(cpal::SampleRate(rate));

        if best
            .as_ref()
            .is_none_or(|(best_score, _)| score < *best_score)
        {
            best = Some((score, concrete));
        }
    }

    let (score, config) = best.ok_or_else(|| anyhow::anyhow!("No valid config found"))?;
    tracing::debug!(
        "Picked config (score {}): {:?}, {} ch, {}Hz",
        score,
        config.sample_format(),
        config.channels(),
        config.sample_rate().0
    );

    Ok(config)
}

/// Anti-aliased resampler: applies a low-pass filter before downsampling
/// to prevent aliasing artifacts, then uses linear interpolation.
pub(crate) fn resample(samples: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if from_rate == to_rate || samples.is_empty() {
        return samples.to_vec();
    }

    // When downsampling, apply an anti-aliasing low-pass filter at the
    // target Nyquist frequency (to_rate / 2) to prevent aliasing.
    let source = if from_rate > to_rate {
        lowpass_antialias(samples, from_rate, to_rate)
    } else {
        samples.to_vec()
    };

    let ratio = from_rate as f64 / to_rate as f64;
    let output_len = (source.len() as f64 / ratio) as usize;
    let mut output = Vec::with_capacity(output_len);

    for i in 0..output_len {
        let src_pos = i as f64 * ratio;
        let idx = src_pos as usize;
        let frac = (src_pos - idx as f64) as f32;

        let sample = if idx + 1 < source.len() {
            source[idx] * (1.0 - frac) + source[idx + 1] * frac
        } else {
            source[idx.min(source.len() - 1)]
        };

        output.push(sample);
    }

    output
}

/// Two-pass (forward + backward) single-pole IIR low-pass filter.
///
/// Applied before downsampling to prevent aliasing. The two-pass approach
/// gives second-order rolloff (~12dB/octave) with zero phase distortion.
/// Cutoff is set to the target Nyquist frequency (to_rate / 2).
fn lowpass_antialias(samples: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    // RC time constant for cutoff at target_nyquist = to_rate / 2
    let cutoff_hz = to_rate as f32 / 2.0;
    let rc = 1.0 / (std::f32::consts::TAU * cutoff_hz);
    let dt = 1.0 / from_rate as f32;
    let alpha = dt / (rc + dt);

    // Forward pass
    let mut filtered = Vec::with_capacity(samples.len());
    let mut prev = samples[0];
    for &s in samples {
        prev += alpha * (s - prev);
        filtered.push(prev);
    }

    // Backward pass (zero-phase: eliminates phase distortion from forward pass)
    prev = *filtered.last().unwrap();
    for s in filtered.iter_mut().rev() {
        prev += alpha * (*s - prev);
        *s = prev;
    }

    filtered
}
