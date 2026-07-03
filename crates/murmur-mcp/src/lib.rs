//! Murmur's MCP integration, shared by the CLI (`murmur mcp`) and the desktop
//! app (`murmur-app mcp`, invoked by the in-app "Connect to editor" button).
//!
//! - [`serve`] runs a stdio Model Context Protocol server exposing the local
//!   transcription history to MCP clients (Claude Desktop, Cursor, Claude Code),
//!   including a `wait_for_next_dictation` tool that lets an agent receive the
//!   user's next spoken phrase mid-task (the app owns recording; the server
//!   only observes the history it writes).
//! - [`install`] writes the server into a client's config so setup is one step.
//! - [`ActionBackend`] is the reverse direction: an MCP client that spawns
//!   allowlisted stdio servers and exposes their tools as voice-command
//!   actions.
//!
//! Everything is local: MCP peers are child processes speaking JSON-RPC over
//! stdin/stdout. No network, no telemetry.

mod client;
mod install;
mod server;
mod wait;

pub use client::{
    ActionBackend, ServerConfig, namespaced_tool_name, parse_mcp_servers, risk_from_annotations,
};
pub use install::{ClientKind, ConfiguredClient, InstallReport, install};
pub use server::serve;
