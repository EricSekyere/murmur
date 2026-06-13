//! Short audio cues for recording start and stop.
//!
//! On Windows this uses the Win32 `Beep` API (routed through the default
//! audio device on modern Windows). On macOS it plays a built-in system
//! sound via `afplay`. Both run on a detached thread so they never block
//! the recording path. No-op on other platforms.

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
