//! Murmur's MCP integration, shared by the CLI (`murmur mcp`) and the desktop
//! app (`murmur-app mcp`, invoked by the in-app "Connect to editor" button).
//!
//! - [`serve`] runs a stdio Model Context Protocol server exposing the local
//!   transcription history to MCP clients (Claude Desktop, Cursor, Claude Code).
//! - [`install`] writes the server into a client's config so setup is one step.
//!
//! Everything is local: the client spawns the host process and speaks JSON-RPC
//! over stdin/stdout. No network, no telemetry.

mod install;
mod server;

pub use install::{ClientKind, ConfiguredClient, InstallReport, install};
pub use server::serve;
