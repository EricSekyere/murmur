//! The roaming live-preview caption window that appears near the active window
//! when caption_position is "window". It is driven entirely from the backend,
//! never takes focus, and is click-through, so it only displays text.
//!
//! When we can locate the focused text field (via UI Automation, captured once
//! at session start), the caption anchors just below it, so dictation into a
//! browser address bar shows the caption by the bar rather than at the window's
//! bottom edge. Otherwise it falls back to anchoring below the window.

use tauri::{Emitter, Manager};

#[cfg(windows)]
use tauri::PhysicalPosition;

#[cfg(windows)]
const CAPTION_WIDTH: i32 = 380;
#[cfg(windows)]
const CAPTION_HEIGHT: i32 = 64;

/// Where to anchor a session's caption: the target window, plus the focused
/// input's screen rect captured at session start (when we could find one).
///
/// Both fields are read only by the Windows anchoring path; on other platforms
/// the caption is not yet anchored to the focused input, so allow them to sit
/// unused there rather than diverge the struct per platform.
#[derive(Clone, Copy)]
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) struct CaptionAnchor {
    pub hwnd: usize,
    /// Focused-input rect (left, top, right, bottom) in physical screen pixels.
    pub focus: Option<(i32, i32, i32, i32)>,
}

#[cfg(windows)]
#[repr(C)]
#[derive(Clone, Copy)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[cfg(windows)]
#[repr(C)]
struct MonitorInfo {
    cb_size: u32,
    rc_monitor: Rect,
    rc_work: Rect,
    flags: u32,
}

#[cfg(windows)]
const MONITOR_DEFAULTTONEAREST: u32 = 2;

#[cfg(windows)]
unsafe extern "system" {
    fn IsWindow(hwnd: usize) -> i32;
    fn GetWindowRect(hwnd: usize, rect: *mut Rect) -> i32;
    fn MonitorFromWindow(hwnd: usize, flags: u32) -> usize;
    fn GetMonitorInfoW(hmon: usize, info: *mut MonitorInfo) -> i32;
    fn GetDpiForWindow(hwnd: usize) -> u32;
}

pub(crate) fn show(app: &tauri::AppHandle, anchor: &CaptionAnchor, text: &str) {
    let Some(win) = app.get_webview_window("caption") else {
        return;
    };

    #[cfg(windows)]
    {
        let Some((x, y)) = anchor_caption(anchor) else {
            // The target window is gone or unreportable: hide rather than leave
            // the caption stranded at a stale or default (0,0) position.
            let _ = win.emit("caption-text", "");
            let _ = win.hide();
            return;
        };
        let _ = win.set_position(PhysicalPosition::new(x, y));
    }

    #[cfg(not(windows))]
    let _ = anchor;

    let _ = win.emit("caption-text", text);
    let _ = win.set_always_on_top(true);
    let _ = win.show();
}

pub(crate) fn hide(app: &tauri::AppHandle) {
    if let Some(win) = app.get_webview_window("caption") {
        let _ = win.emit("caption-text", "");
        let _ = win.hide();
    }
}

/// Resolve the on-screen position: anchor below the focused input if we
/// captured one and the window is still valid, otherwise below the window.
#[cfg(windows)]
fn anchor_caption(anchor: &CaptionAnchor) -> Option<(i32, i32)> {
    if anchor.hwnd == 0 || unsafe { IsWindow(anchor.hwnd) } == 0 {
        return None;
    }
    let rect = match anchor.focus {
        Some((left, top, right, bottom)) => Rect {
            left,
            top,
            right,
            bottom,
        },
        None => {
            let mut wr = Rect {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            };
            if unsafe { GetWindowRect(anchor.hwnd, &mut wr) } == 0 {
                return None;
            }
            wr
        }
    };
    Some(anchor_below(anchor.hwnd, rect))
}

/// Place the caption centered under `r`, on `hwnd`'s monitor, clamped to the
/// work area; if there's no room below, just inside `r`'s bottom edge.
#[cfg(windows)]
fn anchor_below(hwnd: usize, r: Rect) -> (i32, i32) {
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

    // GetWindowRect, UIA rects, and set_position are physical pixels, but the
    // caption window is sized in logical pixels. Scale its dimensions by the
    // monitor DPI so centering and bottom-edge fitting are right on HiDPI.
    let scale = match unsafe { GetDpiForWindow(hwnd) } {
        0 => 1.0,
        dpi => dpi as f32 / 96.0,
    };
    let caption_w = (CAPTION_WIDTH as f32 * scale).round() as i32;
    let caption_h = (CAPTION_HEIGHT as f32 * scale).round() as i32;
    let gap = (8.0 * scale).round() as i32;
    let inset = (12.0 * scale).round() as i32;

    let span = (r.right - r.left).max(0);
    let mut x = r.left + (span - caption_w) / 2;
    x = x.clamp(work.left, (work.right - caption_w).max(work.left));

    let below = r.bottom + gap;
    let y = if below + caption_h <= work.bottom {
        below
    } else {
        (r.bottom - caption_h - inset).max(work.top)
    };
    (x, y)
}

/// Screen rect of the focused input via UI Automation, captured once at session
/// start. Returns `None` for apps without accessibility info, when the focused
/// element isn't inside the target window (e.g. Murmur's own UI took focus), or
/// on any failure, so callers fall back to anchoring below the window.
#[cfg(windows)]
pub(crate) fn focused_input_rect(target_hwnd: usize) -> Option<(i32, i32, i32, i32)> {
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
        CoUninitialize,
    };
    use windows::Win32::UI::Accessibility::{CUIAutomation, IUIAutomation};

    if target_hwnd == 0 {
        return None;
    }

    unsafe {
        // Balance CoUninitialize only when we actually initialized COM here
        // (S_OK / S_FALSE); a different-apartment error leaves COM as-is.
        let should_uninit = CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok();
        let rect = (|| {
            let automation: IUIAutomation =
                CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok()?;
            let element = automation.GetFocusedElement().ok()?;
            let r = element.CurrentBoundingRectangle().ok()?;
            if r.right <= r.left || r.bottom <= r.top {
                return None;
            }
            if !center_in_window(target_hwnd, r.left, r.top, r.right, r.bottom) {
                return None;
            }
            Some((r.left, r.top, r.right, r.bottom))
        })();
        if should_uninit {
            CoUninitialize();
        }
        rect
    }
}

/// Whether the rect's center sits inside the target window's bounds.
#[cfg(windows)]
fn center_in_window(hwnd: usize, left: i32, top: i32, right: i32, bottom: i32) -> bool {
    let mut wr = Rect {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    if unsafe { GetWindowRect(hwnd, &mut wr) } == 0 {
        return false;
    }
    let cx = (left + right) / 2;
    let cy = (top + bottom) / 2;
    cx >= wr.left && cx <= wr.right && cy >= wr.top && cy <= wr.bottom
}
