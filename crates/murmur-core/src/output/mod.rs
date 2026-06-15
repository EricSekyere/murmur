pub mod clipboard;
#[cfg(feature = "keyboard")]
pub mod keyboard;
#[cfg(feature = "keyboard")]
pub mod paste;
pub mod stdout;

use serde::{Deserialize, Serialize};

#[cfg(windows)]
fn should_prefer_clipboard_paste_for_foreground() -> bool {
    let Some(info) = keyboard::foreground_window_info_public() else {
        return false;
    };

    let process = info.process_name.unwrap_or_default().to_ascii_lowercase();
    let class_name = info.class_name.to_ascii_lowercase();
    let is_terminal = is_terminal_window(&process, &class_name);

    if is_terminal {
        tracing::info!(
            process = process,
            class = info.class_name,
            "Foreground is a terminal; using clipboard+paste output"
        );
    }
    is_terminal
}

/// Whether a window (by lowercase process name and class) is a terminal
/// emulator that should receive output via clipboard+paste instead of direct
/// keystroke typing.
///
/// Matches only genuine terminals. It deliberately does NOT match on window
/// title (a browser tab titled "PowerShell tutorial" is not a terminal) or on
/// the `chrome_widgetwin_1` class (shared by every Chromium and Electron app:
/// Chrome, Edge, VS Code, Slack, ...). Those false positives previously forced
/// normal apps through the clipboard, where a paste timing race could leak the
/// user's previous clipboard content into the target window.
#[cfg(windows)]
fn is_terminal_window(process: &str, class_name: &str) -> bool {
    let process_match = matches!(
        process,
        "warp.exe"
            | "windowsterminal.exe"
            | "wt.exe"
            | "cmd.exe"
            | "powershell.exe"
            | "pwsh.exe"
            | "conhost.exe"
            | "wezterm-gui.exe"
            | "alacritty.exe"
            | "mintty.exe"
            | "putty.exe"
    );

    let class_match = class_name.contains("cascadiahostingwindowclass")
        || class_name.contains("consolewindowclass");

    process_match || class_match
}

/// Output strategy for transcribed text.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputMode {
    /// Automatically choose the best method with fallback chain:
    /// keyboard → clipboard+paste → clipboard-only.
    #[default]
    Auto,
    /// Copy text to clipboard. User pastes manually.
    Clipboard,
    /// Simulate keystrokes to type text into the focused application.
    Keyboard,
    /// Copy text to clipboard and simulate Ctrl+V / Cmd+V to paste.
    /// More reliable than keyboard simulation in terminals and elevated windows.
    ClipboardPaste,
    /// Write text to stdout (for CLI piping).
    Stdout,
}

/// Execute the output strategy, including fallback logic for `Auto` mode.
///
/// This centralizes the fallback chain so both the Tauri app and CLI benefit.
#[cfg(feature = "keyboard")]
pub fn dispatch_output(text: &str, mode: OutputMode) -> anyhow::Result<()> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(());
    }

    tracing::info!(
        mode = ?mode,
        chars = trimmed.len(),
        "Outputting transcribed text"
    );

    match mode {
        // Auto and Keyboard share the same strategy: type directly with
        // SendInput Unicode (which never touches the clipboard) for normal
        // windows, and only fall back to clipboard+paste for terminal-like
        // windows where direct typing is unreliable. Typing directly avoids
        // the clipboard-update race that could otherwise paste the user's
        // previous clipboard content into the target app.
        OutputMode::Auto | OutputMode::Keyboard => {
            let text_with_space = format!("{} ", trimmed);

            #[cfg(windows)]
            if should_prefer_clipboard_paste_for_foreground() {
                if let Err(e) = paste::ClipboardPasteOutput::new().paste_text(&text_with_space) {
                    tracing::warn!(
                        "Terminal-target clipboard+paste failed, copying to clipboard: {}",
                        e
                    );
                    clipboard::ClipboardOutput::new()?.copy(trimmed)?;
                }
                return Ok(());
            }

            let kb_result =
                keyboard::KeyboardOutput::new().and_then(|mut kb| kb.type_text(&text_with_space));

            if let Err(e) = kb_result {
                tracing::warn!(
                    "Keyboard output failed, falling back to clipboard+paste: {}",
                    e
                );
                let paste_result = paste::ClipboardPasteOutput::new().paste_text(&text_with_space);

                if let Err(e2) = paste_result {
                    tracing::warn!(
                        "Clipboard+paste fallback also failed, copying to clipboard: {}",
                        e2
                    );
                    clipboard::ClipboardOutput::new()?.copy(trimmed)?;
                }
            }
        }
        OutputMode::ClipboardPaste => {
            let text_with_space = format!("{} ", trimmed);
            if let Err(e) = paste::ClipboardPasteOutput::new().paste_text(&text_with_space) {
                tracing::warn!("Clipboard+paste failed, copying to clipboard: {}", e);
                clipboard::ClipboardOutput::new()?.copy(trimmed)?;
            }
        }
        OutputMode::Clipboard => {
            clipboard::ClipboardOutput::new()?.copy(trimmed)?;
        }
        OutputMode::Stdout => {
            stdout::StdoutOutput::new().write(trimmed)?;
        }
    }

    Ok(())
}

#[cfg(all(test, windows))]
mod tests {
    use super::is_terminal_window;

    #[test]
    fn browsers_and_electron_apps_are_not_terminals() {
        // All Chromium/Electron apps share the chrome_widgetwin_1 class.
        // None of these should be routed through clipboard+paste.
        assert!(!is_terminal_window("chrome.exe", "chrome_widgetwin_1"));
        assert!(!is_terminal_window("msedge.exe", "chrome_widgetwin_1"));
        assert!(!is_terminal_window("code.exe", "chrome_widgetwin_1"));
        assert!(!is_terminal_window("slack.exe", "chrome_widgetwin_1"));
        assert!(!is_terminal_window("discord.exe", "chrome_widgetwin_1"));
        assert!(!is_terminal_window("notepad.exe", "notepad"));
        assert!(!is_terminal_window("winword.exe", "opusapp"));
    }

    #[test]
    fn titles_do_not_trigger_terminal_detection() {
        // A browser tab about PowerShell is not a terminal.
        assert!(!is_terminal_window("chrome.exe", "chrome_widgetwin_1"));
    }

    #[test]
    fn genuine_terminals_are_detected() {
        assert!(is_terminal_window("warp.exe", "window class"));
        assert!(is_terminal_window(
            "windowsterminal.exe",
            "cascadiahostingwindowclass"
        ));
        assert!(is_terminal_window("cmd.exe", "consolewindowclass"));
        assert!(is_terminal_window("powershell.exe", "consolewindowclass"));
        assert!(is_terminal_window("pwsh.exe", "consolewindowclass"));
        assert!(is_terminal_window("wezterm-gui.exe", "window class"));
        assert!(is_terminal_window("alacritty.exe", "alacritty"));
    }
}
