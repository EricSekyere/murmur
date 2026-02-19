use anyhow::Result;
use enigo::{Enigo, Keyboard, Settings};

/// Simulates keystrokes to type text into the currently focused application.
pub struct KeyboardOutput {
    enigo: Enigo,
}

impl KeyboardOutput {
    pub fn new() -> Result<Self> {
        let enigo = Enigo::new(&Settings::default())?;
        Ok(Self { enigo })
    }

    /// Type the given text character by character.
    pub fn type_text(&mut self, text: &str) -> Result<()> {
        tracing::debug!("Typing {} characters via keyboard simulation", text.len());
        self.enigo.text(text)?;
        Ok(())
    }
}
