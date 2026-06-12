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
    native_rate: u32,
    native_channels: u16,
}

impl AudioCapture {
    /// Create a new AudioCapture using the default input device.
    pub fn new() -> Result<Self> {
        Ok(Self {
            buffer: Arc::new(Mutex::new(Vec::new())),
            stream: None,
            native_rate: AudioBuffer::SAMPLE_RATE,
            native_channels: 1,
        })
    }

    /// Start recording from the default microphone.
    pub fn start(&mut self, preferred_device: Option<&str>) -> Result<()> {
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

        let stream = build_input_stream_for_format(&device, &config, sample_format, &self.buffer)?;

        stream.play()?;
        self.stream = Some(stream);
        tracing::info!("Audio capture started");
        Ok(())
    }

    /// Stop recording and return the captured audio buffer (16 kHz mono).
    pub fn stop(&mut self) -> Result<AudioBuffer> {
        self.stream = None;
        let mut samples = self.buffer.lock().map_err(|e| anyhow::anyhow!("{}", e))?;
        let raw = std::mem::take(&mut *samples);

        tracing::info!(
            "Raw capture: {} samples at {}Hz, {} ch",
            raw.len(),
            self.native_rate,
            self.native_channels
        );

        // Downmix to mono
        let mono = if self.native_channels > 1 {
            let ch = self.native_channels as usize;
            raw.chunks_exact(ch)
                .map(|frame| frame.iter().sum::<f32>() / ch as f32)
                .collect::<Vec<f32>>()
        } else {
            raw
        };

        // Resample to 16 kHz
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
        self.stream.is_some()
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
) -> Result<cpal::Stream> {
    let err_fn = |err| tracing::error!("Audio stream error: {}", err);

    let stream = match sample_format {
        SampleFormat::F32 => {
            let buffer = Arc::clone(buffer);
            device.build_input_stream(
                config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    if let Ok(mut buf) = buffer.lock() {
                        buf.extend_from_slice(data);
                    }
                },
                err_fn,
                None,
            )?
        }
        SampleFormat::I16 => {
            let buffer = Arc::clone(buffer);
            device.build_input_stream(
                config,
                move |data: &[i16], _: &cpal::InputCallbackInfo| {
                    if let Ok(mut buf) = buffer.lock() {
                        buf.extend(data.iter().map(|&s| s as f32 / 32768.0));
                    }
                },
                err_fn,
                None,
            )?
        }
        SampleFormat::U16 => {
            let buffer = Arc::clone(buffer);
            device.build_input_stream(
                config,
                move |data: &[u16], _: &cpal::InputCallbackInfo| {
                    if let Ok(mut buf) = buffer.lock() {
                        // Zero-centered: silence in unsigned formats sits at
                        // the midpoint (32768), not at 0.
                        buf.extend(data.iter().map(|&s| (s as f32 - 32768.0) / 32768.0));
                    }
                },
                err_fn,
                None,
            )?
        }
        SampleFormat::I32 => {
            let buffer = Arc::clone(buffer);
            device.build_input_stream(
                config,
                move |data: &[i32], _: &cpal::InputCallbackInfo| {
                    if let Ok(mut buf) = buffer.lock() {
                        buf.extend(data.iter().map(|&s| s as f32 / i32::MAX as f32));
                    }
                },
                err_fn,
                None,
            )?
        }
        SampleFormat::U32 => {
            let buffer = Arc::clone(buffer);
            device.build_input_stream(
                config,
                move |data: &[u32], _: &cpal::InputCallbackInfo| {
                    if let Ok(mut buf) = buffer.lock() {
                        buf.extend(
                            data.iter()
                                .map(|&s| ((s as f64 - 2_147_483_648.0) / 2_147_483_648.0) as f32),
                        );
                    }
                },
                err_fn,
                None,
            )?
        }
        SampleFormat::I8 => {
            let buffer = Arc::clone(buffer);
            device.build_input_stream(
                config,
                move |data: &[i8], _: &cpal::InputCallbackInfo| {
                    if let Ok(mut buf) = buffer.lock() {
                        buf.extend(data.iter().map(|&s| s as f32 / i8::MAX as f32));
                    }
                },
                err_fn,
                None,
            )?
        }
        SampleFormat::U8 => {
            let buffer = Arc::clone(buffer);
            device.build_input_stream(
                config,
                move |data: &[u8], _: &cpal::InputCallbackInfo| {
                    if let Ok(mut buf) = buffer.lock() {
                        buf.extend(data.iter().map(|&s| (s as f32 - 128.0) / 128.0));
                    }
                },
                err_fn,
                None,
            )?
        }
        format => anyhow::bail!("Unsupported sample format: {:?}", format),
    };

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

        // Pick the best sample rate this range supports
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
