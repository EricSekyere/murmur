use anyhow::{Context, Result};
use arboard::Clipboard;

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

        // Save current clipboard content so we can restore it after pasting.
        let previous = clipboard.get_text().ok();

        clipboard
            .set_text(text)
            .context("Failed to set clipboard text")?;

        // Brief pause to let the clipboard update propagate.
        std::thread::sleep(std::time::Duration::from_millis(30));

        // Release any stuck modifier keys before simulating Ctrl+V.
        #[cfg(windows)]
        super::keyboard::release_all_modifiers_public();

        let paste_result = simulate_paste();

        // Wait for the paste to be processed by the target application.
        std::thread::sleep(std::time::Duration::from_millis(100));

        // Restore the original clipboard content. If there was nothing
        // restorable (the clipboard was empty or held non-text content like
        // an image), clear it instead of leaving our dictated text behind —
        // otherwise the transcription silently lands on the user's clipboard.
        match previous {
            Some(prev) => {
                let _ = clipboard.set_text(&prev);
            }
            None => {
                let _ = clipboard.clear();
            }
        }

        paste_result
    }
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
