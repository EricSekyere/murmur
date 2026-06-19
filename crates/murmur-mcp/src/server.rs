//! Stdio MCP server exposing Murmur's local transcription history through two
//! read-only tools: `get_recent_transcripts` and `search_transcripts`. The
//! client spawns the host process and speaks JSON-RPC over stdin/stdout, so
//! nothing leaves the machine. The server never mutates the history log.

use anyhow::Result;
use murmur_core::history::{History, HistoryEntry};
use rmcp::{
    ErrorData as McpError, ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, Content, Implementation, ServerCapabilities, ServerInfo},
    schemars, tool, tool_handler, tool_router,
    transport::stdio,
};
use serde::{Deserialize, Serialize};

const DEFAULT_LIMIT: usize = 20;
const MAX_LIMIT: usize = 100;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct RecentRequest {
    /// Maximum number of transcripts to return (default 20, max 100).
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct SearchRequest {
    /// Case-insensitive substring to match against transcript text.
    query: String,
    /// Maximum number of matches to return (default 20, max 100).
    #[serde(default)]
    limit: Option<usize>,
}

/// Stable JSON shape returned per transcript, decoupled from the on-disk
/// [`HistoryEntry`] so storage changes don't silently alter the tool contract.
#[derive(Serialize)]
struct TranscriptOut {
    text: String,
    timestamp_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    app: Option<String>,
}

impl From<HistoryEntry> for TranscriptOut {
    fn from(e: HistoryEntry) -> Self {
        Self {
            text: e.text,
            timestamp_ms: e.timestamp_ms,
            app: e.app,
        }
    }
}

#[derive(Clone)]
struct MurmurMcp {
    tool_router: ToolRouter<MurmurMcp>,
}

#[tool_router]
impl MurmurMcp {
    fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        description = "List the most recent phrases dictated through Murmur (voice-to-text), newest first."
    )]
    fn get_recent_transcripts(
        &self,
        Parameters(req): Parameters<RecentRequest>,
    ) -> Result<CallToolResult, McpError> {
        history_json("", req.limit)
    }

    #[tool(
        description = "Search Murmur's voice transcription history for a case-insensitive substring, newest first."
    )]
    fn search_transcripts(
        &self,
        Parameters(req): Parameters<SearchRequest>,
    ) -> Result<CallToolResult, McpError> {
        history_json(&req.query, req.limit)
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for MurmurMcp {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions(
                "Murmur exposes your local voice-to-text history. Call get_recent_transcripts \
                 to see what was just dictated, or search_transcripts to find a past phrase.",
            );
        info.server_info = Implementation::new("murmur", env!("CARGO_PKG_VERSION"));
        info
    }
}

/// Load the history log and return up to `limit` (clamped) entries matching
/// `query` as a pretty JSON array.
fn history_json(query: &str, limit: Option<usize>) -> Result<CallToolResult, McpError> {
    let limit = limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let path = History::default_path()
        .map_err(|e| McpError::internal_error(format!("history path: {e}"), None))?;
    let entries: Vec<TranscriptOut> = History::load(&path)
        .search(query, limit)
        .into_iter()
        .map(TranscriptOut::from)
        .collect();
    let json = serde_json::to_string_pretty(&entries)
        .map_err(|e| McpError::internal_error(format!("serialize: {e}"), None))?;
    Ok(CallToolResult::success(vec![Content::text(json)]))
}

/// Serve the MCP protocol over stdio until the client disconnects. The protocol
/// owns stdout, so the host must keep diagnostics on stderr.
pub async fn serve() -> Result<()> {
    let service = MurmurMcp::new()
        .serve(stdio())
        .await
        .map_err(|e| anyhow::anyhow!("Failed to start MCP stdio server: {e}"))?;
    service
        .waiting()
        .await
        .map_err(|e| anyhow::anyhow!("MCP server terminated abnormally: {e}"))?;
    Ok(())
}
