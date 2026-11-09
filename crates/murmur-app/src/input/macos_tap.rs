//! macOS global input through a listen-only Quartz event tap.
//!
//! Stands in for `rdev::listen`, which resolves every key press to text with
//! the Text Input Source APIs. macOS traps those calls off the main thread,
//! and the listener can't run on the main thread (Tauri owns it), so the first
//! keystroke killed the app. Murmur only needs key codes, so this tap never
//! touches those APIs.

use std::cell::RefCell;

use core_foundation::runloop::CFRunLoop;
use core_graphics::event::{
    CGEvent, CGEventFlags, CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement,
    CGEventType, CallbackResult, EventField,
};
use rdev::{Button, EventType, Key};

/// Apple virtual key codes (`kVK_*`) of the modifiers, each with the
/// device-dependent flag bit (`NX_DEVICE*KEYMASK`) set while that key is held.
/// Modifiers only report a flags change, so this bit is what tells a press
/// from a release, even while the same modifier on the other side is down.
const MODIFIERS: [(u16, Key, u64); 8] = [
    (0x3B, Key::ControlLeft, 0x0000_0001),
    (0x38, Key::ShiftLeft, 0x0000_0002),
    (0x3C, Key::ShiftRight, 0x0000_0004),
    (0x37, Key::MetaLeft, 0x0000_0008),
    (0x36, Key::MetaRight, 0x0000_0010),
    (0x3A, Key::Alt, 0x0000_0020),
    (0x3D, Key::AltGr, 0x0000_0040),
    (0x3E, Key::ControlRight, 0x0000_2000),
];

/// `kVK_ANSI_*` codes of the letter keys a double-tap target can name.
const LETTERS: [(u16, Key); 26] = [
    (0x00, Key::KeyA),
    (0x0B, Key::KeyB),
    (0x08, Key::KeyC),
    (0x02, Key::KeyD),
    (0x0E, Key::KeyE),
    (0x03, Key::KeyF),
    (0x05, Key::KeyG),
    (0x04, Key::KeyH),
    (0x22, Key::KeyI),
    (0x26, Key::KeyJ),
    (0x28, Key::KeyK),
    (0x25, Key::KeyL),
    (0x2E, Key::KeyM),
    (0x2D, Key::KeyN),
    (0x1F, Key::KeyO),
    (0x23, Key::KeyP),
    (0x0C, Key::KeyQ),
    (0x0F, Key::KeyR),
    (0x01, Key::KeyS),
    (0x11, Key::KeyT),
    (0x20, Key::KeyU),
    (0x09, Key::KeyV),
    (0x0D, Key::KeyW),
    (0x07, Key::KeyX),
    (0x10, Key::KeyY),
    (0x06, Key::KeyZ),
];

/// Deliver key and mouse-button events to `on_event`, blocking the calling
/// thread on its run loop for as long as the app runs.
///
/// # Errors
/// When macOS refuses the event tap, typically because Input Monitoring
/// hasn't been allowed for the app.
pub(super) fn listen(on_event: impl FnMut(EventType)) -> anyhow::Result<()> {
    let on_event = RefCell::new(on_event);
    CGEventTap::with_enabled(
        CGEventTapLocation::HID,
        CGEventTapPlacement::HeadInsertEventTap,
        CGEventTapOptions::ListenOnly,
        vec![
            CGEventType::KeyDown,
            CGEventType::KeyUp,
            CGEventType::FlagsChanged,
            CGEventType::LeftMouseDown,
            CGEventType::RightMouseDown,
        ],
        |_proxy, event_type, event| {
            // The tap fires only on this thread's run loop, so the borrow is
            // never contended; try_borrow_mut just keeps a panic path out.
            if let (Some(converted), Ok(mut on_event)) =
                (convert(event_type, event), on_event.try_borrow_mut())
            {
                on_event(converted);
            }
            CallbackResult::Keep
        },
        CFRunLoop::run_current,
    )
    .map_err(|()| anyhow::anyhow!("macOS refused the event tap (Input Monitoring not allowed?)"))
}

fn convert(event_type: CGEventType, event: &CGEvent) -> Option<EventType> {
    match event_type {
        CGEventType::LeftMouseDown => Some(EventType::ButtonPress(Button::Left)),
        CGEventType::RightMouseDown => Some(EventType::ButtonPress(Button::Right)),
        CGEventType::KeyDown | CGEventType::KeyUp | CGEventType::FlagsChanged => {
            let code = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE);
            key_event(event_type, u16::try_from(code).ok()?, event.get_flags())
        }
        _ => None,
    }
}

/// Classify a keyboard event by its key code, never by the text it types.
fn key_event(event_type: CGEventType, code: u16, flags: CGEventFlags) -> Option<EventType> {
    let key = key_from_code(code);
    match event_type {
        CGEventType::KeyDown => Some(EventType::KeyPress(key)),
        CGEventType::KeyUp => Some(EventType::KeyRelease(key)),
        CGEventType::FlagsChanged => {
            // Caps Lock and Fn have no device bit; calling them a press is
            // enough, since all they can do is cancel a pending double tap.
            let held = MODIFIERS
                .iter()
                .find(|&&(modifier, ..)| modifier == code)
                .is_none_or(|&(_, _, mask)| flags.bits() & mask != 0);
            Some(if held {
                EventType::KeyPress(key)
            } else {
                EventType::KeyRelease(key)
            })
        }
        _ => None,
    }
}

fn key_from_code(code: u16) -> Key {
    MODIFIERS
        .iter()
        .map(|&(modifier, key, _)| (modifier, key))
        .chain(LETTERS)
        .find_map(|(known, key)| (known == code).then_some(key))
        .unwrap_or(Key::Unknown(u32::from(code)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEFT_COMMAND: u16 = 0x37;
    const RIGHT_COMMAND: u16 = 0x36;
    const RIGHT_CONTROL: u16 = 0x3E;
    const SPACE: u16 = 0x31;
    const V: u16 = 0x09;
    const LEFT_COMMAND_HELD: u64 = 0x0010_0008;

    fn flags(bits: u64) -> CGEventFlags {
        CGEventFlags::from_bits_retain(bits)
    }

    #[test]
    fn left_command_goes_down_then_up() {
        assert_eq!(
            key_event(
                CGEventType::FlagsChanged,
                LEFT_COMMAND,
                flags(LEFT_COMMAND_HELD)
            ),
            Some(EventType::KeyPress(Key::MetaLeft))
        );
        assert_eq!(
            key_event(CGEventType::FlagsChanged, LEFT_COMMAND, flags(0)),
            Some(EventType::KeyRelease(Key::MetaLeft))
        );
    }

    #[test]
    fn releasing_right_command_while_left_is_held_is_a_release() {
        assert_eq!(
            key_event(
                CGEventType::FlagsChanged,
                RIGHT_COMMAND,
                flags(LEFT_COMMAND_HELD)
            ),
            Some(EventType::KeyRelease(Key::MetaRight))
        );
    }

    #[test]
    fn right_control_is_recognised() {
        assert_eq!(
            key_event(CGEventType::FlagsChanged, RIGHT_CONTROL, flags(0x0004_2000)),
            Some(EventType::KeyPress(Key::ControlRight))
        );
    }

    #[test]
    fn letter_keys_map_to_their_letter() {
        assert_eq!(
            key_event(CGEventType::KeyDown, V, flags(0)),
            Some(EventType::KeyPress(Key::KeyV))
        );
        assert_eq!(
            key_event(CGEventType::KeyUp, V, flags(0)),
            Some(EventType::KeyRelease(Key::KeyV))
        );
    }

    #[test]
    fn other_keys_are_reported_by_code() {
        assert_eq!(
            key_event(CGEventType::KeyDown, SPACE, flags(0)),
            Some(EventType::KeyPress(Key::Unknown(u32::from(SPACE))))
        );
    }
}
