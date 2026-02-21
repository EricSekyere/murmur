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
    pub fn start(&mut self) -> Result<()> {
        if let Ok(mut buf) = self.buffer.lock() {
            buf.clear();
        }

        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| anyhow::anyhow!("No input device available"))?;

        tracing::info!("Using input device: {}", device.name()?);

        let supported = pick_best_config(&device)?;
        let sample_format = supported.sample_format();
        let native_rate = supported.sample_rate().0;
        let native_channels = supported.channels();

        tracing::info!(
            "Selected config: {}Hz, {} channel(s), format: {:?}",
            native_rate,
            native_channels,
            sample_format
        );

        self.native_rate = native_rate;
        self.native_channels = native_channels;

        let config = supported.config();

        let stream = match sample_format {
            SampleFormat::F32 => {
                let buffer = Arc::clone(&self.buffer);
                device.build_input_stream(
                    &config,
                    move |data: &[f32], _: &cpal::InputCallbackInfo| {
                        if let Ok(mut buf) = buffer.lock() {
                            buf.extend_from_slice(data);
                        }
                    },
                    |err| tracing::error!("Audio stream error: {}", err),
                    None,
                )?
            }
            SampleFormat::I16 => {
                let buffer = Arc::clone(&self.buffer);
                device.build_input_stream(
                    &config,
                    move |data: &[i16], _: &cpal::InputCallbackInfo| {
                        if let Ok(mut buf) = buffer.lock() {
                            buf.extend(data.iter().map(|&s| s as f32 / 32768.0));
                        }
                    },
                    |err| tracing::error!("Audio stream error: {}", err),
                    None,
                )?
            }
            SampleFormat::I32 => {
                let buffer = Arc::clone(&self.buffer);
                device.build_input_stream(
                    &config,
                    move |data: &[i32], _: &cpal::InputCallbackInfo| {
                        if let Ok(mut buf) = buffer.lock() {
                            buf.extend(data.iter().map(|&s| s as f32 / i32::MAX as f32));
                        }
                    },
                    |err| tracing::error!("Audio stream error: {}", err),
                    None,
                )?
            }
            format => anyhow::bail!("Unsupported sample format: {:?}", format),
        };

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

/// Linear interpolation resampler.
pub(crate) fn resample(samples: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if from_rate == to_rate || samples.is_empty() {
        return samples.to_vec();
    }

    let ratio = from_rate as f64 / to_rate as f64;
    let output_len = (samples.len() as f64 / ratio) as usize;
    let mut output = Vec::with_capacity(output_len);

    for i in 0..output_len {
        let src_pos = i as f64 * ratio;
        let idx = src_pos as usize;
        let frac = (src_pos - idx as f64) as f32;

        let sample = if idx + 1 < samples.len() {
            samples[idx] * (1.0 - frac) + samples[idx + 1] * frac
        } else {
            samples[idx.min(samples.len() - 1)]
        };

        output.push(sample);
    }

    output
}
