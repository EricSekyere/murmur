pub mod clipboard;
#[cfg(feature = "keyboard")]
pub mod keyboard;
pub mod stdout;

/// Output strategy for transcribed text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputMode {
    /// Simulate keystrokes to type text into the focused application.
    Keyboard,
    /// Copy text to clipboard (and optionally paste).
    Clipboard,
    /// Write text to stdout (for CLI piping).
    Stdout,
}
