use super::AudioBuffer;
use anyhow::Result;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::{Arc, Mutex};

/// Mono channel count.
pub const CHANNELS: u16 = 1;

/// Manages microphone capture via CPAL.
pub struct AudioCapture {
    buffer: Arc<Mutex<Vec<f32>>>,
    stream: Option<cpal::Stream>,
}

impl AudioCapture {
    /// Create a new AudioCapture using the default input device.
    pub fn new() -> Result<Self> {
        Ok(Self {
            buffer: Arc::new(Mutex::new(Vec::new())),
            stream: None,
        })
    }

    /// Start recording from the default microphone.
    pub fn start(&mut self) -> Result<()> {
        // Clear any previous samples
        if let Ok(mut buf) = self.buffer.lock() {
            buf.clear();
        }

        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| anyhow::anyhow!("No input device available"))?;

        tracing::info!("Using input device: {}", device.name()?);

        let config = cpal::StreamConfig {
            channels: CHANNELS,
            sample_rate: cpal::SampleRate(AudioBuffer::SAMPLE_RATE),
            buffer_size: cpal::BufferSize::Default,
        };

        let buffer = Arc::clone(&self.buffer);
        let stream = device.build_input_stream(
            &config,
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                if let Ok(mut buf) = buffer.lock() {
                    buf.extend_from_slice(data);
                }
            },
            |err| {
                tracing::error!("Audio stream error: {}", err);
            },
            None,
        )?;

        stream.play()?;
        self.stream = Some(stream);
        tracing::info!("Audio capture started");
        Ok(())
    }

    /// Stop recording and return the captured audio buffer.
    pub fn stop(&mut self) -> Result<AudioBuffer> {
        self.stream = None;
        let mut samples = self.buffer.lock().map_err(|e| anyhow::anyhow!("{}", e))?;
        let captured = AudioBuffer {
            samples: std::mem::take(&mut *samples),
            sample_rate: AudioBuffer::SAMPLE_RATE,
        };
        tracing::info!(
            "Audio capture stopped, {} samples ({:.2}s)",
            captured.samples.len(),
            captured.duration_secs()
        );
        Ok(captured)
    }

    /// Check if currently recording.
    pub fn is_recording(&self) -> bool {
        self.stream.is_some()
    }
}
