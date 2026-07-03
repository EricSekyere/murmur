//! Echo-cancelled capture on Linux via the PulseAudio protocol.
//!
//! Works against both PulseAudio and PipeWire (pipewire-pulse implements
//! `module-echo-cancel` natively): `pactl` loads the module, which publishes an
//! echo-cancelled virtual source, and a `parec` child streams that source as
//! raw float32 into the shared capture buffer. Subprocesses instead of a
//! libpulse binding keep the dependency tree pure Rust; both tools ship in
//! `pulseaudio-utils`, present on effectively every desktop install.
//!
//! Scope: the AEC reference is the module's virtual sink (`pactl` exposes no
//! way to enable PipeWire's `monitor.mode`), so only audio played through that
//! sink is cancelled. Users who want system-wide cancellation during meetings
//! select "Murmur Echo Cancel" as the output device in sound settings. The
//! module stays loaded for the server session once created: reloading it every
//! push-to-talk would rebuild the audio graph (audible clicks), and an existing
//! source is detected and reused across app runs.

use anyhow::{Context, Result, bail};
use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Name of the echo-cancelled source the module publishes.
pub const SOURCE_NAME: &str = "murmur_echo_cancel";
/// Name of the module's playback sink (the cancellation reference).
const SINK_NAME: &str = "murmur_echo_cancel_sink";

/// Fixed capture format requested from `parec`; the server resamples for us.
pub const RATE: u32 = 48_000;
pub const CHANNELS: u16 = 1;

/// A live echo-cancelled capture session: a `parec` child plus the reader
/// thread draining its stdout into the shared buffer. Dropping it ends the
/// session; the echo-cancel module itself stays loaded (see module docs).
pub struct PulseAecCapture {
    child: Child,
    stop: Arc<AtomicBool>,
    reader: Option<std::thread::JoinHandle<()>>,
}

impl PulseAecCapture {
    /// Whether the `parec` child is still running. `parec` exits immediately
    /// when the source or server is unusable, so this is the health probe for
    /// the first session of a run (a silent-but-open stream is legitimate: the
    /// mic may simply be muted).
    pub fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

impl Drop for PulseAecCapture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // Kill unblocks the reader's stdout read with EOF.
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(handle) = self.reader.take() {
            let _ = handle.join();
        }
    }
}

/// Start an echo-cancelled capture session, appending f32 samples (RATE Hz,
/// CHANNELS ch) to `buffer` until dropped. `max_samples` caps the buffer so a
/// stalled consumer can never grow it without bound.
///
/// Errors when `pactl`/`parec` are missing or the server refuses the module —
/// the caller falls back to the raw microphone.
pub fn open(buffer: Arc<Mutex<Vec<f32>>>, max_samples: usize) -> Result<PulseAecCapture> {
    ensure_module()?;

    let mut child = Command::new("parec")
        .args([
            "-d",
            SOURCE_NAME,
            "--format=float32le",
            &format!("--rate={RATE}"),
            &format!("--channels={CHANNELS}"),
            "--latency-msec=20",
            "--client-name=murmur",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("parec not available (install pulseaudio-utils)")?;

    let mut stdout = child
        .stdout
        .take()
        .context("parec spawned without a stdout pipe")?;

    let stop = Arc::new(AtomicBool::new(false));
    let stop_reader = Arc::clone(&stop);
    let reader = std::thread::Builder::new()
        .name("pulse-aec-reader".into())
        .spawn(move || {
            let mut chunk = [0u8; 16 * 1024];
            // Bytes of a sample split across read boundaries.
            let mut carry = [0u8; 4];
            let mut carry_len = 0usize;
            let mut scratch: Vec<f32> = Vec::with_capacity(chunk.len() / 4 + 1);

            while !stop_reader.load(Ordering::Relaxed) {
                let n = match stdout.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };

                scratch.clear();
                decode_f32le(&chunk[..n], &mut carry, &mut carry_len, &mut scratch);

                if let Ok(mut buf) = buffer.lock() {
                    let room = max_samples.saturating_sub(buf.len());
                    let take = scratch.len().min(room);
                    buf.extend_from_slice(&scratch[..take]);
                }
            }
        })
        .context("Failed to spawn the pulse AEC reader thread")?;

    Ok(PulseAecCapture {
        child,
        stop,
        reader: Some(reader),
    })
}

/// Decode little-endian f32 PCM from a raw byte stream into `out`, carrying a
/// sample split across read boundaries in `carry`/`carry_len`.
fn decode_f32le(mut data: &[u8], carry: &mut [u8; 4], carry_len: &mut usize, out: &mut Vec<f32>) {
    if *carry_len > 0 {
        let take = (4 - *carry_len).min(data.len());
        carry[*carry_len..*carry_len + take].copy_from_slice(&data[..take]);
        *carry_len += take;
        data = &data[take..];
        if *carry_len == 4 {
            out.push(f32::from_le_bytes(*carry));
            *carry_len = 0;
        }
    }
    // The read may have ended inside the carry; the tail write below would
    // otherwise zero `carry_len` and drop those bytes.
    if data.is_empty() {
        return;
    }
    for sample in data.chunks_exact(4) {
        // chunks_exact(4) guarantees the length; unwrap-free convert.
        let mut le = [0u8; 4];
        le.copy_from_slice(sample);
        out.push(f32::from_le_bytes(le));
    }
    let tail = data.chunks_exact(4).remainder();
    carry[..tail.len()].copy_from_slice(tail);
    *carry_len = tail.len();
}

/// Whether `pactl list short sources` output already lists our source.
fn source_exists(list_output: &str) -> bool {
    list_output
        .lines()
        .any(|line| line.split_whitespace().nth(1) == Some(SOURCE_NAME))
}

/// Make sure the echo-cancel source exists, loading `module-echo-cancel` if
/// this is the first Murmur session on this audio server.
fn ensure_module() -> Result<()> {
    if source_exists(&run_pactl(&["list", "short", "sources"])?) {
        return Ok(());
    }

    let source_arg = format!("source_name={SOURCE_NAME}");
    let sink_arg = format!("sink_name={SINK_NAME}");
    let props_arg = "source_properties=device.description=MurmurEchoCancel";

    // WebRTC is the best AEC engine both servers offer, but it is a compile
    // option; retry with the server's default engine before giving up.
    let attempts: [&[&str]; 2] = [
        &[
            "load-module",
            "module-echo-cancel",
            "aec_method=webrtc",
            &source_arg,
            &sink_arg,
            props_arg,
        ],
        &[
            "load-module",
            "module-echo-cancel",
            &source_arg,
            &sink_arg,
            props_arg,
        ],
    ];

    let mut last_err = None;
    for args in attempts {
        match run_pactl(args) {
            Ok(_) => {
                tracing::info!("Loaded PulseAudio/PipeWire module-echo-cancel");
                return Ok(());
            }
            Err(e) => last_err = Some(e),
        }
    }
    // last_err is always Some here: both attempts failed.
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("module-echo-cancel load failed")))
}

fn run_pactl(args: &[&str]) -> Result<String> {
    let out = Command::new("pactl")
        .args(args)
        .output()
        .context("pactl not available (install pulseaudio-utils)")?;
    if !out.status.success() {
        bail!(
            "pactl {} failed: {}",
            args.first().copied().unwrap_or_default(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes_of(samples: &[f32]) -> Vec<u8> {
        samples.iter().flat_map(|s| s.to_le_bytes()).collect()
    }

    #[test]
    fn decode_whole_chunk() {
        let samples = [0.0f32, 0.5, -1.0, 3.25];
        let (mut carry, mut carry_len, mut out) = ([0u8; 4], 0usize, Vec::new());
        decode_f32le(&bytes_of(&samples), &mut carry, &mut carry_len, &mut out);
        assert_eq!(out, samples);
        assert_eq!(carry_len, 0);
    }

    #[test]
    fn decode_carries_split_sample_across_reads() {
        let samples = [1.5f32, -0.25, 42.0];
        let bytes = bytes_of(&samples);
        // Split mid-sample at every possible offset, in two reads.
        for split in 1..bytes.len() {
            let (mut carry, mut carry_len, mut out) = ([0u8; 4], 0usize, Vec::new());
            decode_f32le(&bytes[..split], &mut carry, &mut carry_len, &mut out);
            decode_f32le(&bytes[split..], &mut carry, &mut carry_len, &mut out);
            assert_eq!(out, samples, "split at byte {split}");
            assert_eq!(carry_len, 0, "split at byte {split}");
        }
    }

    #[test]
    fn decode_single_byte_reads() {
        let samples = [-7.125f32, 0.001];
        let bytes = bytes_of(&samples);
        let (mut carry, mut carry_len, mut out) = ([0u8; 4], 0usize, Vec::new());
        for b in &bytes {
            decode_f32le(
                std::slice::from_ref(b),
                &mut carry,
                &mut carry_len,
                &mut out,
            );
        }
        assert_eq!(out, samples);
    }

    #[test]
    fn source_exists_matches_exact_name_only() {
        let listing = format!(
            "1\talsa_input.pci-0000_00_1f.3\tmodule-alsa-card.c\ts16le 2ch 48000Hz\tRUNNING\n\
             2\t{SOURCE_NAME}\tmodule-echo-cancel.c\tfloat32le 1ch 48000Hz\tIDLE\n"
        );
        assert!(source_exists(&listing));
        assert!(!source_exists(
            "1\talsa_input.usb\tmodule-alsa-card.c\ts16le 2ch 48000Hz\tIDLE\n"
        ));
        // A name that merely contains ours must not match.
        assert!(!source_exists(
            "3\tmurmur_echo_cancel_sink.monitor\tmodule-echo-cancel.c\tfloat32le 1ch 48000Hz\tIDLE\n"
        ));
    }
}
