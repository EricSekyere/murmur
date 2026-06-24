use anyhow::{Context, Result};
use arboard::Clipboard;

/// How long to leave our text on the clipboard after the paste keystroke so a
/// slow target finishes reading before we restore the previous contents.
const CLIPBOARD_READ_GRACE_MS: u64 = 500;

/// Outputs text by copying to the clipboard and simulating a paste keystroke.
///
/// This is more reliable than direct keystroke simulation in many applications:
/// - Terminals (Warp, Windows Terminal, iTerm)
/// - Elevated / admin windows (bypasses UIPI since clipboard is global)
/// - Electron apps with custom input handling
/// - Browser address bars and developer tools
///
/// The original clipboard content is saved and restored after pasting.
#[derive(Default)]
pub struct ClipboardPasteOutput;

impl ClipboardPasteOutput {
    pub fn new() -> Self {
        Self
    }

    /// Copy text to clipboard and simulate Ctrl+V (Cmd+V on macOS) to paste it.
    ///
    /// Saves and restores the previous clipboard content.
    pub fn paste_text(&self, text: &str) -> Result<()> {
        if text.is_empty() {
            return Ok(());
        }

        #[cfg(windows)]
        super::keyboard::log_foreground_window_public();

        tracing::debug!(
            "Pasting {} characters via clipboard + paste shortcut",
            text.len()
        );

        let mut clipboard =
            Clipboard::new().context("Failed to open clipboard for paste output")?;

        // Save current clipboard content (text or image) so we can restore it
        // after pasting and not destroy what the user had copied.
        let previous = capture_clipboard(&mut clipboard);

        clipboard
            .set_text(text)
            .context("Failed to set clipboard text")?;

        // Wait until the clipboard actually reports our text before pasting.
        // A fixed sleep is unreliable: if Ctrl+V fires before the OS finishes
        // updating the clipboard, it pastes the user's *previous* clipboard
        // content into the target window. Poll up to ~250ms for confirmation.
        let mut confirmed = false;
        for _ in 0..25 {
            if clipboard.get_text().is_ok_and(|t| t == text) {
                confirmed = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        if !confirmed {
            tracing::warn!(
                "Clipboard did not confirm our text; skipping paste to avoid pasting stale content"
            );
            // Restore the original content and bail; the caller's fallback
            // chain (keyboard typing) will deliver the text instead.
            restore_clipboard(&mut clipboard, &previous);
            anyhow::bail!("Clipboard update not confirmed; refusing to paste stale content");
        }

        // Release any stuck modifier keys before simulating Ctrl+V.
        #[cfg(windows)]
        super::keyboard::release_all_modifiers_public();

        let paste_result = simulate_paste();

        // Give the target app time to actually read the clipboard before we
        // restore it. If we restore too soon, a slow app (e.g. an Electron
        // terminal under load) reads the clipboard after the restore and
        // pastes the user's previous content instead of our text. This path is
        // only reached for genuine terminals, so the wait is off the common
        // path; favour a wider window over a tighter one to avoid leaking the
        // previous clipboard. This is still best-effort, not a guarantee.
        std::thread::sleep(std::time::Duration::from_millis(CLIPBOARD_READ_GRACE_MS));

        // Restore the original clipboard content (text or image). If there was
        // nothing restorable, clear it rather than leave our dictated text on
        // the user's clipboard.
        restore_clipboard(&mut clipboard, &previous);

        paste_result
    }
}

/// What the clipboard held before we overwrote it with the dictated text.
enum PreviousClipboard {
    Text(String),
    Image(arboard::ImageData<'static>),
    Empty,
}

fn capture_clipboard(clipboard: &mut Clipboard) -> PreviousClipboard {
    if let Ok(text) = clipboard.get_text() {
        PreviousClipboard::Text(text)
    } else if let Ok(image) = clipboard.get_image() {
        PreviousClipboard::Image(image)
    } else {
        PreviousClipboard::Empty
    }
}

fn restore_clipboard(clipboard: &mut Clipboard, previous: &PreviousClipboard) {
    let _ = match previous {
        PreviousClipboard::Text(text) => clipboard.set_text(text),
        PreviousClipboard::Image(image) => clipboard.set_image(image.clone()),
        PreviousClipboard::Empty => clipboard.clear(),
    };
}

/// Simulate Ctrl+V (Windows/Linux) or Cmd+V (macOS).
#[cfg(windows)]
fn simulate_paste() -> Result<()> {
    use std::mem;

    // Reuse the same FFI layout as keyboard.rs.
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct KeybdInput {
        w_vk: u16,
        w_scan: u16,
        dw_flags: u32,
        time: u32,
        dw_extra_info: usize,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Input {
        input_type: u32,
        _padding: u32,
        ki: KeybdInput,
        _extra: u32,
    }

    const INPUT_KEYBOARD: u32 = 1;
    const KEYEVENTF_KEYUP: u32 = 0x0002;
    const VK_CONTROL: u16 = 0x11;
    const VK_V: u16 = 0x56;

    unsafe extern "system" {
        fn SendInput(c_inputs: u32, p_inputs: *const Input, cb_size: i32) -> u32;
    }

    fn make_input(vk: u16, flags: u32) -> Input {
        Input {
            input_type: INPUT_KEYBOARD,
            _padding: 0,
            ki: KeybdInput {
                w_vk: vk,
                w_scan: 0,
                dw_flags: flags,
                time: 0,
                dw_extra_info: 0,
            },
            _extra: 0,
        }
    }

    let inputs = [
        make_input(VK_CONTROL, 0),               // Ctrl down
        make_input(VK_V, 0),                     // V down
        make_input(VK_V, KEYEVENTF_KEYUP),       // V up
        make_input(VK_CONTROL, KEYEVENTF_KEYUP), // Ctrl up
    ];

    let sent = unsafe {
        SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            mem::size_of::<Input>() as i32,
        )
    };

    if sent == 0 {
        anyhow::bail!("SendInput failed for Ctrl+V paste simulation");
    }

    tracing::debug!(
        "Simulated Ctrl+V paste ({}/{} events sent)",
        sent,
        inputs.len()
    );
    Ok(())
}

/// Simulate Cmd+V (macOS) or Ctrl+V (Linux) using enigo.
#[cfg(not(windows))]
fn simulate_paste() -> Result<()> {
    use enigo::{Enigo, Key, Keyboard, Settings};

    let mut enigo = Enigo::new(&Settings::default())?;

    let modifier = if cfg!(target_os = "macos") {
        Key::Meta
    } else {
        Key::Control
    };

    enigo.key(modifier, enigo::Direction::Press)?;
    let res = (|| {
        enigo.key(Key::Unicode('v'), enigo::Direction::Press)?;
        enigo.key(Key::Unicode('v'), enigo::Direction::Release)?;
        Ok::<(), anyhow::Error>(())
    })();
    let _ = enigo.key(modifier, enigo::Direction::Release);
    res?;

    tracing::debug!("Simulated paste keystroke via enigo");
    Ok(())
}
