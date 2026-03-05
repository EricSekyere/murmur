use anyhow::Result;

/// Simulates keystrokes to type text into the currently focused application.
///
/// On Windows, uses the `SendInput` API with `KEYEVENTF_UNICODE` to inject
/// characters at the OS kernel level — the same mechanism used by Windows
/// Dictation and IME input. Works in any window that accepts keyboard input:
/// terminals, browsers, editors, elevated windows. No clipboard involved.
///
/// On other platforms, falls back to clipboard + paste.
pub struct KeyboardOutput;

impl KeyboardOutput {
    pub fn new() -> Result<Self> {
        Ok(Self)
    }

    /// Type text into the focused application.
    pub fn type_text(&mut self, text: &str) -> Result<()> {
        if text.is_empty() {
            return Ok(());
        }

        #[cfg(windows)]
        {
            tracing::debug!("Typing {} characters via SendInput (Unicode)", text.len());
            send_unicode_input(text)
        }

        #[cfg(not(windows))]
        {
            tracing::debug!("Typing {} characters via clipboard paste", text.len());
            clipboard_paste(text)
        }
    }
}

// ─── Windows: SendInput with KEYEVENTF_UNICODE ──────────────────────────────

#[cfg(windows)]
fn send_unicode_input(text: &str) -> Result<()> {
    use std::mem;

    // FFI types matching the Windows INPUT / KEYBDINPUT structures.
    // Defined inline to avoid pulling in a large Windows bindings crate.
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct KeybdInput {
        w_vk: u16,
        w_scan: u16,
        dw_flags: u32,
        time: u32,
        dw_extra_info: usize,
    }

    // The INPUT struct's union has mouse, keyboard, and hardware variants.
    // We only use keyboard, but padding must match the largest variant (mouse)
    // which is 32 bytes on x64. Total INPUT size = 4 (type) + 4 (padding) + 32 = 40.
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Input {
        input_type: u32,
        _padding: u32, // alignment padding before union on x64
        ki: KeybdInput,
        // Remaining padding to match MOUSEINPUT union size (largest variant).
        // MOUSEINPUT is 28 bytes on x64; KEYBDINPUT is 24 bytes.
        _extra: u32,
    }

    const INPUT_KEYBOARD: u32 = 1;
    const KEYEVENTF_UNICODE: u32 = 0x0004;
    const KEYEVENTF_KEYUP: u32 = 0x0002;
    const VK_RETURN: u16 = 0x0D;

    unsafe extern "system" {
        fn SendInput(c_inputs: u32, p_inputs: *const Input, cb_size: i32) -> u32;
    }

    fn make_key_down(vk: u16, scan: u16, flags: u32) -> Input {
        Input {
            input_type: INPUT_KEYBOARD,
            _padding: 0,
            ki: KeybdInput {
                w_vk: vk,
                w_scan: scan,
                dw_flags: flags,
                time: 0,
                dw_extra_info: 0,
            },
            _extra: 0,
        }
    }

    fn make_key_up(vk: u16, scan: u16, flags: u32) -> Input {
        Input {
            input_type: INPUT_KEYBOARD,
            _padding: 0,
            ki: KeybdInput {
                w_vk: vk,
                w_scan: scan,
                dw_flags: flags | KEYEVENTF_KEYUP,
                time: 0,
                dw_extra_info: 0,
            },
            _extra: 0,
        }
    }

    let mut inputs: Vec<Input> = Vec::with_capacity(text.len() * 2);

    for ch in text.chars() {
        match ch {
            // Newlines → send VK_RETURN (Enter key) for maximum compatibility
            '\n' | '\r' => {
                inputs.push(make_key_down(VK_RETURN, 0, 0));
                inputs.push(make_key_up(VK_RETURN, 0, 0));
            }
            // Everything else → KEYEVENTF_UNICODE with UTF-16 code units
            _ => {
                let mut utf16_buf = [0u16; 2];
                let utf16 = ch.encode_utf16(&mut utf16_buf);
                for &mut code_unit in utf16 {
                    inputs.push(make_key_down(0, code_unit, KEYEVENTF_UNICODE));
                    inputs.push(make_key_up(0, code_unit, KEYEVENTF_UNICODE));
                }
            }
        }
    }

    if inputs.is_empty() {
        return Ok(());
    }

    let sent = unsafe {
        SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            mem::size_of::<Input>() as i32,
        )
    };

    if sent == 0 {
        anyhow::bail!(
            "SendInput failed — the target window may be elevated (run as administrator)"
        );
    }

    if sent != inputs.len() as u32 {
        tracing::warn!(
            "SendInput sent {}/{} events (some may have been blocked by the target app)",
            sent,
            inputs.len()
        );
    }

    Ok(())
}

// ─── Non-Windows: clipboard + paste fallback ─────────────────────────────────

#[cfg(not(windows))]
fn clipboard_paste(text: &str) -> Result<()> {
    use anyhow::Context;
    use arboard::Clipboard;
    use enigo::{Enigo, Key, Keyboard, Settings};

    let mut clipboard = Clipboard::new().context("Failed to open clipboard")?;
    let previous = clipboard.get_text().ok();

    clipboard
        .set_text(text)
        .context("Failed to set clipboard text")?;

    std::thread::sleep(std::time::Duration::from_millis(50));

    let mut enigo = Enigo::new(&Settings::default())?;

    // macOS uses Cmd+V, others use Ctrl+V
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

    std::thread::sleep(std::time::Duration::from_millis(150));

    // Restore original clipboard
    if let Some(prev) = previous {
        let _ = clipboard.set_text(&prev);
    }

    Ok(())
}
