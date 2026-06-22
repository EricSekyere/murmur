//! Linux voice capture with echo cancellation via PulseAudio / PipeWire.
//!
//! Loads `module-echo-cancel` (WebRTC AEC) against the default sink and
//! source, then records the cleaned virtual source with `parec`. That removes
//! the speaker audio the mic would otherwise pick up, the same effect the
//! Windows communications-mode path gives. The module and `parec` are driven
//! through `pactl`/`parec`, which ship with PulseAudio and with PipeWire's
//! Pulse compatibility layer, so this works on both. Anything missing or
//! failing returns an error and the caller falls back to the raw CPAL mic.
//!
//! Phase 1 is deliberately process-based (no libpulse FFI): zero new compiled
//! dependencies and a clean fallback. A later pass can move to `libpulse`
//! bindings to drop the runtime dependency on the `pactl`/`parec` CLIs.

use std::io::Read;
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

/// Format we ask `parec` to deliver. Mono means no downmix; 48 kHz is a clean
/// 3:1 decimation to the pipeline's 16 kHz. The capture buffer holds native
/// interleaved f32, which for mono is just the samples.
const RATE: u32 = 48_000;
const CHANNELS: u16 = 1;

/// Our predictably-named echo-cancel source. Reusing one name across sessions
/// avoids stacking duplicate modules if a prior run left one loaded.
const SOURCE_NAME: &str = "murmur_echo_cancel";

/// How long to wait for the first samples before declaring the source dead and
/// falling back to the raw mic.
const READY_TIMEOUT: Duration = Duration::from_millis(800);

/// Handle to a running voice-capture session. Dropping it stops `parec`, joins
/// the reader thread, and unloads the echo-cancel module if we loaded it.
pub struct PulseVoiceCapture {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    child: Option<Child>,
    /// Module index we loaded, or `None` when we reused an existing source (we
    /// must not unload a module someone else owns).
    loaded_module: Option<String>,
}

impl Drop for PulseVoiceCapture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        // Killing parec closes the stdout pipe, so the reader's blocking read
        // returns and the thread can exit.
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        if let Some(idx) = &self.loaded_module {
            unload_module(idx);
        }
    }
}

/// Start echo-cancelling capture into `buffer` (native f32). Returns the handle
/// plus the `(sample_rate, channels)` the buffer is filled at.
pub fn open_voice_capture(
    buffer: Arc<Mutex<Vec<f32>>>,
    max_samples: usize,
) -> Result<(PulseVoiceCapture, u32, u16)> {
    ensure_tool("pactl")?;
    ensure_tool("parec")?;

    let loaded_module = ensure_echo_cancel_source().context("no echo-cancel source available")?;

    // From here on, any early return must unload a module we just loaded.
    let started = start_parec(&buffer, max_samples, loaded_module.clone());
    let capture = match started {
        Ok(capture) => capture,
        Err(e) => {
            if let Some(idx) = &loaded_module {
                unload_module(idx);
            }
            return Err(e);
        }
    };

    // Confirm audio actually flows before claiming success, so a failed parec
    // (e.g. the source vanished) falls back to the raw mic instead of looking
    // like a dead microphone. Dropping `capture` tears everything back down.
    if !wait_for_first_samples(&buffer) {
        drop(capture);
        bail!("echo-cancel source '{SOURCE_NAME}' produced no audio");
    }

    tracing::debug!("Voice capture reading pulse source '{SOURCE_NAME}' ({RATE}Hz, {CHANNELS}ch)");
    Ok((capture, RATE, CHANNELS))
}

/// Spawn `parec` on the echo-cancel source and the thread draining it.
fn start_parec(
    buffer: &Arc<Mutex<Vec<f32>>>,
    max_samples: usize,
    loaded_module: Option<String>,
) -> Result<PulseVoiceCapture> {
    let mut child = Command::new("parec")
        .arg("--raw")
        .arg("--format=float32le")
        .arg(format!("--rate={RATE}"))
        .arg(format!("--channels={CHANNELS}"))
        .arg(format!("--device={SOURCE_NAME}"))
        .arg("--client-name=murmur")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("failed to spawn parec")?;

    let stdout = child.stdout.take().context("parec produced no stdout")?;
    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = Arc::clone(&stop);
    let buf_thread = Arc::clone(buffer);

    let thread = std::thread::Builder::new()
        .name("pulse-voice-capture".into())
        .spawn(move || read_loop(stdout, &buf_thread, max_samples, &stop_thread))
        .context("failed to spawn pulse capture thread")?;

    Ok(PulseVoiceCapture {
        stop,
        thread: Some(thread),
        child: Some(child),
        loaded_module,
    })
}

/// Decode `parec`'s little-endian f32 stream into `buffer` until stopped or the
/// pipe closes. Reads may split a 4-byte sample, so carry the remainder.
fn read_loop(
    mut stdout: ChildStdout,
    buffer: &Arc<Mutex<Vec<f32>>>,
    max_samples: usize,
    stop: &AtomicBool,
) {
    let mut raw = [0u8; 8192];
    let mut carry: Vec<u8> = Vec::with_capacity(4);
    while !stop.load(Ordering::Acquire) {
        let n = match stdout.read(&mut raw) {
            Ok(0) => break, // parec exited or the pipe closed
            Ok(n) => n,
            Err(_) => break,
        };
        carry.extend_from_slice(&raw[..n]);
        let whole = carry.len() - carry.len() % 4;
        if let Ok(mut buf) = buffer.lock()
            && buf.len() < max_samples
        {
            for s in carry[..whole].chunks_exact(4) {
                let v = f32::from_le_bytes([s[0], s[1], s[2], s[3]]);
                buf.push(if v.is_finite() { v } else { 0.0 });
            }
        }
        carry.drain(..whole);
    }
}

/// Block until the buffer receives its first samples, or the timeout elapses.
fn wait_for_first_samples(buffer: &Arc<Mutex<Vec<f32>>>) -> bool {
    let deadline = Instant::now() + READY_TIMEOUT;
    while Instant::now() < deadline {
        if buffer.lock().map(|b| !b.is_empty()).unwrap_or(false) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

/// Ensure our echo-cancel source exists. Returns the module index we loaded (to
/// unload later), or `None` if a source by our name already existed.
fn ensure_echo_cancel_source() -> Result<Option<String>> {
    if echo_cancel_source_exists()? {
        return Ok(None);
    }
    Ok(Some(load_echo_cancel_module()?))
}

/// Whether a source named [`SOURCE_NAME`] is already registered.
fn echo_cancel_source_exists() -> Result<bool> {
    let out = Command::new("pactl")
        .args(["list", "short", "sources"])
        .output()
        .context("pactl list sources failed")?;
    if !out.status.success() {
        bail!("pactl list sources exited with {}", out.status);
    }
    let text = String::from_utf8_lossy(&out.stdout);
    // Columns are tab-separated: index, name, driver, ...
    Ok(text
        .lines()
        .any(|l| l.split('\t').nth(1) == Some(SOURCE_NAME)))
}

/// Load `module-echo-cancel` (WebRTC AEC) against the default sink/source and
/// return the new module's index. `use_master_format` keeps rates aligned.
fn load_echo_cancel_module() -> Result<String> {
    let out = Command::new("pactl")
        .arg("load-module")
        .arg("module-echo-cancel")
        .arg("aec_method=webrtc")
        .arg(format!("source_name={SOURCE_NAME}"))
        .arg("use_master_format=1")
        .output()
        .context("pactl load-module failed")?;
    if !out.status.success() {
        bail!(
            "module-echo-cancel load failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let idx = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if idx.is_empty() {
        bail!("module-echo-cancel loaded but returned no index");
    }
    Ok(idx)
}

/// Unload a module by index, best-effort.
fn unload_module(index: &str) {
    let _ = Command::new("pactl")
        .args(["unload-module", index])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Whether a CLI tool is callable (its `--version` exits 0).
fn ensure_tool(name: &str) -> Result<()> {
    let ok = Command::new(name)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if ok {
        Ok(())
    } else {
        bail!("{name} not available")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Manual smoke test: needs a Linux box with PulseAudio/PipeWire plus
    /// `pactl`/`parec`, so it's ignored in CI. Run locally with:
    ///   cargo test -p murmur-core pulse_smoke -- --ignored --nocapture
    #[test]
    #[ignore]
    fn pulse_smoke_captures_audio() {
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
