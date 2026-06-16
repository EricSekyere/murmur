//! Delivering text to the window the user is dictating into.
//!
//! Murmur's windows are non-activating, so the target app normally keeps
//! focus for the whole record→transcribe cycle. Focus is only restored when
//! Murmur itself ended up in front (e.g. the user clicked the stop button).

use murmur_core::output::OutputMode;

#[cfg(windows)]
use crate::state::AppState;

/// Output transcribed text according to the configured output mode,
/// restoring focus to the user's target window first when necessary.
pub(crate) fn output_text(
    text: &str,
    mode: OutputMode,
    #[cfg(windows)] previous_hwnd: usize,
    #[cfg(windows)] last_external_hwnd: usize,
) -> anyhow::Result<()> {
    #[cfg(windows)]
    {
        let needs_focused_target = !matches!(mode, OutputMode::Clipboard | OutputMode::Stdout);
        if needs_focused_target && !ensure_external_target(previous_hwnd, last_external_hwnd) {
            murmur_core::output::clipboard::ClipboardOutput::new()?.copy(text.trim())?;
            anyhow::bail!(
                "No target window available (Murmur is in front and no external \
                 window is tracked); copied transcription to clipboard"
            );
        }
    }

    murmur_core::output::dispatch_output(text, mode)
}

/// Make sure the window the user started dictating into receives the text.
///
/// Delivering to whatever happens to be focused at the end of a phrase is a
/// hazard: if focus drifted to another window mid-recording (a notification,
/// a clicked field, a password box) the text would land there. So prefer the
/// recording-start target, only falling back to the live-tracked window or the
/// current foreground when that target is gone.
#[cfg(windows)]
fn ensure_external_target(previous_hwnd: usize, last_external_hwnd: usize) -> bool {
    let current_fg = foreground_window();
    let current_is_external = current_fg != 0 && !is_own_window(current_fg);

    // Already focused on the exact window dictation started in: deliver there
    // without disturbing focus (the common single-window case).
    if current_is_external && current_fg == previous_hwnd {
        return true;
    }

    // Focus drifted or Murmur is in front. Restore the start target first, then
    // the last external window, before accepting the current foreground.
    if [previous_hwnd, last_external_hwnd]
        .iter()
        .any(|&h| h != 0 && !is_own_window(h) && restore_foreground_window(h))
    {
        return true;
    }
    current_is_external
}

/// Restore focus to the window the user was working in before recording.
///
/// Tries a plain SetForegroundWindow first; if Windows refuses (foreground
/// lock), retries with AttachThreadInput, which shares input state with the
/// current foreground thread and lifts the restriction.
#[cfg(windows)]
fn restore_foreground_window(hwnd: usize) -> bool {
    unsafe extern "system" {
        fn AttachThreadInput(id_attach: u32, id_attach_to: u32, f_attach: i32) -> i32;
        fn BringWindowToTop(hwnd: usize) -> i32;
        fn GetCurrentThreadId() -> u32;
        fn SetForegroundWindow(hwnd: usize) -> i32;
        fn GetForegroundWindow() -> usize;
        fn GetWindowThreadProcessId(hwnd: usize, lpdw_process_id: *mut u32) -> u32;
        fn IsWindow(hwnd: usize) -> i32;
        fn IsWindowVisible(hwnd: usize) -> i32;
        fn ShowWindow(hwnd: usize, n_cmd_show: i32) -> i32;
    }
    const SW_RESTORE: i32 = 9;

    if hwnd == 0 || unsafe { IsWindow(hwnd) } == 0 || unsafe { IsWindowVisible(hwnd) } == 0 {
        tracing::warn!(
            "Saved output target is not a visible window: hwnd=0x{:x}",
            hwnd
        );
        return false;
    }
    if unsafe { GetForegroundWindow() } == hwnd {
        return true;
    }

    unsafe {
        ShowWindow(hwnd, SW_RESTORE);
        BringWindowToTop(hwnd);
        SetForegroundWindow(hwnd);
    }
    std::thread::sleep(std::time::Duration::from_millis(75));
    if unsafe { GetForegroundWindow() } == hwnd {
        return true;
    }

    let current_thread = unsafe { GetCurrentThreadId() };
    let mut pid = 0u32;
    let foreground_thread = unsafe { GetWindowThreadProcessId(GetForegroundWindow(), &mut pid) };
    let target_thread = unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };

    unsafe {
        if foreground_thread != 0 && foreground_thread != current_thread {
            AttachThreadInput(current_thread, foreground_thread, 1);
        }
        if target_thread != 0 && target_thread != current_thread {
            AttachThreadInput(current_thread, target_thread, 1);
        }

        ShowWindow(hwnd, SW_RESTORE);
        BringWindowToTop(hwnd);
        SetForegroundWindow(hwnd);

        if target_thread != 0 && target_thread != current_thread {
            AttachThreadInput(current_thread, target_thread, 0);
        }
        if foreground_thread != 0 && foreground_thread != current_thread {
            AttachThreadInput(current_thread, foreground_thread, 0);
        }
    }

    std::thread::sleep(std::time::Duration::from_millis(75));
    let restored = unsafe { GetForegroundWindow() } == hwnd;
    if !restored {
        tracing::warn!(
            "Failed to restore target window; refusing to inject text (target=0x{:x})",
            hwnd
        );
    }
    restored
}

#[cfg(windows)]
pub(crate) fn foreground_window() -> usize {
    unsafe extern "system" {
        fn GetForegroundWindow() -> usize;
    }
    unsafe { GetForegroundWindow() }
}

#[cfg(windows)]
fn window_process_id(hwnd: usize) -> Option<u32> {
    if hwnd == 0 {
        return None;
    }
    unsafe extern "system" {
        fn GetWindowThreadProcessId(hwnd: usize, lpdw_process_id: *mut u32) -> u32;
    }
    let mut process_id = 0u32;
    unsafe {
        GetWindowThreadProcessId(hwnd, &mut process_id);
    }
    (process_id != 0).then_some(process_id)
}

#[cfg(windows)]
pub(crate) fn is_own_window(hwnd: usize) -> bool {
    window_process_id(hwnd) == Some(std::process::id())
}

/// Remember the window text should be delivered to for this session.
#[cfg(windows)]
pub(crate) fn save_output_target_window(state: &AppState) {
    let foreground = foreground_window();
    let target = if foreground != 0 && !is_own_window(foreground) {
        *state
            .last_external_foreground
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = foreground;
        foreground
    } else {
        *state
            .last_external_foreground
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    };

    *state
        .previous_foreground
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = target;
    tracing::info!(
        "Saved output target window: foreground=0x{:x}, target=0x{:x}",
        foreground,
        target,
    );
}

/// Continuously track the last non-Murmur foreground window so output can
/// fall back to it even if the start-of-session snapshot goes stale.
#[cfg(windows)]
pub(crate) fn spawn_foreground_tracker(app: tauri::AppHandle) {
    use tauri::Manager;

    std::thread::spawn(move || {
        loop {
            let hwnd = foreground_window();
            if hwnd != 0
                && !is_own_window(hwnd)
                && let Some(state) = app.try_state::<AppState>()
            {
                *state
                    .last_external_foreground
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()) = hwnd;
            }
            std::thread::sleep(std::time::Duration::from_millis(150));
        }
    });
}
