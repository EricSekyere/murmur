use super::AudioBuffer;
use super::warm::{ConfigCache, SessionState, StreamKey};
use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, SupportedStreamConfig};
use std::sync::{Arc, Mutex};

/// Per-run AEC health: 0 = unknown (probe next session), 1 = delivers signal,
/// -1 = only silence (use the raw mic for the rest of the run).
#[cfg(windows)]
static AEC_STATE: std::sync::atomic::AtomicI8 = std::sync::atomic::AtomicI8::new(0);

/// Consecutive first-session AEC probes that saw only silence. The probe can
/// misjudge a working AEC when the user simply hasn't spoken yet, so a single
/// silent probe uses the raw mic for that session but re-tries AEC next
/// session; only after this many silent probes is AEC given up on for the run.
#[cfg(windows)]
static AEC_SILENT_PROBES: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// Give up on AEC for the run after this many consecutive silent probes.
#[cfg(windows)]
const AEC_MAX_SILENT_PROBES: u8 = 3;

/// How long a probe waits for a signal-bearing sample before falling back to
/// the raw mic for that session. Long enough to span the user's speech onset
/// after the hotkey (a driver that gates idle audio to zero delivers only
/// silence until then); bounded so a silent start can't stall dictation. On a
/// genuinely broken AEC this is also the worst-case audio lost before fallback.
#[cfg(windows)]
const AEC_PROBE_DEADLINE_MS: u64 = 800;

/// Per-run health of the Linux echo-cancel path, mirroring `AEC_STATE`:
/// 0 = unprobed, 1 = working, -1 = unavailable (missing pactl/parec, or the
/// server refused the module) — don't re-spawn subprocesses every session.
#[cfg(target_os = "linux")]
static PULSE_AEC_STATE: std::sync::atomic::AtomicI8 = std::sync::atomic::AtomicI8::new(0);

/// Reserve enough live-buffer capacity for this many seconds of audio so the
/// realtime cpal callback never reallocates during `extend`. A reallocating
/// Vec on the audio thread is the textbook cause of audible dropouts. The
/// consumer drains the buffer routinely, so this is a high-water mark.
const RESERVE_SECS: usize = 30;

/// Hard cap on the live buffer so a stalled consumer can never grow it
/// without bound and reallocate (or OOM) on the realtime thread. Well
/// above the reserve, so normal operation never reaches it.
const MAX_BUFFER_SECS: usize = 60;

/// Whether a freshly opened CPAL stream begins buffering for a session or
/// sits idle discarding samples (mic pre-warm).
enum ArmMode {
    Armed,
    Idle,
}

/// Cached outcome of device selection + config negotiation for a
/// [`StreamKey`], letting repeat opens skip the CPAL HAL queries.
struct CachedDevice {
    device: cpal::Device,
    config: SupportedStreamConfig,
}

/// Manages microphone capture via CPAL.
///
/// Enumerates the device's supported configs, picks the best one,
/// then converts to 16 kHz mono f32 in `stop()`.
///
/// With warm-start enabled ([`Self::set_warm_start`]) the CPAL stream stays
/// open between sessions to eliminate the cold device-open latency that clips
/// the user's first word. While idle-but-warm the callback discards every
/// sample immediately (see [`SessionState`]); audio only accumulates while a
/// session is armed, and the buffer is cleared at arm time so a session never
/// sees pre-session audio.
pub struct AudioCapture {
    buffer: Arc<Mutex<Vec<f32>>>,
    stream: Option<cpal::Stream>,
    /// Active Windows voice-capture session (echo cancellation), when used.
    #[cfg(windows)]
    voice: Option<super::wasapi::WasapiVoiceCapture>,
    /// Active Linux echo-cancelled capture (PulseAudio/PipeWire), when used.
    #[cfg(target_os = "linux")]
    voice: Option<super::pulse_aec::PulseAecCapture>,
    native_rate: u32,
    native_channels: u16,
    /// Armed/discard flag and TTFC metric shared with the CPAL callback.
    session: Arc<SessionState>,
    /// Keep the CPAL stream open between sessions (mic pre-warm).
    warm_enabled: bool,
    /// Key the currently open CPAL stream was built for; a session may only
    /// warm-reuse the stream when its key matches exactly.
    open_key: Option<StreamKey>,
    config_cache: ConfigCache<CachedDevice>,
}

impl AudioCapture {
    /// Create a new AudioCapture using the default input device.
    pub fn new() -> Result<Self> {
        Ok(Self {
            buffer: Arc::new(Mutex::new(Vec::new())),
            stream: None,
            #[cfg(any(windows, target_os = "linux"))]
            voice: None,
            native_rate: AudioBuffer::SAMPLE_RATE,
            native_channels: 1,
            session: Arc::new(SessionState::new()),
            warm_enabled: false,
            open_key: None,
            config_cache: ConfigCache::new(),
        })
    }

    /// Start recording. With `echo_cancellation` on, prefer the OS voice-capture
    /// path (Windows AEC) on the default mic; otherwise, or on failure, use the
    /// raw CPAL microphone.
    pub fn start(&mut self, preferred_device: Option<&str>, echo_cancellation: bool) -> Result<()> {
        // Timestamp before any device work so a cold start's HAL queries and
        // stream build all count toward the time-to-first-chunk metric.
        self.session.start_session();
        #[cfg(any(windows, target_os = "linux"))]
        {
            self.voice = None;
        }

        let on_default = preferred_device
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .is_none();
        if echo_cancellation && on_default && self.try_start_voice_capture().is_some() {
            // The voice path feeds the buffer from its own OS capture loop,
            // bypassing the CPAL callback, so warm reuse and the armed gate
            // don't apply to it; keeping a second, idle mic handle open next
            // to it would buy nothing, so drop any warm stream.
            self.stream = None;
            self.open_key = None;
            return Ok(());
        }

        self.start_cpal(preferred_device, echo_cancellation)
    }

    /// Enable or disable warm-start (keep the mic stream open between
    /// sessions). Disabling releases an idle-open stream immediately; a live
    /// session keeps recording and `stop()` honours the new flag.
    pub fn set_warm_start(&mut self, enabled: bool) {
        self.warm_enabled = enabled;
        if !enabled && !self.session.is_armed() {
            self.stream = None;
            self.open_key = None;
        }
    }

    /// Open the input stream idle (discarding every sample) so the next
    /// session skips the device open entirely. No-op unless warm-start is
    /// enabled, and when a session is active. Skipped when the session would
    /// use the OS voice-capture path (echo cancellation on the default
    /// device): that path opens its own capture, so a pre-warmed raw stream
    /// would never be reused.
    pub fn prewarm(
        &mut self,
        preferred_device: Option<&str>,
        echo_cancellation: bool,
    ) -> Result<()> {
        if !self.warm_enabled || self.is_recording() {
            return Ok(());
        }
        let on_default = preferred_device
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .is_none();
        if echo_cancellation && on_default {
            self.stream = None;
            self.open_key = None;
            return Ok(());
        }
        let key = StreamKey::new(preferred_device, echo_cancellation);
        if self.stream.is_some() {
            if self.open_key.as_ref() == Some(&key) {
                return Ok(());
            }
            // Device or echo-cancellation input changed: retarget the warm
            // stream rather than keep the old device open.
            self.stream = None;
            self.open_key = None;
        }
        self.open_cpal_stream(preferred_device, &key, ArmMode::Idle)?;
        tracing::info!("Microphone pre-warmed (stream open, audio discarded until a session arms)");
        Ok(())
    }

    /// Try the OS echo-cancelling capture path. Returns `Some(())` on success.
    /// Windows uses the WASAPI Communications AEC; Linux loads PulseAudio/
    /// PipeWire `module-echo-cancel` and captures its cancelled source. macOS
    /// has no implementation yet (returns `None`, so the caller falls back to
    /// the raw mic). Plan: the VoiceProcessingIO AudioUnit on macOS.
    fn try_start_voice_capture(&mut self) -> Option<()> {
        #[cfg(windows)]
        {
            use std::sync::atomic::Ordering;
            const MAX_SAMPLES: usize = 60 * 48_000 * 2;
            // This machine's Communications AEC already proved silent this run.
            if AEC_STATE.load(Ordering::Relaxed) < 0 {
                return None;
            }
            if let Ok(mut buf) = self.buffer.lock() {
                buf.clear();
                let want = 30 * 48_000 * 2;
                let cap = buf.capacity();
                if cap < want {
                    buf.reserve(want - cap);
                }
            }
            match super::wasapi::open_voice_capture(Arc::clone(&self.buffer), MAX_SAMPLES) {
                Ok((cap, rate, channels)) => {
                    self.native_rate = rate;
                    self.native_channels = channels;
                    self.voice = Some(cap);
                    tracing::info!(
                        "Voice capture (echo cancellation) active: {}Hz, {} channel(s)",
                        rate,
                        channels
                    );
                    // Confirm the AEC actually delivers signal before trusting
                    // it. Some Windows stacks' Communications AEC emits only
                    // digital zeros even with a render reference. Wait for a
                    // signal-bearing sample rather than sampling a fixed window:
                    // right after the hotkey the user is usually still silent,
                    // and a driver that gates idle audio to zero would fail an
                    // immediate sample even when its AEC passes speech fine, so
                    // polling for the first real sample lets a working AEC prove
                    // itself once the user speaks. A fully silent probe uses the
                    // raw mic for this session but re-tries AEC next session, so
                    // one silent start can't demote a working AEC for the whole
                    // run; AEC is abandoned only after AEC_MAX_SILENT_PROBES.
                    if AEC_STATE.load(Ordering::Relaxed) == 0 {
                        let deadline = std::time::Instant::now()
                            + std::time::Duration::from_millis(AEC_PROBE_DEADLINE_MS);
                        let mut alive = false;
                        while std::time::Instant::now() < deadline {
                            std::thread::sleep(std::time::Duration::from_millis(30));
                            alive = self
                                .buffer
                                .lock()
                                .map(|b| b.iter().any(|s| s.abs() > 1e-5))
                                .unwrap_or(false);
                            if alive {
                                break;
                            }
                        }
                        if alive {
                            AEC_STATE.store(1, Ordering::Relaxed);
                            AEC_SILENT_PROBES.store(0, Ordering::Relaxed);
                        } else {
                            let silent = AEC_SILENT_PROBES.fetch_add(1, Ordering::Relaxed) + 1;
                            if silent >= AEC_MAX_SILENT_PROBES {
                                AEC_STATE.store(-1, Ordering::Relaxed);
                                tracing::warn!(
                                    silent_probes = silent,
                                    "Echo cancellation delivered only silence across probes; using the raw microphone for this run"
                                );
                            } else {
                                tracing::info!(
                                    probe = silent,
                                    max = AEC_MAX_SILENT_PROBES,
                                    "Echo cancellation delivered no signal within {}ms; using the raw microphone for this session, will retry AEC next session",
                                    AEC_PROBE_DEADLINE_MS
                                );
                            }
                            self.voice = None;
                            if let Ok(mut b) = self.buffer.lock() {
                                b.clear();
                            }
                            return None;
                        }
                    }
                    Some(())
                }
                Err(e) => {
                    tracing::warn!("Voice capture unavailable ({e}); using raw microphone");
                    None
                }
            }
        }
        #[cfg(target_os = "linux")]
        {
            use std::sync::atomic::Ordering;
            // Mono at the fixed parec rate; same 60 s hard cap as the raw path.
            const MAX_SAMPLES: usize = 60 * super::pulse_aec::RATE as usize;
            // This run already proved the pulse AEC path unusable.
            if PULSE_AEC_STATE.load(Ordering::Relaxed) < 0 {
                return None;
            }
            if let Ok(mut buf) = self.buffer.lock() {
                buf.clear();
                let want = 30 * super::pulse_aec::RATE as usize;
                let cap = buf.capacity();
                if cap < want {
                    buf.reserve(want - cap);
                }
            }
            match super::pulse_aec::open(Arc::clone(&self.buffer), MAX_SAMPLES) {
                Ok(mut cap) => {
                    // First session this run: parec exits immediately when the
                    // source or server is unusable, so a short liveness check
                    // catches that. Silence is NOT treated as failure here (a
                    // muted mic is legitimate), unlike the Windows AEC probe
                    // which guards a known emit-zeros bug.
                    if PULSE_AEC_STATE.load(Ordering::Relaxed) == 0 {
                        std::thread::sleep(std::time::Duration::from_millis(250));
                        if cap.is_alive() {
                            PULSE_AEC_STATE.store(1, Ordering::Relaxed);
                        } else {
                            PULSE_AEC_STATE.store(-1, Ordering::Relaxed);
                            tracing::warn!(
                                "Echo-cancelled capture exited immediately; using the raw microphone instead"
                            );
                            if let Ok(mut b) = self.buffer.lock() {
                                b.clear();
                            }
                            return None;
                        }
                    }
                    self.native_rate = super::pulse_aec::RATE;
                    self.native_channels = super::pulse_aec::CHANNELS;
                    self.voice = Some(cap);
                    tracing::info!(
                        "Voice capture (echo cancellation) active: {}Hz, {} channel(s)",
                        super::pulse_aec::RATE,
                        super::pulse_aec::CHANNELS
                    );
                    Some(())
                }
                Err(e) => {
                    // Missing pactl/parec or a refused module load will not fix
                    // itself this run; remember so later sessions skip the
                    // subprocess round-trips.
                    PULSE_AEC_STATE.store(-1, Ordering::Relaxed);
                    tracing::warn!(
                        "Echo-cancelled capture unavailable ({e}); using raw microphone"
                    );
                    None
                }
            }
        }
        #[cfg(not(any(windows, target_os = "linux")))]
        {
            None
        }
    }

    /// Raw microphone capture via CPAL (no echo cancellation). Reuses an
    /// idle warm stream when its key matches; otherwise opens one, going
    /// through the device-config cache when possible.
    fn start_cpal(
        &mut self,
        preferred_device: Option<&str>,
        echo_cancellation: bool,
    ) -> Result<()> {
        let key = StreamKey::new(preferred_device, echo_cancellation);

        if self.warm_enabled && self.stream.is_some() && self.open_key.as_ref() == Some(&key) {
            // Clear before arming so the session never sees pre-session
            // audio; the disarmed callback cannot append concurrently.
            self.prepare_buffer();
            self.session.arm(true);
            tracing::info!("Audio capture started (warm stream reused)");
            return Ok(());
        }

        // No reusable stream (cold start, warm disabled, or key changed).
        self.stream = None;
        self.open_key = None;
        self.open_cpal_stream(preferred_device, &key, ArmMode::Armed)?;
        tracing::info!("Audio capture started");
        Ok(())
    }

    /// Build and play a CPAL stream for `key`, preferring the cached device +
    /// config. Any failure opening from the cache falls back to fresh HAL
    /// queries (self-heal: a stale handle after a device unplug or a changed
    /// driver default must not wedge dictation); a successful open re-stores
    /// the entry.
    fn open_cpal_stream(
        &mut self,
        preferred_device: Option<&str>,
        key: &StreamKey,
        arm: ArmMode,
    ) -> Result<()> {
        if let Some(cached) = self.config_cache.take(key) {
            match self.open_with(&cached.device, cached.config.clone(), key, &arm) {
                Ok(()) => {
                    tracing::debug!("Opened input stream from cached device config");
                    self.config_cache.store(key.clone(), cached);
                    return Ok(());
                }
                Err(e) => {
                    tracing::warn!(
                        "Cached audio device/config failed to open ({e}); re-querying the device"
                    );
                }
            }
        }

        let host = cpal::default_host();
        let device = select_input_device(&host, preferred_device)?;
        tracing::info!("Using input device: {}", device.name()?);

        let supported = choose_input_config(&device)?;
        tracing::info!(
            "Selected config: {}Hz, {} channel(s), format: {:?}",
            supported.sample_rate().0,
            supported.channels(),
            supported.sample_format()
        );

        self.open_with(&device, supported.clone(), key, &arm)?;
        self.config_cache.store(
            key.clone(),
            CachedDevice {
                device,
                config: supported,
            },
        );
        Ok(())
    }

    /// Build, arm (unless pre-warming), and play a stream from a concrete
    /// device + config, recording its key for warm reuse.
    fn open_with(
        &mut self,
        device: &cpal::Device,
        supported: SupportedStreamConfig,
        key: &StreamKey,
        arm: &ArmMode,
    ) -> Result<()> {
        let sample_format = supported.sample_format();
        self.native_rate = supported.sample_rate().0;
        self.native_channels = supported.channels();
        self.prepare_buffer();

        let config = supported.config();
        let max_samples =
            MAX_BUFFER_SECS * self.native_rate as usize * self.native_channels.max(1) as usize;

        let stream = build_input_stream_for_format(
            device,
            &config,
            sample_format,
            &self.buffer,
            max_samples,
            &self.session,
        )?;

        // Arm before play so the very first callback already buffers (and
        // records TTFC); a pre-warm stays disarmed and discards everything.
        if matches!(arm, ArmMode::Armed) {
            self.session.arm(false);
        }
        stream.play()?;
        self.stream = Some(stream);
        self.open_key = Some(key.clone());
        Ok(())
    }

    /// Clear the live buffer and pre-reserve [`RESERVE_SECS`] of capacity at
    /// the current native config so the realtime callback never reallocates.
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

    /// Stop recording and return the captured audio buffer (16 kHz mono).
    ///
    /// In warm mode the CPAL stream stays open for the next session; only
    /// the armed flag flips, returning the callback to discard mode.
    pub fn stop(&mut self) -> Result<AudioBuffer> {
        // Disarm before draining: the callback re-checks the flag under the
        // buffer lock, so no straggling callback can append audio after the
        // drain and leave it sitting in the idle warm buffer.
        self.session.disarm();
        if !self.warm_enabled {
            self.stream = None;
            self.open_key = None;
        }
        #[cfg(any(windows, target_os = "linux"))]
        {
            self.voice = None;
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
            super::dsp::resample(&mono, self.native_rate, target_rate)
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

    /// Check if currently recording. An idle warm stream (open but disarmed,
    /// discarding every sample) does not count as recording.
    pub fn is_recording(&self) -> bool {
        #[cfg(any(windows, target_os = "linux"))]
        if self.voice.is_some() {
            return true;
        }
        self.stream.is_some() && self.session.is_armed()
    }

    /// Whether the OS echo-cancelling capture path is the active source (as
    /// opposed to the raw CPAL mic it falls back to). Calibration uses this to
    /// skip the gain boost the raw mic needs: the voice stream is already
    /// AGC-leveled by the OS, so boosting it just re-amplifies idle noise.
    pub fn echo_cancellation_active(&self) -> bool {
        #[cfg(any(windows, target_os = "linux"))]
        {
            self.voice.is_some()
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
    session: &Arc<SessionState>,
) -> Result<cpal::Stream> {
    // Zero-center for unsigned formats: silence sits at the midpoint, not 0.
    match sample_format {
        SampleFormat::F32 => {
            build_typed_stream::<f32, _>(device, config, buffer, max_samples, session, |s| s)
        }
        SampleFormat::I16 => {
            build_typed_stream::<i16, _>(device, config, buffer, max_samples, session, |s| {
                s as f32 / 32768.0
            })
        }
        SampleFormat::U16 => {
            build_typed_stream::<u16, _>(device, config, buffer, max_samples, session, |s| {
                (s as f32 - 32768.0) / 32768.0
            })
        }
        SampleFormat::I32 => {
            build_typed_stream::<i32, _>(device, config, buffer, max_samples, session, |s| {
                // Convert through f64: an i32 has more precision than f32's
                // mantissa, so dividing in f32 would lose low bits.
                (s as f64 / i32::MAX as f64) as f32
            })
        }
        SampleFormat::U32 => {
            build_typed_stream::<u32, _>(device, config, buffer, max_samples, session, |s| {
                ((s as f64 - 2_147_483_648.0) / 2_147_483_648.0) as f32
            })
        }
        SampleFormat::I8 => {
            build_typed_stream::<i8, _>(device, config, buffer, max_samples, session, |s| {
                s as f32 / i8::MAX as f32
            })
        }
        SampleFormat::U8 => {
            build_typed_stream::<u8, _>(device, config, buffer, max_samples, session, |s| {
                (s as f32 - 128.0) / 128.0
            })
        }
        format => anyhow::bail!("Unsupported sample format: {:?}", format),
    }
}

/// Build an input stream that converts each sample to f32 and appends it to
/// the live buffer, dropping new audio once the buffer hits `max_samples` so a
/// stalled consumer cannot grow it without bound on the realtime thread.
///
/// Privacy invariant: while the session is disarmed (idle warm stream) every
/// sample is discarded immediately — no lock taken, no allocation, nothing
/// buffered or written anywhere.
fn build_typed_stream<T, F>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    buffer: &Arc<Mutex<Vec<f32>>>,
    max_samples: usize,
    session: &Arc<SessionState>,
    convert: F,
) -> Result<cpal::Stream>
where
    T: cpal::SizedSample + Send + 'static,
    F: Fn(T) -> f32 + Send + 'static,
{
    let buffer = Arc::clone(buffer);
    let session = Arc::clone(session);
    let err_fn = |err| tracing::error!("Audio stream error: {}", err);
    let stream = device.build_input_stream(
        config,
        move |data: &[T], _: &cpal::InputCallbackInfo| {
            if !session.is_armed() {
                return;
            }
            if let Some((ttfc_ms, warm)) = session.on_delivery() {
                // Once per session; the metric is sample count/time only,
                // never audio content.
                tracing::debug!(ttfc_ms, warm, "first audio chunk");
            }
            // Re-check armed under the lock: stop() disarms before draining,
            // so a callback that raced the disarm cannot append audio after
            // the drain and leave it buffered in an idle warm stream.
            if let Ok(mut buf) = buffer.lock()
                && session.is_armed()
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Manual warm-cycle smoke test: needs a real input device, so it's
    /// ignored in CI. Run locally with:
    ///   cargo test -p murmur-core warm_cycle -- --ignored --nocapture
    #[test]
    #[ignore]
    fn warm_cycle_reuses_stream_and_discards_while_idle() {
        let mut capture = AudioCapture::new().expect("create capture");
        capture.set_warm_start(true);

        capture.start(None, false).expect("cold start");
        std::thread::sleep(Duration::from_millis(400));
        let first = capture.stop().expect("stop first session");
        assert!(!first.samples.is_empty(), "cold session captured no audio");

        // Warm mode keeps the stream open but idle between sessions...
        assert!(capture.stream.is_some(), "warm stream was torn down");
        assert!(
            !capture.is_recording(),
            "idle warm stream reads as recording"
        );

        // ...and the idle stream must discard everything (privacy invariant).
        std::thread::sleep(Duration::from_millis(300));
        let idle_len = capture
            .live_buffer()
            .lock()
            .map(|b| b.len())
            .unwrap_or(usize::MAX);
        assert_eq!(idle_len, 0, "idle warm stream buffered audio");

        capture.start(None, false).expect("warm start");
        std::thread::sleep(Duration::from_millis(400));
        let second = capture.stop().expect("stop second session");
        assert!(!second.samples.is_empty(), "warm session captured no audio");

        // Disabling warm-start releases the idle stream immediately.
        capture.set_warm_start(false);
        assert!(capture.stream.is_none(), "disable did not release the mic");
    }
}
