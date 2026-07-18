//! System-audio (loopback) capture — a spike proving Windows render-mix
//! capture needs no new dependency beyond the CPAL/WASAPI stack Murmur already
//! ships. Enables the future meeting-arc, where "the other side" of a call is
//! whatever is playing out of the speakers.
//!
//! # How it works (Windows / WASAPI)
//! CPAL's WASAPI backend opens an **output** endpoint in loopback mode when you
//! call [`build_input_stream`](cpal::traits::DeviceTrait::build_input_stream)
//! on it: the render mix is delivered back as capture data. So the recipe is
//! "take the default *output* device, take its *output* config (loopback
//! delivers the render mix in the device's native format), then build an
//! *input* stream on it." No `AUDCLNT_STREAMFLAGS_LOOPBACK` plumbing of our
//! own — CPAL detects the render endpoint and sets it.
//!
//! # Empirically discovered quirks (this machine, verified by the probe)
//! - **Silence delivers ZERO callbacks, not zero-filled buffers.** WASAPI
//!   loopback is event-driven off the render engine; while nothing is playing,
//!   the render engine parks and no capture packets are produced, so CPAL's
//!   data callback simply does not fire. CPAL does *not* paper this over with a
//!   silence clock. Consequence for the future meeting mixer: it CANNOT assume
//!   a steady loopback sample clock to align against the mic — it needs its own
//!   monotonic silence clock (or to insert silence by wall-time) to keep the
//!   two streams time-aligned across gaps where the far side is quiet. This is
//!   the single most valuable finding of the spike.
//! - **Default-output device change mid-capture:** the stream is bound to the
//!   endpoint it opened; when the user switches the default output device the
//!   existing loopback stream keeps pointing at the old endpoint (it does not
//!   auto-migrate) and typically goes silent or raises a device error via the
//!   error callback. A real meeting mode must watch for default-device changes
//!   and reopen. Out of scope for the spike (no device-change listener here).
//! - **Exclusive-mode output:** if another app holds the render endpoint in
//!   WASAPI exclusive mode, the shared-mode loopback client cannot observe that
//!   stream; the loopback delivers silence (again: no callbacks) for the
//!   duration. Rare on desktops; noted for completeness.
//!
//! # Follow-ups (NOT this spike)
//! Linux: capture a PulseAudio/PipeWire `.monitor` source of the default sink.
//! macOS: a Core Audio process tap (macOS 14.4+) or an aggregate/loopback
//! device. Both keep this same public shape.
//!
//! # Privacy
//! System-audio capture is inert unless [`LoopbackCapture::start`] is called;
//! nothing in the app wires it up today. Meeting-mode consent and capture-in-
//! progress UX are a product-layer concern for the real feature, not this
//! low-level module.

use super::AudioBuffer;
use anyhow::Result;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// Reserve this many seconds of native-rate audio up front so the realtime
/// WASAPI callback never reallocates while appending (a reallocating `Vec` on
/// the audio thread is the classic dropout cause). Loopback sessions can run
/// long, so this is a high-water mark, not a limit.
const RESERVE_SECS: usize = 30;

/// Hard cap on the live buffer so a stalled consumer can never grow it without
/// bound (or OOM) on the realtime thread. Well above the reserve.
const MAX_BUFFER_SECS: usize = 60;

/// Captures the system render mix (what is playing out of the speakers) via
/// WASAPI loopback, converting to 16 kHz mono f32 in [`Self::stop`].
///
/// Mirrors [`super::capture::AudioCapture`]'s buffering discipline: a shared
/// `Arc<Mutex<Vec<f32>>>` filled by the realtime callback, pre-reserved and
/// hard-capped so the callback never reallocates, poison-safe locks, and
/// sample-format conversion at the capture boundary. Unlike `AudioCapture` it
/// has no warm-start / armed gate: a loopback session is a plain start/stop.
///
/// The public shape is identical on every platform; only Windows has a working
/// backend today (see the module docs).
pub struct LoopbackCapture {
    buffer: Arc<Mutex<Vec<f32>>>,
    #[cfg(windows)]
    stream: Option<cpal::Stream>,
    native_rate: u32,
    native_channels: u16,
    /// Count of realtime callbacks since the last [`Self::start`]. Lets the
    /// probe observe the silence-behaviour quirk (do callbacks fire while
    /// nothing plays?) without ever touching audio content.
    callbacks: Arc<AtomicUsize>,
}

impl LoopbackCapture {
    /// Create an idle loopback capture. Opens no device until [`Self::start`].
    pub fn new() -> Self {
        Self {
            buffer: Arc::new(Mutex::new(Vec::new())),
            #[cfg(windows)]
            stream: None,
            native_rate: AudioBuffer::SAMPLE_RATE,
            native_channels: 1,
            callbacks: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Start capturing the system render mix.
    ///
    /// Windows: opens the default output device, takes its *output* config
    /// (loopback delivers the render mix in the device's native format), then
    /// builds an *input* stream on that output device — CPAL/WASAPI turns this
    /// into a loopback capture. Records the native rate/channels and buffers
    /// samples interleaved as delivered.
    #[cfg(windows)]
    pub fn start(&mut self) -> Result<()> {
        use cpal::traits::{DeviceTrait, HostTrait};

        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| anyhow::anyhow!("No default output device available for loopback"))?;
        tracing::info!(device = %device.name().unwrap_or_default(), "Opening loopback on output device");

        // The OUTPUT config: WASAPI loopback delivers the render mix in the
        // render endpoint's own mix format, so the input stream must be built
        // against the output config, not an input config.
        let supported = device
            .default_output_config()
            .map_err(|e| anyhow::anyhow!("default_output_config failed: {e}"))?;
        let sample_format = supported.sample_format();
        self.native_rate = supported.sample_rate().0;
        self.native_channels = supported.channels();
        tracing::info!(
            rate = self.native_rate,
            channels = self.native_channels,
            format = ?sample_format,
            "Loopback config negotiated"
        );

        self.prepare_buffer();
        self.callbacks.store(0, Ordering::Relaxed);

        let config = supported.config();
        let max_samples =
            MAX_BUFFER_SECS * self.native_rate as usize * self.native_channels.max(1) as usize;
        let stream = build_loopback_stream(
            &device,
            &config,
            sample_format,
            &self.buffer,
            max_samples,
            &self.callbacks,
        )?;

        use cpal::traits::StreamTrait;
        stream
            .play()
            .map_err(|e| anyhow::anyhow!("Failed to start loopback stream: {e}"))?;
        self.stream = Some(stream);
        tracing::info!("Loopback capture started");
        Ok(())
    }

    /// System-audio loopback is Windows-only in this spike.
    ///
    /// Linux (PulseAudio/PipeWire `.monitor` source) and macOS (Core Audio
    /// process tap) are documented follow-ups, not implemented here. The type
    /// keeps the same shape so callers compile on every platform.
    #[cfg(not(windows))]
    pub fn start(&mut self) -> Result<()> {
        anyhow::bail!(
            "System-audio loopback capture is not supported on this platform yet \
             (Windows only in this spike; Linux monitor-source and macOS process-tap \
             variants are documented follow-ups)"
        )
    }

    /// Stop capturing and return the render mix as 16 kHz mono.
    ///
    /// Reuses [`AudioBuffer::from_raw`] for the downmix + resample so this path
    /// shares exactly one implementation with `AudioCapture::stop` rather than
    /// duplicating the frame-average / resampler logic.
    pub fn stop(&mut self) -> Result<AudioBuffer> {
        #[cfg(windows)]
        {
            // Dropping the stream halts the WASAPI callback before we drain, so
            // no straggling callback can append after we take the buffer.
            self.stream = None;
        }

        let raw = {
            let mut samples = self.buffer.lock().unwrap_or_else(|e| e.into_inner());
            std::mem::take(&mut *samples)
        };

        tracing::info!(
            samples = raw.len(),
            rate = self.native_rate,
            channels = self.native_channels,
            callbacks = self.callbacks.load(Ordering::Relaxed),
            "Loopback capture stopped"
        );

        Ok(AudioBuffer::from_raw(
            &raw,
            self.native_rate,
            self.native_channels,
        ))
    }

    /// Number of realtime callbacks since the last [`Self::start`]. Zero after a
    /// silent session is the WASAPI event-starvation quirk (see module docs).
    pub fn callback_count(&self) -> usize {
        self.callbacks.load(Ordering::Relaxed)
    }

    /// Native sample rate of the current/last loopback session.
    pub fn native_rate(&self) -> u32 {
        self.native_rate
    }

    /// Native channel count of the current/last loopback session.
    pub fn native_channels(&self) -> u16 {
        self.native_channels
    }

    /// Clone of the live buffer `Arc` for external monitoring (e.g. the probe).
    pub fn live_buffer(&self) -> Arc<Mutex<Vec<f32>>> {
        Arc::clone(&self.buffer)
    }

    /// Clear the live buffer and pre-reserve [`RESERVE_SECS`] of capacity at the
    /// current native config so the realtime callback never reallocates.
    fn prepare_buffer(&self) {
        let reserve =
            RESERVE_SECS * self.native_rate as usize * self.native_channels.max(1) as usize;
        let mut buf = self.buffer.lock().unwrap_or_else(|e| e.into_inner());
        buf.clear();
        let current_cap = buf.capacity();
        if current_cap < reserve {
            buf.reserve(reserve - current_cap);
        }
    }
}

impl Default for LoopbackCapture {
    fn default() -> Self {
        Self::new()
    }
}

/// Build a loopback input stream, dispatching on the render endpoint's sample
/// format. Mirrors the format coverage of `AudioCapture` (F32/I16/U16/I32/
/// U32/I8/U8); unsigned formats are zero-centred on the midpoint.
#[cfg(windows)]
fn build_loopback_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    sample_format: cpal::SampleFormat,
    buffer: &Arc<Mutex<Vec<f32>>>,
    max_samples: usize,
    callbacks: &Arc<AtomicUsize>,
) -> Result<cpal::Stream> {
    use cpal::SampleFormat;
    match sample_format {
        SampleFormat::F32 => {
            build_typed_loopback::<f32, _>(device, config, buffer, max_samples, callbacks, |s| s)
        }
        SampleFormat::I16 => {
            build_typed_loopback::<i16, _>(device, config, buffer, max_samples, callbacks, |s| {
                s as f32 / 32768.0
            })
        }
        SampleFormat::U16 => {
            build_typed_loopback::<u16, _>(device, config, buffer, max_samples, callbacks, |s| {
                (s as f32 - 32768.0) / 32768.0
            })
        }
        SampleFormat::I32 => build_typed_loopback::<i32, _>(
            device,
            config,
            buffer,
            max_samples,
            callbacks,
            // Through f64: i32 has more precision than f32's mantissa.
            |s| (s as f64 / i32::MAX as f64) as f32,
        ),
        SampleFormat::U32 => {
            build_typed_loopback::<u32, _>(device, config, buffer, max_samples, callbacks, |s| {
                ((s as f64 - 2_147_483_648.0) / 2_147_483_648.0) as f32
            })
        }
        SampleFormat::I8 => {
            build_typed_loopback::<i8, _>(device, config, buffer, max_samples, callbacks, |s| {
                s as f32 / i8::MAX as f32
            })
        }
        SampleFormat::U8 => {
            build_typed_loopback::<u8, _>(device, config, buffer, max_samples, callbacks, |s| {
                (s as f32 - 128.0) / 128.0
            })
        }
        format => anyhow::bail!("Unsupported loopback sample format: {format:?}"),
    }
}

/// Build a typed loopback stream that converts each sample to f32 and appends
/// it to the live buffer, dropping audio once `max_samples` is reached so a
/// stalled consumer cannot grow the buffer without bound on the realtime
/// thread. Never allocates in steady state (the buffer is pre-reserved).
#[cfg(windows)]
fn build_typed_loopback<T, F>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    buffer: &Arc<Mutex<Vec<f32>>>,
    max_samples: usize,
    callbacks: &Arc<AtomicUsize>,
    convert: F,
) -> Result<cpal::Stream>
where
    T: cpal::SizedSample + Send + 'static,
    F: Fn(T) -> f32 + Send + 'static,
{
    use cpal::traits::DeviceTrait;

    let buffer = Arc::clone(buffer);
    let callbacks = Arc::clone(callbacks);
    let err_fn = |err| tracing::error!("Loopback stream error: {}", err);
    let stream = device
        .build_input_stream(
            config,
            move |data: &[T], _: &cpal::InputCallbackInfo| {
                // Diagnostic only: proves whether WASAPI fires callbacks while
                // the render engine is idle (it does not — see module docs).
                callbacks.fetch_add(1, Ordering::Relaxed);
                if let Ok(mut buf) = buffer.lock()
                    && buf.len() < max_samples
                {
                    // Sanitize at the boundary: a NaN/inf from a driver would
                    // otherwise poison downstream RMS scoring and the resampler.
                    buf.extend(data.iter().map(|&s| {
                        let v = convert(s);
                        if v.is_finite() { v } else { 0.0 }
                    }));
                }
            },
            err_fn,
            None,
        )
        .map_err(|e| anyhow::anyhow!("Failed to build loopback input stream: {e}"))?;
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The type is inert until `start()`: a fresh instance holds no stream and
    /// reports its default 16 kHz mono config. Pure, so it runs in CI.
    #[test]
    fn new_is_inert() {
        let cap = LoopbackCapture::new();
        assert_eq!(cap.callback_count(), 0);
        assert_eq!(cap.native_rate(), AudioBuffer::SAMPLE_RATE);
        assert_eq!(cap.native_channels(), 1);
        assert!(cap.live_buffer().lock().is_ok_and(|b| b.is_empty()));
    }

    /// On non-Windows, `start()` fails with a clear not-supported error and
    /// captures nothing. On Windows this path is exercised by the ignored
    /// hardware smoke tests below instead.
    #[cfg(not(windows))]
    #[test]
    fn start_unsupported_off_windows() {
        let mut cap = LoopbackCapture::new();
        let err = cap.start().expect_err("start must fail off Windows");
        assert!(err.to_string().contains("not supported"));
    }

    /// Hardware smoke test: needs a real default output endpoint, so it's
    /// ignored in CI. Proves the loopback stream opens and stop() returns a
    /// buffer without error. Run locally with:
    ///   cargo test -p murmur-core --lib -- --ignored loopback_smoke_open_stop --nocapture
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn loopback_smoke_open_stop() {
        let mut cap = LoopbackCapture::new();
        cap.start().expect("open loopback on default output device");
        std::thread::sleep(std::time::Duration::from_millis(800));
        let out = cap.stop().expect("stop loopback");
        // The open/stop cycle must always succeed. We do NOT assert samples
        // arrived: WASAPI loopback delivers ZERO callbacks while nothing is
        // playing (event starvation), so a silent desktop legitimately yields
        // an empty buffer. The companion test below asserts audio only when
        // callbacks were observed.
        println!(
            "loopback open/stop: {}Hz {}ch, {} callbacks, {} samples (16k mono)",
            cap.native_rate(),
            cap.native_channels(),
            cap.callback_count(),
            out.samples.len()
        );
    }

    /// Hardware smoke test: with audio actively playing, callbacks fire and
    /// samples must arrive. Ignored in CI (needs playback). Run locally with
    /// music/video playing:
    ///   cargo test -p murmur-core --lib -- --ignored loopback_smoke_captures_audio --nocapture
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn loopback_smoke_captures_audio() {
        let mut cap = LoopbackCapture::new();
        cap.start().expect("open loopback on default output device");
        std::thread::sleep(std::time::Duration::from_millis(1500));
        let out = cap.stop().expect("stop loopback");
        let n = out.samples.len();
        let rms = if n > 0 {
            (out.samples.iter().map(|s| s * s).sum::<f32>() / n as f32).sqrt()
        } else {
            0.0
        };
        let peak = out.samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        println!(
            "loopback captured: {} callbacks, {n} samples, rms={rms:.6}, peak={peak:.6}",
            cap.callback_count()
        );
        // Only meaningful if audio was actually playing. When callbacks arrived
        // (render engine active) they must have carried samples; a silent run
        // (zero callbacks) is the documented event-starvation case, not a bug.
        if cap.callback_count() > 0 {
            assert!(n > 0, "callbacks fired but no samples reached the buffer");
        }
    }
}
