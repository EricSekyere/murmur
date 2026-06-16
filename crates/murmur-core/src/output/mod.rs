pub mod clipboard;
#[cfg(feature = "keyboard")]
pub mod keyboard;
#[cfg(feature = "keyboard")]
pub mod paste;
pub mod stdout;

use serde::{Deserialize, Serialize};

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
        // Auto and Keyboard both type directly with SendInput Unicode, which
        // never touches the clipboard. This is the only way to guarantee the
        // user's clipboard can never leak into the target: routing terminals
        // through clipboard+paste always left a window where a slow app could
        // read and paste the previous clipboard. Modern terminals (Warp,
        // Windows Terminal, ...) accept direct Unicode input fine. Clipboard
        // paste is now only a fallback when direct typing fails (e.g. an
        // elevated window), and a user who genuinely needs it can still select
        // the ClipboardPaste mode, per app via App Profiles if they like.
        OutputMode::Auto | OutputMode::Keyboard => {
            let text_with_space = format!("{} ", trimmed);

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
