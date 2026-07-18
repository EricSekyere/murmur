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

/// Whether typed output carries the dictation trailing space.
#[cfg(feature = "keyboard")]
enum Separator {
    /// Append a space so consecutive dictated phrases don't run together.
    TrailingSpace,
    /// Leading + trailing space: splices onto already-typed text after a
    /// junction repair backspaced the previous phrase's mark and space.
    /// A variant is needed because `deliver` trims the input, so a leading
    /// space can never ride in on the text itself.
    Joining,
    /// Type the text exactly as given (e.g. a resolved file path).
    Exact,
}

/// Dictation output: type `text` followed by a separating space, with the
/// `Auto` fallback chain. Centralizes the chain so both app and CLI benefit.
#[cfg(feature = "keyboard")]
pub fn dispatch_output(text: &str, mode: OutputMode) -> anyhow::Result<()> {
    deliver(text, mode, Separator::TrailingSpace)
}

/// Output `text` verbatim, without the dictation trailing space. For
/// command-mode results such as a resolved file path typed at the cursor.
#[cfg(feature = "keyboard")]
pub fn dispatch_verbatim(text: &str, mode: OutputMode) -> anyhow::Result<()> {
    deliver(text, mode, Separator::Exact)
}

/// Junction-repair output: type `" " + text + " "` so the phrase joins onto
/// already-typed text whose terminal mark and space were just backspaced
/// away (see `crate::dictation_junction`).
#[cfg(feature = "keyboard")]
pub fn dispatch_joining(text: &str, mode: OutputMode) -> anyhow::Result<()> {
    deliver(text, mode, Separator::Joining)
}

#[cfg(feature = "keyboard")]
fn deliver(text: &str, mode: OutputMode, separator: Separator) -> anyhow::Result<()> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(());
    }

    tracing::info!(
        mode = ?mode,
        chars = trimmed.len(),
        "Outputting transcribed text"
    );

    // Typed variants may append a separating space for dictation; clipboard
    // and stdout always emit the exact text.
    let typed = match separator {
        Separator::TrailingSpace => format!("{trimmed} "),
        Separator::Joining => format!(" {trimmed} "),
        Separator::Exact => trimmed.to_string(),
    };

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
            let kb_result = keyboard::KeyboardOutput::new().and_then(|mut kb| kb.type_text(&typed));

            if let Err(e) = kb_result {
                tracing::warn!(
                    "Keyboard output failed, falling back to clipboard+paste: {}",
                    e
                );
                let paste_result = paste::ClipboardPasteOutput::new().paste_text(&typed);

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
            if let Err(e) = paste::ClipboardPasteOutput::new().paste_text(&typed) {
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
