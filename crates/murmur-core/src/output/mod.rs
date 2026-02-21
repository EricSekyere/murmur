pub mod clipboard;
#[cfg(feature = "keyboard")]
pub mod keyboard;
pub mod stdout;

use serde::{Deserialize, Serialize};

/// Output strategy for transcribed text.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputMode {
    /// Copy text to clipboard and paste (Ctrl+V) into the focused application.
    /// Most reliable method — works with terminals, editors, browsers, etc.
    #[default]
    Clipboard,
    /// Simulate keystrokes to type text into the focused application.
    Keyboard,
    /// Write text to stdout (for CLI piping).
    Stdout,
}
