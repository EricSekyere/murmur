use anyhow::Result;
use arboard::Clipboard;

/// Outputs text via the system clipboard.
pub struct ClipboardOutput {
    clipboard: Clipboard,
}

impl ClipboardOutput {
    pub fn new() -> Result<Self> {
        let clipboard = Clipboard::new()?;
        Ok(Self { clipboard })
    }

    pub fn copy(&mut self, text: &str) -> Result<()> {
        tracing::debug!("Copying {} characters to clipboard", text.len());
        self.clipboard.set_text(text)?;
        Ok(())
    }
}

/// Current clipboard text; errors when the clipboard is unavailable or holds
/// no text (e.g. an image). The content is never logged.
pub fn read() -> Result<String> {
    let text = Clipboard::new()?.get_text()?;
    tracing::debug!("Read {} characters from clipboard", text.len());
    Ok(text)
}
