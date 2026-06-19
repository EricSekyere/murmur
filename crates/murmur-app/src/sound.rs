//! Short audio cues for recording start/stop.
//!
//! Uses the Win32 `Beep` API (routed through the default audio device on
//! modern Windows). Cues are queued to one long-lived worker thread so rapid
//! start/stop never piles up detached threads. No-op on other platforms.

/// Play a single high tone when recording starts.
pub(crate) fn play_start() {
    #[cfg(windows)]
    play_tone(880, 110);
}

/// Play a single low tone when recording stops.
pub(crate) fn play_stop() {
    #[cfg(windows)]
    play_tone(520, 110);
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
