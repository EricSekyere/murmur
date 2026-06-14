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
    let title = info.title.to_ascii_lowercase();
    let class_name = info.class_name.to_ascii_lowercase();

    let process_match = matches!(
        process.as_str(),
        "warp.exe"
            | "windows terminal.exe"
            | "windowsterminal.exe"
            | "wezterm-gui.exe"
            | "alacritty.exe"
            | "claude.exe"
            | "codex.exe"
    );

    let title_match = [
        "warp",
        "claude",
        "codex",
        "terminal",
        "powershell",
        "cmd",
        "wezterm",
    ]
    .iter()
    .any(|needle| title.contains(needle));

    let class_match = [
        "cascadiahostingwindowclass",
        "pty",
        "terminal",
        "chrome_widgetwin_1",
    ]
    .iter()
    .any(|needle| class_name.contains(needle));

    if process_match || title_match || class_match {
        tracing::info!(
            process = process,
            title = info.title,
            class = info.class_name,
            "Foreground target looks terminal-like; preferring clipboard+paste output"
        );
        true
    } else {
        false
    }
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
