use anyhow::{Context, Result};
use arboard::Clipboard;
use enigo::{Enigo, Key, Keyboard, Settings};

/// Simulates keystrokes to type text into the currently focused application.
///
/// Uses clipboard + Ctrl+V for reliable cross-terminal support, then restores
/// the original clipboard contents so the user's clipboard isn't clobbered.
pub struct KeyboardOutput {
    enigo: Enigo,
}

impl KeyboardOutput {
    pub fn new() -> Result<Self> {
        let enigo = Enigo::new(&Settings::default())?;
        Ok(Self { enigo })
    }

    /// Type text into the focused application via clipboard paste.
    pub fn type_text(&mut self, text: &str) -> Result<()> {
        tracing::debug!("Typing {} characters via clipboard paste", text.len());

        let mut clipboard = Clipboard::new().context("Failed to open clipboard")?;

        // Save whatever the user had on the clipboard
        let previous = clipboard.get_text().ok();

        clipboard
            .set_text(text)
            .context("Failed to set clipboard text")?;

        // Small delay to let the clipboard settle
        std::thread::sleep(std::time::Duration::from_millis(30));

        // Simulate Ctrl+V
        self.enigo.key(Key::Control, enigo::Direction::Press)?;
        self.enigo.key(Key::Unicode('v'), enigo::Direction::Press)?;
        self.enigo.key(Key::Unicode('v'), enigo::Direction::Release)?;
        self.enigo.key(Key::Control, enigo::Direction::Release)?;

        // Let the paste complete before restoring
        std::thread::sleep(std::time::Duration::from_millis(50));

        // Restore original clipboard
        if let Some(prev) = previous {
            let _ = clipboard.set_text(&prev);
        }

        Ok(())
    }
}
