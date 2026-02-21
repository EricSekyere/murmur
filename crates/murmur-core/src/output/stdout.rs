use anyhow::Result;
use std::io::Write;

/// Outputs text to stdout for CLI piping.
#[derive(Default)]
pub struct StdoutOutput;

impl StdoutOutput {
    pub fn new() -> Self {
        Self
    }

    /// Write text to stdout followed by a newline, flushing immediately.
    pub fn write(&self, text: &str) -> Result<()> {
        let mut stdout = std::io::stdout().lock();
        writeln!(stdout, "{}", text)?;
        stdout.flush()?;
        Ok(())
    }
}
