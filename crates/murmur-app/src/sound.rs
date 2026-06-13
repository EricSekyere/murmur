//! Short audio cues for recording start/stop.
//!
//! Uses the Win32 `Beep` API (routed through the default audio device on
//! modern Windows), played on a detached thread so it never blocks the
//! recording path. No-op on other platforms.

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

/// Spawn a thread that plays one tone (frequency_hz, duration_ms).
#[cfg(windows)]
fn play_tone(freq: u32, duration_ms: u32) {
    unsafe extern "system" {
        fn Beep(dw_freq: u32, dw_duration: u32) -> i32;
    }
    std::thread::spawn(move || unsafe {
        Beep(freq, duration_ms);
    });
}
