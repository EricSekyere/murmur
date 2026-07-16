//! Global input: hotkey events, double-tap toggle, and click-to-stop.

// Only the double-tap tracker (Windows/macOS) uses these.
#[cfg(any(windows, target_os = "macos"))]
use std::time::{Duration, Instant};

use tauri::Manager;
use tauri_plugin_global_shortcut::ShortcutState;

use crate::session::handle_toggle;
use crate::state::AppState;

/// Dispatch the configured global hotkey by activation mode: toggle (press
/// flips) or hold/push-to-talk (press starts, release stops).
pub(crate) fn handle_hotkey_event(app: &tauri::AppHandle, shortcut_state: ShortcutState) {
    if hotkey_hold_mode(app) {
        match shortcut_state {
            // Auto-repeat may resend Pressed; begin_recording is idempotent.
            ShortcutState::Pressed => {
                crate::session::begin_recording(app);
                show_widget_window(app);
            }
            ShortcutState::Released => crate::session::end_recording(app),
        }
    } else if shortcut_state == ShortcutState::Pressed {
        handle_toggle(app);
    }
}

/// Whether the configured hotkey should hold-to-talk. Falls back to toggle on
/// lock contention (try_lock keeps the shortcut handler non-blocking).
fn hotkey_hold_mode(app: &tauri::AppHandle) -> bool {
    app.try_state::<AppState>()
        .and_then(|state| {
            state
                .settings
                .try_lock()
                .ok()
                .map(|s| s.activation_mode == "hold")
        })
        .unwrap_or(false)
}

pub(crate) fn show_widget_window(app: &tauri::AppHandle) {
    if let Some(widget) = app.get_webview_window("widget") {
        let _ = widget.show();
    }
}

/// Which key participates in double-tap toggle detection.
#[cfg(any(windows, target_os = "macos"))]
#[derive(Clone, Copy, PartialEq, Eq)]
enum TapTarget {
    /// The platform modifier, either side (Ctrl on Windows, Cmd on macOS).
    Modifier,
    /// One specific key (e.g. right Ctrl, or a bare letter). Taps only count
    /// while no other modifier is held and no other key was pressed in
    /// between, so shortcuts (including Murmur's own Ctrl+V paste output)
    /// never register as taps.
    Key(rdev::Key),
}

#[cfg(any(windows, target_os = "macos"))]
fn is_platform_double_tap_modifier(key: rdev::Key) -> bool {
    #[cfg(windows)]
    {
        matches!(key, rdev::Key::ControlLeft | rdev::Key::ControlRight)
    }
    #[cfg(target_os = "macos")]
    {
        matches!(key, rdev::Key::MetaLeft | rdev::Key::MetaRight)
    }
}

#[cfg(any(windows, target_os = "macos"))]
fn letter_to_rdev_key(letter: char) -> Option<rdev::Key> {
    use rdev::Key::*;
    Some(match letter {
        'a' => KeyA,
        'b' => KeyB,
        'c' => KeyC,
        'd' => KeyD,
        'e' => KeyE,
        'f' => KeyF,
        'g' => KeyG,
        'h' => KeyH,
        'i' => KeyI,
        'j' => KeyJ,
        'k' => KeyK,
        'l' => KeyL,
        'm' => KeyM,
        'n' => KeyN,
        'o' => KeyO,
        'p' => KeyP,
        'q' => KeyQ,
        'r' => KeyR,
        's' => KeyS,
        't' => KeyT,
        'u' => KeyU,
        'v' => KeyV,
        'w' => KeyW,
        'x' => KeyX,
        'y' => KeyY,
        'z' => KeyZ,
        _ => return None,
    })
}

/// Resolve the configured `double_tap_key`; unknown values fall back to the
/// platform modifier.
#[cfg(any(windows, target_os = "macos"))]
fn tap_target_from_setting(value: &str) -> TapTarget {
    let v = value.trim().to_lowercase();
    match v.as_str() {
        "" | "ctrl" | "control" | "cmd" | "command" | "super" | "meta" => TapTarget::Modifier,
        "rctrl" | "rightctrl" | "right_ctrl" | "right-ctrl" => {
            TapTarget::Key(rdev::Key::ControlRight)
        }
        "rcmd" | "rightcmd" | "right_cmd" | "right-cmd" | "rmeta" => {
            TapTarget::Key(rdev::Key::MetaRight)
        }
        "lctrl" | "leftctrl" | "left_ctrl" | "left-ctrl" => TapTarget::Key(rdev::Key::ControlLeft),
        other => {
            let mut chars = other.chars();
            match (chars.next().and_then(letter_to_rdev_key), chars.next()) {
                (Some(key), None) => TapTarget::Key(key),
                _ => TapTarget::Modifier,
            }
        }
    }
}

#[cfg(any(windows, target_os = "macos"))]
fn is_modifier_key(key: rdev::Key) -> bool {
    use rdev::Key::*;
    matches!(
        key,
        ControlLeft | ControlRight | ShiftLeft | ShiftRight | Alt | AltGr | MetaLeft | MetaRight
    )
}

/// Double-tap state machine. A tap is the release of the target key with no
/// other key pressed since its press and no modifier held; two taps within
/// the window toggle recording.
#[cfg(any(windows, target_os = "macos"))]
struct TapTracker {
    target: TapTarget,
    last_tap: Option<Instant>,
    combo_used: bool,
    held_modifiers: std::collections::HashSet<rdev::Key>,
}

#[cfg(any(windows, target_os = "macos"))]
impl TapTracker {
    const DOUBLE_TAP_WINDOW: Duration = Duration::from_millis(450);

    fn new() -> Self {
        Self {
            target: TapTarget::Modifier,
            last_tap: None,
            combo_used: false,
            held_modifiers: std::collections::HashSet::new(),
        }
    }

    fn is_target(&self, key: rdev::Key) -> bool {
        match self.target {
            TapTarget::Modifier => is_platform_double_tap_modifier(key),
            TapTarget::Key(tap_key) => key == tap_key,
        }
    }

    fn on_key_press(&mut self, key: rdev::Key) {
        if is_modifier_key(key) {
            self.held_modifiers.insert(key);
        }
        if self.is_target(key) {
            self.combo_used = false;
        } else {
            self.combo_used = true;
            self.last_tap = None;
        }
    }

    /// Returns true when this release completes a double tap.
    fn on_key_release(&mut self, key: rdev::Key) -> bool {
        if is_modifier_key(key) {
            self.held_modifiers.remove(&key);
        }
        if !self.is_target(key) {
            return false;
        }

        let extra_modifier_held = match self.target {
            TapTarget::Modifier => false,
            TapTarget::Key(_) => !self.held_modifiers.is_empty(),
        };
        if self.combo_used || extra_modifier_held {
            self.combo_used = false;
            self.last_tap = None;
            return false;
        }

        let now = Instant::now();
        let is_double = self
            .last_tap
            .is_some_and(|last| now.duration_since(last) <= Self::DOUBLE_TAP_WINDOW);
        self.last_tap = if is_double { None } else { Some(now) };
        is_double
    }

    fn invalidate(&mut self) {
        self.last_tap = None;
    }
}

pub(crate) fn spawn_global_input_listener(app: tauri::AppHandle) {
    std::thread::spawn(move || {
        tracing::info!("Starting global input listener");
        #[cfg(any(windows, target_os = "macos"))]
        let mut tracker = TapTracker::new();
        // In hold (push-to-talk) mode, true while the activation key is down.
        #[cfg(any(windows, target_os = "macos"))]
        let mut hold_active = false;

        if let Err(e) = rdev::listen(move |event| {
            let _ =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match event.event_type {
                    #[cfg(any(windows, target_os = "macos"))]
                    rdev::EventType::KeyPress(key) => {
                        // Refresh target + mode from settings (try_lock keeps
                        // the hook non-blocking; stale-by-one-event is fine).
                        let hold_mode = sync_activation(&app, &mut tracker.target);
                        if hold_mode {
                            // Auto-repeat resends KeyPress; begin_recording is
                            // idempotent, so a held key won't restart.
                            if tracker.is_target(key) && !hold_active {
                                hold_active = true;
                                handle_hold_start(&app);
                            }
                        } else {
                            tracker.on_key_press(key);
                        }
                    }
                    #[cfg(any(windows, target_os = "macos"))]
                    rdev::EventType::KeyRelease(key) => {
                        let hold_mode = sync_activation(&app, &mut tracker.target);
                        if hold_mode {
                            if tracker.is_target(key) && hold_active {
                                hold_active = false;
                                handle_hold_stop(&app);
                            }
                        } else if tracker.on_key_release(key) {
                            handle_toggle(&app);
                            show_widget_window(&app);
                        }
                    }
                    rdev::EventType::ButtonPress(
                        rdev::Button::Left | rdev::Button::Right | rdev::Button::Middle,
                    ) => {
                        #[cfg(any(windows, target_os = "macos"))]
                        tracker.invalidate();
                        handle_click_to_stop(&app);
                    }
                    _ => {}
                }));
        }) {
            tracing::error!("Global input listener failed: {:?}", e);
        }
    });
}

/// Refresh the tap target from settings and report whether hold (PTT) mode
/// is active. Returns false on lock contention (treat as toggle).
#[cfg(any(windows, target_os = "macos"))]
fn sync_activation(app: &tauri::AppHandle, target: &mut TapTarget) -> bool {
    if let Some(state) = app.try_state::<AppState>()
        && let Ok(settings) = state.settings.try_lock()
    {
        *target = tap_target_from_setting(&settings.double_tap_key);
        return settings.activation_mode == "hold";
    }
    false
}

#[cfg(any(windows, target_os = "macos"))]
fn handle_hold_start(app: &tauri::AppHandle) {
    crate::session::begin_recording(app);
    show_widget_window(app);
}

#[cfg(any(windows, target_os = "macos"))]
fn handle_hold_stop(app: &tauri::AppHandle) {
    crate::session::end_recording(app);
}

fn handle_click_to_stop(app: &tauri::AppHandle) {
    let state = app.state::<AppState>();
    let click_to_stop = state
        .settings
        .try_lock()
        .map(|s| s.click_to_stop)
        .unwrap_or(false);
    if !click_to_stop {
        return;
    }
    let is_recording = state.recording.try_lock().map(|g| *g).unwrap_or(false);
    if is_recording {
        handle_toggle(app);
    }
}

#[cfg(test)]
#[cfg(any(windows, target_os = "macos"))]
mod tests {
    use super::*;

    fn tracker_for(key: rdev::Key) -> TapTracker {
        let mut t = TapTracker::new();
        t.target = TapTarget::Key(key);
        t
    }

    #[test]
    fn two_clean_taps_toggle() {
        let mut t = tracker_for(rdev::Key::ControlRight);
        t.on_key_press(rdev::Key::ControlRight);
        assert!(!t.on_key_release(rdev::Key::ControlRight));
        t.on_key_press(rdev::Key::ControlRight);
        assert!(t.on_key_release(rdev::Key::ControlRight));
    }

    #[test]
    fn shortcut_combo_does_not_count_as_tap() {
        let mut t = tracker_for(rdev::Key::KeyV);
        // Ctrl+V: V released while Ctrl held.
        t.on_key_press(rdev::Key::ControlLeft);
        t.on_key_press(rdev::Key::KeyV);
        assert!(!t.on_key_release(rdev::Key::KeyV));
        t.on_key_release(rdev::Key::ControlLeft);
        // A clean tap right after must start a fresh count, not complete one.
        t.on_key_press(rdev::Key::KeyV);
        assert!(!t.on_key_release(rdev::Key::KeyV));
    }

    #[test]
    fn intervening_key_invalidates_tap() {
        let mut t = tracker_for(rdev::Key::ControlRight);
        t.on_key_press(rdev::Key::ControlRight);
        t.on_key_release(rdev::Key::ControlRight);
        t.on_key_press(rdev::Key::KeyA);
        t.on_key_release(rdev::Key::KeyA);
        t.on_key_press(rdev::Key::ControlRight);
        assert!(!t.on_key_release(rdev::Key::ControlRight));
    }
}
