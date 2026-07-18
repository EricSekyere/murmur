//! Live probe for Windows system-audio (loopback) capture.
//!
//! Starts a [`LoopbackCapture`], records for N seconds (first CLI arg, default
//! 3), then prints the negotiated native config, total 16 kHz samples, their
//! peak/RMS, and a callback timeline so you can see whether WASAPI fires
//! callbacks while nothing is playing (the event-starvation quirk).
//!
//! Run with something playing to see real audio; run silent to observe the
//! zero-callback behaviour:
//!   cargo run -p murmur-core --example loopback_probe -- 3
//!
//! `println!` is fine here — this is an example binary, not library code.

use murmur_core::audio::loopback::LoopbackCapture;
use std::time::{Duration, Instant};

fn main() -> anyhow::Result<()> {
    let secs: u64 = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .filter(|&n| n > 0)
        .unwrap_or(3);

    println!("Starting loopback capture for {secs}s (play audio to see signal)...");

    let mut cap = LoopbackCapture::new();
    cap.start()?;
    println!(
        "Native config: {} Hz, {} channel(s)",
        cap.native_rate(),
        cap.native_channels()
    );

    // Sample the callback counter each 250 ms so a reader can see whether
    // callbacks arrive continuously, only while audio plays, or not at all.
    let start = Instant::now();
    let deadline = start + Duration::from_secs(secs);
    let mut last = 0usize;
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(250));
        let now = cap.callback_count();
        let delta = now - last;
        last = now;
        println!(
            "  t={:>5.2}s  callbacks +{delta:<4} (total {now})",
            start.elapsed().as_secs_f32()
        );
    }

    let total_callbacks = cap.callback_count();
    let out = cap.stop()?;

    let n = out.samples.len();
    let rms = if n > 0 {
        (out.samples.iter().map(|s| s * s).sum::<f32>() / n as f32).sqrt()
    } else {
        0.0
    };
    let peak = out.samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));

    println!("---");
    println!("Total callbacks : {total_callbacks}");
    println!("16 kHz samples  : {n} ({:.2}s mono)", out.duration_secs());
    println!("Peak amplitude  : {peak:.6}");
    println!("RMS             : {rms:.6}");
    if total_callbacks == 0 {
        println!(
            "Observation     : zero callbacks — WASAPI loopback delivered nothing while the \
             render engine was idle (event starvation). Play audio and re-run to see signal."
        );
    } else if peak < 1e-5 {
        println!(
            "Observation     : callbacks fired but samples were silent (render active, output \
             muted or truly silent content)."
        );
    } else {
        println!("Observation     : captured live system audio.");
    }

    Ok(())
}
