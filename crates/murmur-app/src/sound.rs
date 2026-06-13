//! Short audio cues for recording start/stop.
//!
//! Uses the Win32 `Beep` API (routed through the default audio device on
//! modern Windows), played on a detached thread so it never blocks the
//! recording path. No-op on other platforms.

/// Play a rising two-tone chirp when recording starts.
pub(crate) fn play_start() {
    #[cfg(windows)]
    play_tones(&[(660, 70), (988, 90)]);
}

/// Play a falling two-tone chirp when recording stops.
pub(crate) fn play_stop() {
    #[cfg(windows)]
    play_tones(&[(660, 70), (440, 90)]);
}

/// Spawn a thread that beeps each (frequency_hz, duration_ms) in sequence.
#[cfg(windows)]
fn play_tones(tones: &'static [(u32, u32)]) {
    unsafe extern "system" {
        fn Beep(dw_freq: u32, dw_duration: u32) -> i32;
    }
    std::thread::spawn(move || {
        for &(freq, dur) in tones {
            unsafe {
                Beep(freq, dur);
            }
        }
    });
}
