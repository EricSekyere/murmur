//! Windows voice capture with built-in echo cancellation.
//!
//! Opens the default communications microphone via WASAPI in the
//! "communications" stream category, which makes Windows apply its echo
//! cancellation + noise suppression (the same processing conferencing apps
//! use). That removes the speaker audio the mic would otherwise pick up. The
//! caller falls back to the raw CPAL mic if anything here fails.

use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use anyhow::{Context, Result, bail};
use windows::Win32::Foundation::{CloseHandle, FALSE, WAIT_OBJECT_0};
use windows::Win32::Media::Audio::{
    AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMOPTIONS_NONE,
    AudioCategory_Communications, AudioClientProperties, IAudioCaptureClient, IAudioClient2,
    IAudioRenderClient, IMMDeviceEnumerator, MMDeviceEnumerator, eCapture, eConsole, eRender,
};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
    CoUninitialize,
};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
use windows::core::PCWSTR;

const WAVE_FORMAT_IEEE_FLOAT: u16 = 0x0003;
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;
const AUDCLNT_BUFFERFLAGS_SILENT: u32 = 0x2;

/// Handle to a running voice-capture session. Dropping it stops the capture
/// thread and waits for it to finish.
pub struct WasapiVoiceCapture {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for WasapiVoiceCapture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Start communications-mode capture into `buffer` (interleaved native f32).
/// Returns the handle plus the negotiated `(sample_rate, channels)`.
pub fn open_voice_capture(
    buffer: Arc<Mutex<Vec<f32>>>,
    max_samples: usize,
) -> Result<(WasapiVoiceCapture, u32, u16)> {
    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = Arc::clone(&stop);
    let (tx, rx) = std::sync::mpsc::channel::<Result<(u32, u16), String>>();

    let thread = std::thread::Builder::new()
        .name("wasapi-voice-capture".into())
        .spawn(move || {
            // All COM objects live on this thread, so nothing crosses threads.
            if let Err(e) = capture_loop(buffer, max_samples, stop_thread, &tx) {
                let _ = tx.send(Err(format!("{e:#}")));
            }
        })
        .context("Failed to spawn voice capture thread")?;

    match rx.recv() {
        Ok(Ok((rate, channels))) => Ok((
            WasapiVoiceCapture {
                stop,
                thread: Some(thread),
            },
            rate,
            channels,
        )),
        Ok(Err(e)) => {
            stop.store(true, Ordering::Release);
            let _ = thread.join();
            bail!("voice capture init failed: {e}");
        }
        Err(_) => {
            let _ = thread.join();
            bail!("voice capture thread exited before initializing");
        }
    }
}

/// Frees COM on thread exit no matter how we leave the loop.
struct ComGuard;
impl Drop for ComGuard {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

fn capture_loop(
    buffer: Arc<Mutex<Vec<f32>>>,
    max_samples: usize,
    stop: Arc<AtomicBool>,
    init_tx: &Sender<Result<(u32, u16), String>>,
) -> Result<()> {
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED)
            .ok()
            .context("CoInitializeEx failed")?;
        let _com = ComGuard;

        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                .context("Create MMDeviceEnumerator failed")?;
        // AEC comes from the Communications *category* (SetClientProperties below),
        // not the endpoint role. Use the same default endpoint the raw CPAL path
        // uses (eConsole) so AEC processes the mic the user actually dictates into.
        // The Communications default can be a different, silent device (a muted
        // headset, a webcam mic), which is a classic cause of digital silence.
        let device = enumerator
            .GetDefaultAudioEndpoint(eCapture, eConsole)
            .context("No default capture device")?;
        let client: IAudioClient2 = device
            .Activate(CLSCTX_ALL, None)
            .context("Activate IAudioClient2 failed")?;

        // Communications category = Windows applies AEC + noise suppression.
        let props = AudioClientProperties {
            cbSize: std::mem::size_of::<AudioClientProperties>() as u32,
            bIsOffload: FALSE,
            eCategory: AudioCategory_Communications,
            Options: AUDCLNT_STREAMOPTIONS_NONE,
        };
        client
            .SetClientProperties(&props)
            .context("SetClientProperties(Communications) failed")?;

        let format_ptr = client.GetMixFormat().context("GetMixFormat failed")?;
        let format = *format_ptr;
        // Copy out of the packed WAVEFORMATEX before use (field refs are unaligned).
        let channels = format.nChannels;
        let rate = format.nSamplesPerSec;
        let bits = format.wBitsPerSample;
        let tag = format.wFormatTag;
        let is_float =
            tag == WAVE_FORMAT_IEEE_FLOAT || (tag == WAVE_FORMAT_EXTENSIBLE && bits == 32);
        tracing::debug!(
            "Voice capture format: tag={tag:#06x} {channels}ch {rate}Hz {bits}-bit float={is_float}"
        );

        let event =
            CreateEventW(None, FALSE, FALSE, PCWSTR::null()).context("CreateEventW failed")?;

        let init = client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
            0,
            0,
            format_ptr,
            None,
        );
        CoTaskMemFree(Some(format_ptr as *const _));
        init.context("IAudioClient::Initialize failed")?;

        client
            .SetEventHandle(event)
            .context("SetEventHandle failed")?;
        let capture: IAudioCaptureClient =
            client.GetService().context("GetService(capture) failed")?;
        client.Start().context("IAudioClient::Start failed")?;

        // Initialized: hand the negotiated format back so the caller can return.
        let _ = init_tx.send(Ok((rate, channels)));

        // AEC echo reference: the Windows Communications AEC gates its capture to
        // digital silence unless a render stream is also active in the same comms
        // session to serve as the echo reference. Open a silent render keep-alive
        // so the AEC passes the mic through. Best-effort; capture runs without it.
        let render = match setup_silence_render(&enumerator, &props) {
            Ok(r) => Some(r),
            Err(e) => {
                tracing::warn!("AEC render reference unavailable: {e:#}");
                None
            }
        };

        let ch = channels.max(1) as usize;
        // Diagnostics: how much audio the AEC pipeline actually delivered, and how
        // much of it the engine flagged as silence. Reveals on a real repro whether
        // the comms/AEC path is the source of the digital-silence sessions.
        let mut captured_frames: u64 = 0;
        let mut silent_frames: u64 = 0;
        while !stop.load(Ordering::Acquire) {
            // Keep the AEC reference fed with silence so it never underruns.
            if let Some((rclient, rrender, bufsize)) = render.as_ref()
                && let Ok(padding) = rclient.GetCurrentPadding()
            {
                let avail = bufsize.saturating_sub(padding);
                if avail > 0 && rrender.GetBuffer(avail).is_ok() {
                    let _ = rrender.ReleaseBuffer(avail, AUDCLNT_BUFFERFLAGS_SILENT);
                }
            }
            if WaitForSingleObject(event, 200) != WAIT_OBJECT_0 {
                continue;
            }
            loop {
                let next = capture.GetNextPacketSize().unwrap_or(0);
                if next == 0 {
                    break;
                }
                let mut data: *mut u8 = ptr::null_mut();
                let mut frames: u32 = 0;
                let mut flags: u32 = 0;
                if capture
                    .GetBuffer(&mut data, &mut frames, &mut flags, None, None)
                    .is_err()
                {
                    break;
                }
                let silent = flags & AUDCLNT_BUFFERFLAGS_SILENT != 0;
                captured_frames += frames as u64;
                if silent {
                    silent_frames += frames as u64;
                }
                let total = frames as usize * ch;
                if total > 0
                    && let Ok(mut buf) = buffer.lock()
                    && buf.len() < max_samples
                {
                    if silent {
                        buf.extend(std::iter::repeat_n(0.0, total));
                    } else {
                        push_samples(&mut buf, data, total, bits, is_float);
                    }
                }
                let _ = capture.ReleaseBuffer(frames);
            }
        }

        let silent_pct = if captured_frames > 0 {
            silent_frames * 100 / captured_frames
        } else {
            0
        };
        tracing::info!(
            "Voice capture ended: {captured_frames} frames ({silent_pct}% flagged silent by the AEC)"
        );

        if let Some((rclient, _, _)) = render.as_ref() {
            let _ = rclient.Stop();
        }
        let _ = client.Stop();
        let _ = CloseHandle(event);
    }
    Ok(())
}

/// Open a silent shared-mode render stream on the default endpoint in the
/// Communications category. Its only job is to give the capture-side AEC its
/// echo reference so the AEC passes the mic through instead of zeroing it.
fn setup_silence_render(
    enumerator: &IMMDeviceEnumerator,
    props: &AudioClientProperties,
) -> Result<(IAudioClient2, IAudioRenderClient, u32)> {
    unsafe {
        let device = enumerator
            .GetDefaultAudioEndpoint(eRender, eConsole)
            .context("No default render endpoint")?;
        let client: IAudioClient2 = device
            .Activate(CLSCTX_ALL, None)
            .context("Activate render")?;
        client
            .SetClientProperties(props)
            .context("render SetClientProperties")?;
        let fmt = client.GetMixFormat().context("render GetMixFormat")?;
        let init = client.Initialize(AUDCLNT_SHAREMODE_SHARED, 0, 0, 0, fmt, None);
        CoTaskMemFree(Some(fmt as *const _));
        init.context("render Initialize")?;
        let render: IAudioRenderClient = client.GetService().context("render GetService")?;
        let bufsize = client.GetBufferSize().context("render GetBufferSize")?;
        // Pre-roll the whole buffer with silence so it starts cleanly.
        let _ = render.GetBuffer(bufsize).context("render GetBuffer")?;
        render
            .ReleaseBuffer(bufsize, AUDCLNT_BUFFERFLAGS_SILENT)
            .context("render ReleaseBuffer")?;
        client.Start().context("render Start")?;
        Ok((client, render, bufsize))
    }
}

/// Convert `count` interleaved native samples at `data` to f32 and append them.
unsafe fn push_samples(
    buf: &mut Vec<f32>,
    data: *const u8,
    count: usize,
    bits: u16,
    is_float: bool,
) {
    match (is_float, bits) {
        (true, 32) => {
            let p = data as *const f32;
            for i in 0..count {
                let v = unsafe { *p.add(i) };
                buf.push(if v.is_finite() { v } else { 0.0 });
            }
        }
        (false, 16) => {
            let p = data as *const i16;
            for i in 0..count {
                buf.push(unsafe { *p.add(i) } as f32 / 32768.0);
            }
        }
        (false, 32) => {
            let p = data as *const i32;
            for i in 0..count {
                buf.push((unsafe { *p.add(i) } as f64 / i32::MAX as f64) as f32);
            }
        }
        _ => buf.extend(std::iter::repeat_n(0.0, count)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Manual smoke test: needs a real mic + a default communications endpoint,
    /// so it's ignored in CI. Run locally with:
    ///   cargo test -p murmur-core wasapi_smoke -- --ignored --nocapture
    #[test]
    #[ignore]
    fn wasapi_smoke_captures_audio() {
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let (cap, rate, channels) =
            open_voice_capture(Arc::clone(&buffer), 10_000_000).expect("open voice capture");
        std::thread::sleep(Duration::from_millis(800));
        drop(cap);
        let samples = buffer.lock().unwrap();
        let n = samples.len();
        let rms = if n > 0 {
            (samples.iter().map(|s| s * s).sum::<f32>() / n as f32).sqrt()
        } else {
            0.0
        };
        let max = samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        println!("voice capture: {rate}Hz {channels}ch, {n} samples, rms={rms:.6}, max={max:.6}");
        assert!(n > 0, "voice capture produced no samples");
    }
}
