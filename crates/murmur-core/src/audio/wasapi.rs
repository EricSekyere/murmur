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
    IMMDeviceEnumerator, MMDeviceEnumerator, eCapture, eCommunications,
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
        let device = enumerator
            .GetDefaultAudioEndpoint(eCapture, eCommunications)
            .context("No default communications capture device")?;
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

        let ch = channels.max(1) as usize;
        while !stop.load(Ordering::Acquire) {
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
                let total = frames as usize * ch;
                if total > 0
                    && let Ok(mut buf) = buffer.lock()
                    && buf.len() < max_samples
                {
                    if flags & AUDCLNT_BUFFERFLAGS_SILENT != 0 {
                        buf.extend(std::iter::repeat_n(0.0, total));
                    } else {
                        push_samples(&mut buf, data, total, bits, is_float);
                    }
                }
                let _ = capture.ReleaseBuffer(frames);
            }
        }

        let _ = client.Stop();
        let _ = CloseHandle(event);
    }
    Ok(())
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
