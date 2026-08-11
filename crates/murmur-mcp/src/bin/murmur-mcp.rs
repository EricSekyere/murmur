//! Minimal stdio entry point for the Murmur MCP server. End users normally
//! run `murmur mcp` from the CLI; this binary exists so the crate's
//! integration tests can drive the real protocol over a child process, and it
//! doubles as a standalone server for headless setups.
//!
//! Diagnostics go to stderr through a hand-rolled subscriber: stdout is the
//! JSON-RPC channel and must never carry log output, and the crate's
//! dependency set is deliberately kept as-is (no tracing-subscriber here).

use std::fmt::Write as _;
use std::io::Write as _;

use tracing::field::{Field, Visit};

struct StderrSubscriber;

struct MessageVisitor(String);

impl Visit for MessageVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        // Best-effort formatting: a full formatter is not worth a dependency
        // edge for a diagnostics-only sink.
        let _ = write!(self.0, " {}={:?}", field.name(), value);
    }
}

impl tracing::Subscriber for StderrSubscriber {
    fn enabled(&self, _metadata: &tracing::Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }

    fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        let mut line = format!(
            "{} {}:",
            event.metadata().level(),
            event.metadata().target()
        );
        let mut visitor = MessageVisitor(String::new());
        event.record(&mut visitor);
        line.push_str(&visitor.0);
        // Stderr only: stdout belongs to the MCP JSON-RPC stream. A failed
        // diagnostic write is not worth crashing the server over.
        let _ = writeln!(std::io::stderr(), "{line}");
    }

    fn enter(&self, _span: &tracing::span::Id) {}

    fn exit(&self, _span: &tracing::span::Id) {}
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Only wire the sink up when logging was asked for; the common case
    // stays silent and pays nothing per event.
    if std::env::var_os("RUST_LOG").is_some() {
        let _ = tracing::subscriber::set_global_default(StderrSubscriber);
    }
    murmur_core::config::Settings::migrate_from_voitex();
    murmur_mcp::serve().await
}
