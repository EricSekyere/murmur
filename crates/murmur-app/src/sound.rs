//! Short audio cues for recording start and stop.
//!
//! On Windows this uses the Win32 `Beep` API (routed through the default audio
//! device on modern Windows), with cues queued to one long-lived worker thread
//! so rapid start/stop never piles up threads. On macOS it plays a built-in
//! system sound via `afplay` on a detached thread. No-op on other platforms.

/// Play a cue when recording starts.
pub(crate) fn play_start() {
    #[cfg(windows)]
    play_tone(880, 110);
    #[cfg(target_os = "macos")]
    play_system_sound("/System/Library/Sounds/Tink.aiff");
}

/// Play a cue when recording stops.
pub(crate) fn play_stop() {
    #[cfg(windows)]
    play_tone(520, 110);
    #[cfg(target_os = "macos")]
    play_system_sound("/System/Library/Sounds/Bottle.aiff");
}

/// Queue one tone (frequency_hz, duration_ms) to the single cue thread.
#[cfg(windows)]
fn play_tone(freq: u32, duration_ms: u32) {
    use std::sync::OnceLock;
    use std::sync::mpsc::{Sender, channel};

    static CUES: OnceLock<Sender<(u32, u32)>> = OnceLock::new();
    let tx = CUES.get_or_init(|| {
        let (tx, rx) = channel::<(u32, u32)>();
        std::thread::spawn(move || {
            unsafe extern "system" {
                fn Beep(dw_freq: u32, dw_duration: u32) -> i32;
            }
            while let Ok((freq, dur)) = rx.recv() {
                unsafe {
                    Beep(freq, dur);
                }
            }
        });
        tx
    });
    // Sender lives for the process, so this only fails if the worker panicked.
    let _ = tx.send((freq, duration_ms));
}

/// Spawn `afplay` to play a system sound file without blocking.
#[cfg(target_os = "macos")]
fn play_system_sound(path: &'static str) {
    std::thread::spawn(move || {
        let _ = std::process::Command::new("/usr/bin/afplay")
            .arg(path)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    });
}
