//! The roaming live-preview caption window that appears near the active
//! window when caption_position is "window". It is driven entirely from the
//! backend, never takes focus, and is click-through, so it only displays text.

use tauri::{Emitter, Manager};

#[cfg(windows)]
use tauri::PhysicalPosition;

#[cfg(windows)]
const CAPTION_WIDTH: i32 = 380;
#[cfg(windows)]
const CAPTION_HEIGHT: i32 = 64;

/// Show the caption near `target_hwnd` with the given interim text.
pub(crate) fn show(app: &tauri::AppHandle, target_hwnd: usize, text: &str) {
    let Some(win) = app.get_webview_window("caption") else {
        return;
    };

    #[cfg(windows)]
    {
        let Some((x, y)) = anchor_near_window(target_hwnd) else {
            // The target window is gone or unreportable: hide rather than leave
            // the caption stranded at a stale or default (0,0) position.
            let _ = win.emit("caption-text", "");
            let _ = win.hide();
            return;
        };
        tracing::debug!("Caption near hwnd=0x{:x} at ({}, {})", target_hwnd, x, y);
        let _ = win.set_position(PhysicalPosition::new(x, y));
    }

    #[cfg(not(windows))]
    let _ = target_hwnd;

    let _ = win.emit("caption-text", text);
    let _ = win.set_always_on_top(true);
    let _ = win.show();
}

/// Hide the caption and clear its text.
pub(crate) fn hide(app: &tauri::AppHandle) {
    if let Some(win) = app.get_webview_window("caption") {
        let _ = win.emit("caption-text", "");
        let _ = win.hide();
    }
}

/// Place the caption just below the target window when there is room on its
/// monitor, otherwise just inside its bottom edge (maximized windows). Centered
/// horizontally and clamped to the monitor work area.
#[cfg(windows)]
fn anchor_near_window(hwnd: usize) -> Option<(i32, i32)> {
    if hwnd == 0 {
        return None;
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Rect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }
    #[repr(C)]
    struct MonitorInfo {
        cb_size: u32,
        rc_monitor: Rect,
        rc_work: Rect,
        flags: u32,
    }
    const MONITOR_DEFAULTTONEAREST: u32 = 2;

    unsafe extern "system" {
        fn IsWindow(hwnd: usize) -> i32;
        fn GetWindowRect(hwnd: usize, rect: *mut Rect) -> i32;
        fn MonitorFromWindow(hwnd: usize, flags: u32) -> usize;
        fn GetMonitorInfoW(hmon: usize, info: *mut MonitorInfo) -> i32;
        fn GetDpiForWindow(hwnd: usize) -> u32;
    }

    if unsafe { IsWindow(hwnd) } == 0 {
        return None;
    }
    let mut r = Rect {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    if unsafe { GetWindowRect(hwnd, &mut r) } == 0 {
        return None;
    }

    let work = {
        let hmon = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
        let mut info = MonitorInfo {
            cb_size: std::mem::size_of::<MonitorInfo>() as u32,
            rc_monitor: r,
            rc_work: r,
            flags: 0,
        };
        if hmon != 0 && unsafe { GetMonitorInfoW(hmon, &mut info) } != 0 {
            info.rc_work
        } else {
            r
        }
    };

    // GetWindowRect and set_position are in physical pixels, but the caption
    // window is sized in logical pixels. Scale its dimensions by the target
    // monitor's DPI so centering and bottom-edge fitting are right on HiDPI.
    let scale = match unsafe { GetDpiForWindow(hwnd) } {
        0 => 1.0,
        dpi => dpi as f32 / 96.0,
    };
    let caption_w = (CAPTION_WIDTH as f32 * scale).round() as i32;
    let caption_h = (CAPTION_HEIGHT as f32 * scale).round() as i32;
    let gap = (8.0 * scale).round() as i32;
    let inset = (12.0 * scale).round() as i32;

    let win_w = (r.right - r.left).max(0);
    let mut x = r.left + (win_w - caption_w) / 2;
    x = x.clamp(work.left, (work.right - caption_w).max(work.left));

    let below = r.bottom + gap;
    let y = if below + caption_h <= work.bottom {
        below
    } else {
        (r.bottom - caption_h - inset).max(work.top)
    };

    Some((x, y))
}
