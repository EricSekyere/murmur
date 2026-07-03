//! Native desktop actions behind voice commands: app/URL launch, window
//! focus, keystrokes.
//!
//! The [`NativeActions`] trait keeps the executor's dispatch logic testable
//! with a mock; [`SystemActions`] is the real OS-backed implementation.

use anyhow::{Context, Result, bail};

/// Native actions a matched voice command can trigger. Implementations run
/// quick OS calls, never long work.
pub trait NativeActions {
    /// Launch an application, document, or URL.
    fn launch(&self, target: &str) -> Result<()>;

    /// Bring the first visible window whose title contains `query`
    /// (case-insensitive) to the foreground.
    fn focus_window(&self, query: &str) -> Result<()>;

    /// Send a key chord spoken as space or `+` separated names, e.g.
    /// "control shift p", "ctrl+enter", "f5".
    fn send_keys(&self, keys: &str) -> Result<()>;
}

/// OS-backed [`NativeActions`]: ShellExecuteW / EnumWindows on Windows, a
/// best-effort desktop opener elsewhere, enigo keystrokes everywhere.
pub struct SystemActions;

impl NativeActions for SystemActions {
    fn launch(&self, target: &str) -> Result<()> {
        platform::launch(target)
    }

    fn focus_window(&self, query: &str) -> Result<()> {
        platform::focus_window(query)
    }

    fn send_keys(&self, keys: &str) -> Result<()> {
        press_chord(&parse_chord(keys)?)
    }
}

/// A parsed key spec: modifiers held around the tapped keys.
struct Chord {
    modifiers: Vec<enigo::Key>,
    taps: Vec<enigo::Key>,
}

enum ParsedKey {
    Modifier(enigo::Key),
    Tap(enigo::Key),
}

/// Parse a spoken key spec into a [`Chord`]. Unknown names are rejected so a
/// misheard phrase can never press something else instead.
fn parse_chord(keys: &str) -> Result<Chord> {
    let mut chord = Chord {
        modifiers: Vec::new(),
        taps: Vec::new(),
    };
    let tokens = keys
        .split(|c: char| c.is_whitespace() || c == '+')
        .filter(|t| !t.is_empty());
    for token in tokens {
        match parse_key(&token.to_lowercase()) {
            Some(ParsedKey::Modifier(key)) => chord.modifiers.push(key),
            Some(ParsedKey::Tap(key)) => chord.taps.push(key),
            // Privacy: the spec comes from the utterance; never name it in
            // the error or above trace.
            None => {
                tracing::trace!(?token, "unrecognized key name");
                bail!("unrecognized key name in key command");
            }
        }
    }
    if chord.modifiers.is_empty() && chord.taps.is_empty() {
        bail!("no keys to press");
    }
    Ok(chord)
}

/// Map one lowercased token to a key; `None` for unknown names.
fn parse_key(token: &str) -> Option<ParsedKey> {
    use enigo::Key;
    let modifier = match token {
        "ctrl" | "control" => Some(Key::Control),
        "shift" => Some(Key::Shift),
        "alt" | "option" => Some(Key::Alt),
        "win" | "windows" | "meta" | "super" | "cmd" | "command" => Some(Key::Meta),
        _ => None,
    };
    if let Some(key) = modifier {
        return Some(ParsedKey::Modifier(key));
    }
    let tap = match token {
        "enter" | "return" => Key::Return,
        "tab" => Key::Tab,
        "escape" | "esc" => Key::Escape,
        "space" | "spacebar" => Key::Space,
        "backspace" => Key::Backspace,
        "delete" | "del" => Key::Delete,
        "home" => Key::Home,
        "end" => Key::End,
        "up" => Key::UpArrow,
        "down" => Key::DownArrow,
        "left" => Key::LeftArrow,
        "right" => Key::RightArrow,
        "pageup" => Key::PageUp,
        "pagedown" => Key::PageDown,
        _ => return function_or_char(token),
    };
    Some(ParsedKey::Tap(tap))
}

/// `f1`..`f12`, or a single character key.
fn function_or_char(token: &str) -> Option<ParsedKey> {
    use enigo::Key;
    const F_KEYS: [Key; 12] = [
        Key::F1,
        Key::F2,
        Key::F3,
        Key::F4,
        Key::F5,
        Key::F6,
        Key::F7,
        Key::F8,
        Key::F9,
        Key::F10,
        Key::F11,
        Key::F12,
    ];
    if let Some(n) = token
        .strip_prefix('f')
        .and_then(|d| d.parse::<usize>().ok())
    {
        return n
            .checked_sub(1)
            .and_then(|i| F_KEYS.get(i))
            .copied()
            .map(ParsedKey::Tap);
    }
    let mut chars = token.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => Some(ParsedKey::Tap(Key::Unicode(c))),
        _ => None,
    }
}

/// Hold the modifiers, click each tap, release modifiers in reverse. A
/// modifier-only chord (e.g. "press windows") presses and releases just the
/// modifiers, which is how the Start key works.
fn press_chord(chord: &Chord) -> Result<()> {
    use enigo::{Direction, Enigo, Keyboard, Settings};
    let mut enigo = Enigo::new(&Settings::default()).context("initializing keystroke backend")?;
    for &m in &chord.modifiers {
        enigo.key(m, Direction::Press)?;
    }
    let taps: Result<()> = chord.taps.iter().try_for_each(|&k| {
        enigo.key(k, Direction::Click)?;
        Ok(())
    });
    for &m in chord.modifiers.iter().rev() {
        // Best-effort unwind: modifiers must not stay held after a tap error.
        let _ = enigo.key(m, Direction::Release);
    }
    taps
}

#[cfg(windows)]
mod platform {
    use anyhow::{Result, bail};
    use windows::Win32::Foundation::{BOOL, HWND, LPARAM};
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowTextW, IsIconic, IsWindowVisible, SW_RESTORE, SW_SHOWNORMAL,
        SetForegroundWindow, ShowWindow,
    };
    use windows::core::PCWSTR;

    /// UTF-16, NUL-terminated.
    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// Launch via the shell "open" verb, the same resolution the Run dialog
    /// uses: app names on PATH, documents, and URLs all work.
    pub(super) fn launch(target: &str) -> Result<()> {
        let verb = wide("open");
        let file = wide(target);
        let instance = unsafe {
            ShellExecuteW(
                HWND::default(),
                PCWSTR(verb.as_ptr()),
                PCWSTR(file.as_ptr()),
                PCWSTR::null(),
                PCWSTR::null(),
                SW_SHOWNORMAL,
            )
        };
        // ShellExecuteW's contract: a value greater than 32 is success.
        let code = instance.0 as isize;
        if code <= 32 {
            bail!("shell launch failed (ShellExecuteW code {code})");
        }
        Ok(())
    }

    /// State shared with the EnumWindows callback through its LPARAM.
    struct TitleSearch {
        query_lower: String,
        found: Option<HWND>,
    }

    pub(super) fn focus_window(query: &str) -> Result<()> {
        let mut search = TitleSearch {
            query_lower: query.to_lowercase(),
            found: None,
        };
        // EnumWindows reports an error whenever the callback stops the walk
        // early (our "found it" signal), so its result is deliberately
        // ignored and `found` is the source of truth.
        let _ = unsafe {
            EnumWindows(
                Some(match_title),
                LPARAM(std::ptr::from_mut(&mut search) as isize),
            )
        };
        let Some(hwnd) = search.found else {
            bail!("no visible window title matches the spoken name");
        };
        unsafe {
            if IsIconic(hwnd).as_bool() {
                // Advisory only; SetForegroundWindow below is the real check.
                let _ = ShowWindow(hwnd, SW_RESTORE);
            }
            if !SetForegroundWindow(hwnd).as_bool() {
                bail!("Windows refused to bring the target window forward");
            }
        }
        Ok(())
    }

    unsafe extern "system" fn match_title(hwnd: HWND, lparam: LPARAM) -> BOOL {
        const CONTINUE: BOOL = BOOL(1);
        const STOP: BOOL = BOOL(0);
        // SAFETY: lparam is the TitleSearch owned by focus_window, alive for
        // the whole EnumWindows call.
        let search = unsafe { &mut *(lparam.0 as *mut TitleSearch) };
        if !unsafe { IsWindowVisible(hwnd) }.as_bool() {
            return CONTINUE;
        }
        let mut title = [0u16; 512];
        let len = unsafe { GetWindowTextW(hwnd, &mut title) };
        if len <= 0 {
            return CONTINUE;
        }
        let title = String::from_utf16_lossy(&title[..len as usize]).to_lowercase();
        if title.contains(&search.query_lower) {
            search.found = Some(hwnd);
            return STOP;
        }
        CONTINUE
    }
}

#[cfg(not(windows))]
mod platform {
    use anyhow::{Context, Result, bail};

    #[cfg(target_os = "macos")]
    const OPENER: &str = "open";
    #[cfg(not(target_os = "macos"))]
    const OPENER: &str = "xdg-open";

    /// Best effort: hand the target to the desktop opener and detach.
    pub(super) fn launch(target: &str) -> Result<()> {
        std::process::Command::new(OPENER)
            .arg(target)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .with_context(|| format!("spawning {OPENER}"))?;
        Ok(())
    }

    /// Wayland window control needs per-compositor support; no native focus
    /// off Windows yet.
    pub(super) fn focus_window(_query: &str) -> Result<()> {
        tracing::warn!("window focus by voice is not supported on this platform yet");
        bail!("window focus is not supported on this platform yet");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use enigo::Key;

    fn taps(chord: &Chord) -> &[Key] {
        &chord.taps
    }

    #[test]
    fn chord_splits_modifiers_and_taps() {
        let chord = parse_chord("control shift p").expect("parses");
        assert_eq!(chord.modifiers, vec![Key::Control, Key::Shift]);
        assert_eq!(taps(&chord), [Key::Unicode('p')]);
    }

    #[test]
    fn plus_separated_and_mixed_case_names_parse() {
        let chord = parse_chord("Ctrl+Enter").expect("parses");
        assert_eq!(chord.modifiers, vec![Key::Control]);
        assert_eq!(taps(&chord), [Key::Return]);
    }

    #[test]
    fn named_and_function_keys_parse() {
        assert_eq!(taps(&parse_chord("escape").expect("esc")), [Key::Escape]);
        assert_eq!(taps(&parse_chord("f5").expect("f5")), [Key::F5]);
        assert_eq!(
            taps(&parse_chord("pagedown").expect("pagedown")),
            [Key::PageDown]
        );
    }

    #[test]
    fn modifier_only_chord_is_allowed() {
        let chord = parse_chord("windows").expect("parses");
        assert_eq!(chord.modifiers, vec![Key::Meta]);
        assert!(chord.taps.is_empty());
    }

    #[test]
    fn unknown_or_empty_specs_are_rejected() {
        assert!(parse_chord("frobnicate").is_err());
        assert!(parse_chord("").is_err());
        assert!(parse_chord("f13").is_err());
        assert!(parse_chord("+ +").is_err());
    }
}
