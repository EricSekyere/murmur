use anyhow::Result;

/// Simulates keystrokes to type text into the currently focused application.
///
/// On Windows, uses the `SendInput` API with `KEYEVENTF_UNICODE` to inject
/// characters at the OS kernel level — the same mechanism used by Windows
/// Dictation and IME input. Works in any window that accepts keyboard input:
/// terminals, browsers, editors, elevated windows. No clipboard involved.
///
/// On other platforms, falls back to clipboard + paste.
pub struct KeyboardOutput {
    /// Milliseconds to wait before sending input, giving the target window
    /// time to regain focus after a hotkey release.
    pre_delay_ms: u64,
}

impl KeyboardOutput {
    pub fn new() -> Result<Self> {
        Ok(Self { pre_delay_ms: 80 })
    }

    /// Create with a custom pre-output delay.
    pub fn with_delay(pre_delay_ms: u64) -> Result<Self> {
        Ok(Self { pre_delay_ms })
    }

    /// Type text into the focused application.
    pub fn type_text(&mut self, text: &str) -> Result<()> {
        if text.is_empty() {
            return Ok(());
        }

        #[cfg(windows)]
        log_foreground_window();

        if self.pre_delay_ms > 0 {
            tracing::trace!("Pre-output delay: {}ms", self.pre_delay_ms);
            std::thread::sleep(std::time::Duration::from_millis(self.pre_delay_ms));
        }

        #[cfg(windows)]
        {
            // Release all modifier keys first to prevent interference.
            // After a hotkey like Ctrl+Q, some apps still see modifiers as held.
            release_all_modifiers();

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

/// Press Enter `count` times in the focused window (for "new line" /
/// "new paragraph" voice commands).
pub fn press_enter(count: usize) -> Result<()> {
    if count == 0 {
        return Ok(());
    }
    #[cfg(windows)]
    {
        const VK_RETURN: u16 = 0x0D;
        send_vk_taps(VK_RETURN, count)
    }
    #[cfg(not(windows))]
    {
        send_enigo_key(enigo::Key::Return, count)
    }
}

/// Press Backspace `count` times in the focused window (for "scratch that").
pub fn press_backspace(count: usize) -> Result<()> {
    if count == 0 {
        return Ok(());
    }
    #[cfg(windows)]
    {
        const VK_BACK: u16 = 0x08;
        send_vk_taps(VK_BACK, count)
    }
    #[cfg(not(windows))]
    {
        send_enigo_key(enigo::Key::Backspace, count)
    }
}

/// Editing chords for spoken commands. The primary modifier is Ctrl on
/// Windows/Linux and Cmd on macOS, matching each platform's conventions.
pub fn select_all() -> Result<()> {
    primary_chord(b'A')
}

pub fn copy() -> Result<()> {
    primary_chord(b'C')
}

pub fn cut() -> Result<()> {
    primary_chord(b'X')
}

pub fn paste() -> Result<()> {
    primary_chord(b'V')
}

pub fn undo() -> Result<()> {
    primary_chord(b'Z')
}

/// Redo: Ctrl+Y on Windows, primary+Shift+Z elsewhere (the common binding on
/// macOS and Linux apps).
pub fn redo() -> Result<()> {
    #[cfg(windows)]
    {
        const VK_Y: u16 = 0x59;
        send_chord(&[VK_CONTROL], VK_Y)
    }
    #[cfg(not(windows))]
    {
        send_enigo_chord(
            &[primary_modifier(), enigo::Key::Shift],
            enigo::Key::Unicode('z'),
        )
    }
}

pub fn press_tab() -> Result<()> {
    #[cfg(windows)]
    {
        const VK_TAB: u16 = 0x09;
        send_vk_taps(VK_TAB, 1)
    }
    #[cfg(not(windows))]
    {
        send_enigo_key(enigo::Key::Tab, 1)
    }
}

pub fn press_escape() -> Result<()> {
    #[cfg(windows)]
    {
        const VK_ESCAPE: u16 = 0x1B;
        send_vk_taps(VK_ESCAPE, 1)
    }
    #[cfg(not(windows))]
    {
        send_enigo_key(enigo::Key::Escape, 1)
    }
}

/// Tap an ASCII letter while holding the platform's primary modifier.
fn primary_chord(ascii_upper: u8) -> Result<()> {
    #[cfg(windows)]
    {
        // Letter VK codes equal their uppercase ASCII value.
        send_chord(&[VK_CONTROL], ascii_upper as u16)
    }
    #[cfg(not(windows))]
    {
        let key = enigo::Key::Unicode(ascii_upper.to_ascii_lowercase() as char);
        send_enigo_chord(&[primary_modifier()], key)
    }
}

#[cfg(windows)]
const VK_CONTROL: u16 = 0x11;

#[cfg(not(windows))]
fn primary_modifier() -> enigo::Key {
    #[cfg(target_os = "macos")]
    {
        enigo::Key::Meta
    }
    #[cfg(not(target_os = "macos"))]
    {
        enigo::Key::Control
    }
}

#[cfg(not(windows))]
fn send_enigo_key(key: enigo::Key, count: usize) -> Result<()> {
    use enigo::{Enigo, Keyboard, Settings};
    let mut enigo = Enigo::new(&Settings::default())?;
    for _ in 0..count {
        enigo.key(key, enigo::Direction::Click)?;
    }
    Ok(())
}

/// Press modifiers, click `key`, then release modifiers in reverse order.
#[cfg(not(windows))]
fn send_enigo_chord(modifiers: &[enigo::Key], key: enigo::Key) -> Result<()> {
    use enigo::{Direction, Enigo, Keyboard, Settings};
    let mut enigo = Enigo::new(&Settings::default())?;
    for &m in modifiers {
        enigo.key(m, Direction::Press)?;
    }
    enigo.key(key, Direction::Click)?;
    for &m in modifiers.iter().rev() {
        enigo.key(m, Direction::Release)?;
    }
    Ok(())
}

/// Send a modifier chord (e.g. Ctrl+C) via SendInput: modifiers down, key
/// down, key up, modifiers up.
#[cfg(windows)]
fn send_chord(modifiers: &[u16], vk: u16) -> Result<()> {
    use std::mem;

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

    unsafe extern "system" {
        fn SendInput(c_inputs: u32, p_inputs: *const Input, cb_size: i32) -> u32;
    }

    let event = |vk: u16, flags: u32| Input {
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
    };

    let mut inputs = Vec::with_capacity(modifiers.len() * 2 + 2);
    for &m in modifiers {
        inputs.push(event(m, 0));
    }
    inputs.push(event(vk, 0));
    inputs.push(event(vk, KEYEVENTF_KEYUP));
    for &m in modifiers.iter().rev() {
        inputs.push(event(m, KEYEVENTF_KEYUP));
    }

    let sent = unsafe {
        SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            mem::size_of::<Input>() as i32,
        )
    };
    if sent == 0 {
        anyhow::bail!("SendInput failed for chord (target may be elevated)");
    }
    Ok(())
}

/// Send `count` down+up pairs of a single virtual-key via SendInput.
#[cfg(windows)]
fn send_vk_taps(vk: u16, count: usize) -> Result<()> {
    use std::mem;

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

    unsafe extern "system" {
        fn SendInput(c_inputs: u32, p_inputs: *const Input, cb_size: i32) -> u32;
    }

    let event = |flags: u32| Input {
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
    };

    let mut inputs = Vec::with_capacity(count * 2);
    for _ in 0..count {
        inputs.push(event(0));
        inputs.push(event(KEYEVENTF_KEYUP));
    }

    let sent = unsafe {
        SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            mem::size_of::<Input>() as i32,
        )
    };
    if sent == 0 {
        anyhow::bail!("SendInput failed for key taps (target may be elevated)");
    }
    Ok(())
}

/// Log the foreground window title and class for diagnostics.
/// Public wrapper for use by other output modules.
#[cfg(windows)]
pub fn log_foreground_window_public() {
    log_foreground_window();
}

/// Release all stuck modifier keys.
/// Public wrapper for use by other output modules.
#[cfg(windows)]
pub fn release_all_modifiers_public() {
    release_all_modifiers();
}

/// Foreground window diagnostics for Windows output routing decisions.
#[cfg(windows)]
#[derive(Debug, Clone)]
pub struct ForegroundWindowInfo {
    pub title: String,
    pub class_name: String,
    pub process_name: Option<String>,
}

/// Read foreground window info for heuristics such as terminal detection.
#[cfg(windows)]
pub fn foreground_window_info_public() -> Option<ForegroundWindowInfo> {
    foreground_window_info()
}

// ─── Windows: diagnostics ────────────────────────────────────────────────────

#[cfg(windows)]
fn log_foreground_window() {
    unsafe extern "system" {
        fn GetForegroundWindow() -> *mut std::ffi::c_void;
    }

    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.is_null() {
        tracing::debug!("No foreground window detected");
        return;
    }

    if let Some(info) = foreground_window_info() {
        tracing::info!(
            "Target window: title={:?}, class={:?}, process={:?}, hwnd={:?}",
            info.title,
            info.class_name,
            info.process_name,
            hwnd
        );
    }
}

#[cfg(windows)]
fn foreground_window_info() -> Option<ForegroundWindowInfo> {
    use std::ffi::c_void;

    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;

    unsafe extern "system" {
        fn GetForegroundWindow() -> *mut c_void;
        fn GetWindowTextW(hwnd: *mut c_void, lp_string: *mut u16, n_max: i32) -> i32;
        fn GetClassNameW(hwnd: *mut c_void, lp_class: *mut u16, n_max: i32) -> i32;
        fn GetWindowThreadProcessId(hwnd: *mut c_void, lpdw_process_id: *mut u32) -> u32;
        fn OpenProcess(
            dw_desired_access: u32,
            b_inherit_handle: i32,
            dw_process_id: u32,
        ) -> *mut c_void;
        fn QueryFullProcessImageNameW(
            h_process: *mut c_void,
            dw_flags: u32,
            lp_exe_name: *mut u16,
            lpdw_size: *mut u32,
        ) -> i32;
        fn CloseHandle(h_object: *mut c_void) -> i32;
    }

    fn lossy_wide_to_string(buf: &[u16], len: i32) -> String {
        let len = len.max(0) as usize;
        String::from_utf16_lossy(&buf[..len.min(buf.len())])
    }

    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.is_null() {
        return None;
    }

    let mut title_buf = [0u16; 512];
    let title_len = unsafe { GetWindowTextW(hwnd, title_buf.as_mut_ptr(), title_buf.len() as i32) };
    let title = lossy_wide_to_string(&title_buf, title_len);

    let mut class_buf = [0u16; 256];
    let class_len = unsafe { GetClassNameW(hwnd, class_buf.as_mut_ptr(), class_buf.len() as i32) };
    let class_name = lossy_wide_to_string(&class_buf, class_len);

    let mut process_id = 0u32;
    unsafe {
        GetWindowThreadProcessId(hwnd, &mut process_id);
    }

    let process_name = if process_id == 0 {
        None
    } else {
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, process_id) };
        if handle.is_null() {
            None
        } else {
            let mut path_buf = [0u16; 1024];
            let mut size = path_buf.len() as u32;
            let ok =
                unsafe { QueryFullProcessImageNameW(handle, 0, path_buf.as_mut_ptr(), &mut size) };
            let _ = unsafe { CloseHandle(handle) };

            if ok == 0 || size == 0 {
                None
            } else {
                let path = String::from_utf16_lossy(&path_buf[..size as usize]);
                std::path::Path::new(&path)
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            }
        }
    };

    Some(ForegroundWindowInfo {
        title,
        class_name,
        process_name,
    })
}

// ─── Windows: release stuck modifier keys ────────────────────────────────────

/// Send key-up events for all modifier keys to prevent interference.
///
/// After a global hotkey like Ctrl+Q is released, some applications still see
/// the modifier as "held" because the key-up event was consumed by the hotkey
/// system. Explicitly releasing all modifiers ensures the subsequent text input
/// isn't misinterpreted as shortcut combinations (e.g., Ctrl+H instead of 'h').
#[cfg(windows)]
fn release_all_modifiers() {
    use std::mem;

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

    // All modifier virtual key codes
    const VK_LCONTROL: u16 = 0xA2;
    const VK_RCONTROL: u16 = 0xA3;
    const VK_LSHIFT: u16 = 0xA0;
    const VK_RSHIFT: u16 = 0xA1;
    const VK_LMENU: u16 = 0xA4; // Left Alt
    const VK_RMENU: u16 = 0xA5; // Right Alt
    const VK_LWIN: u16 = 0x5B;
    const VK_RWIN: u16 = 0x5C;

    unsafe extern "system" {
        fn SendInput(c_inputs: u32, p_inputs: *const Input, cb_size: i32) -> u32;
        fn GetAsyncKeyState(v_key: i32) -> i16;
    }

    let modifiers = [
        VK_LCONTROL,
        VK_RCONTROL,
        VK_LSHIFT,
        VK_RSHIFT,
        VK_LMENU,
        VK_RMENU,
        VK_LWIN,
        VK_RWIN,
    ];

    let mut inputs: Vec<Input> = Vec::new();

    for &vk in &modifiers {
        // Only release keys that are currently pressed (bit 15 set = key is down)
        let state = unsafe { GetAsyncKeyState(vk as i32) };
        if state & (1 << 15) != 0 {
            tracing::debug!("Releasing stuck modifier key: VK 0x{:02X}", vk);
            inputs.push(Input {
                input_type: INPUT_KEYBOARD,
                _padding: 0,
                ki: KeybdInput {
                    w_vk: vk,
                    w_scan: 0,
                    dw_flags: KEYEVENTF_KEYUP,
                    time: 0,
                    dw_extra_info: 0,
                },
                _extra: 0,
            });
        }
    }

    if inputs.is_empty() {
        tracing::trace!("No stuck modifier keys detected");
        return;
    }

    let sent = unsafe {
        SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            mem::size_of::<Input>() as i32,
        )
    };

    tracing::info!("Released {}/{} stuck modifier keys", sent, inputs.len());

    // Brief pause to let the modifier release propagate
    std::thread::sleep(std::time::Duration::from_millis(20));
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

    // Restore the original clipboard, or clear it when there was nothing
    // restorable, so the dictated text is never left behind.
    match previous {
        Some(prev) => {
            let _ = clipboard.set_text(&prev);
        }
        None => {
            let _ = clipboard.clear();
        }
    }

    Ok(())
}
