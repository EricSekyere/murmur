//! Stdio MCP server exposing Murmur's local transcription history through
//! read-only tools: `get_recent_transcripts`, `search_transcripts`, and
//! `wait_for_next_dictation`. The client spawns the host process and speaks
//! JSON-RPC over stdin/stdout, so nothing leaves the machine. The server never
//! mutates the history log and never starts a recording: the app owns capture
//! (the user's push-to-talk hotkey); this server only observes the transcripts
//! the app delivers.

use anyhow::Result;
use murmur_core::config::Settings;
use murmur_core::history::{History, HistoryEntry};
use rmcp::{
    ErrorData as McpError, ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, Content, Implementation, ServerCapabilities, ServerInfo},
    schemars, tool, tool_handler, tool_router,
    transport::stdio,
};
use serde::{Deserialize, Serialize};

use crate::wait;

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

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct WaitRequest {
    /// Seconds to wait for the next dictation before giving up (default 30, max 300).
    #[serde(default)]
    timeout_secs: Option<u64>,
}

/// Result shape for `wait_for_next_dictation`, tagged so an agent can branch
/// on `status` without parsing prose.
#[derive(Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum WaitOutcome {
    Received {
        text: String,
        timestamp_ms: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        app: Option<String>,
    },
    TimedOut {
        waited_secs: u64,
        message: &'static str,
    },
    HistoryDisabled {
        message: &'static str,
    },
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
    async fn get_recent_transcripts(
        &self,
        Parameters(req): Parameters<RecentRequest>,
    ) -> Result<CallToolResult, McpError> {
        history_json_blocking(String::new(), req.limit).await
    }

    #[tool(
        description = "Search Murmur's voice transcription history for a case-insensitive substring, newest first."
    )]
    async fn search_transcripts(
        &self,
        Parameters(req): Parameters<SearchRequest>,
    ) -> Result<CallToolResult, McpError> {
        history_json_blocking(req.query, req.limit).await
    }

    #[tool(
        description = "Wait for the next phrase the user dictates through Murmur and return its text. \
                       Read-only: this never starts recording. The user must trigger capture themselves \
                       with Murmur's push-to-talk hotkey; this tool only watches for the resulting \
                       transcript. Use it to ask the user a question and receive their spoken \
                       answer mid-task. Returns status 'received' with the transcript, or \
                       'timed_out' if the user did not dictate within timeout_secs."
    )]
    async fn wait_for_next_dictation(
        &self,
        Parameters(req): Parameters<WaitRequest>,
    ) -> Result<CallToolResult, McpError> {
        if !history_enabled_blocking().await {
            return outcome_json(&WaitOutcome::HistoryDisabled {
                message: "Murmur's save-history setting is off, so new dictations cannot be \
                          observed. Ask the user to enable saving history in Murmur settings.",
            });
        }
        let waited_secs = wait::clamp_timeout(req.timeout_secs);
        let path = History::default_path()
            .map_err(|e| McpError::internal_error(format!("history path: {e}"), None))?;
        // Newest entry only: the baseline and each poll just need the head of
        // the newest-first log. Reads run on the blocking pool: this loop
        // repeats every 500ms and must not stall the reactor's other requests.
        let load = || {
            let path = path.clone();
            async move {
                tokio::task::spawn_blocking(move || History::load_readonly(&path).search("", 1))
                    .await
                    .unwrap_or_default()
            }
        };
        let baseline_ms = load().await.first().map(|e| e.timestamp_ms);
        tracing::debug!(waited_secs, ?baseline_ms, "waiting for next dictation");
        let found =
            wait::wait_for_new_entry(baseline_ms, wait::polls_for(waited_secs), load, || {
                tokio::time::sleep(wait::POLL_INTERVAL)
            })
            .await;
        let outcome = match found {
            Some(entry) => {
                tracing::debug!(timestamp_ms = entry.timestamp_ms, "new dictation observed");
                WaitOutcome::Received {
                    text: entry.text,
                    timestamp_ms: entry.timestamp_ms,
                    app: entry.app,
                }
            }
            None => WaitOutcome::TimedOut {
                waited_secs,
                message: "No new dictation appeared within the timeout. Ask the user to speak \
                          using Murmur's hotkey, then call this tool again.",
            },
        };
        outcome_json(&outcome)
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for MurmurMcp {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions(
                "Murmur exposes your local voice-to-text history. Call get_recent_transcripts \
                 to see what was just dictated, search_transcripts to find a past phrase, or \
                 wait_for_next_dictation to receive the next phrase the user speaks (the user \
                 starts recording with their own hotkey; the server never records).",
            );
        info.server_info = Implementation::new("murmur", env!("CARGO_PKG_VERSION"));
        info
    }
}

/// [`history_json`] on the blocking pool: the read is sync file I/O and must
/// stay off the reactor.
async fn history_json_blocking(
    query: String,
    limit: Option<usize>,
) -> Result<CallToolResult, McpError> {
    tokio::task::spawn_blocking(move || history_json(&query, limit))
        .await
        .map_err(|e| McpError::internal_error(format!("history read task: {e}"), None))?
}

/// Load the history log and return up to `limit` (clamped) entries matching
/// `query` as a pretty JSON array. Sync file I/O: call via
/// [`history_json_blocking`] from async context.
fn history_json(query: &str, limit: Option<usize>) -> Result<CallToolResult, McpError> {
    // Respect the history opt-out: expose nothing when the user has disabled
    // saving history, even if a log file remains on disk from before.
    if !history_enabled() {
        return Ok(CallToolResult::success(vec![Content::text("[]")]));
    }
    let limit = limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let path = History::default_path()
        .map_err(|e| McpError::internal_error(format!("history path: {e}"), None))?;
    let entries: Vec<TranscriptOut> = History::load_readonly(&path)
        .search(query, limit)
        .into_iter()
        .map(TranscriptOut::from)
        .collect();
    let json = serde_json::to_string_pretty(&entries)
        .map_err(|e| McpError::internal_error(format!("serialize: {e}"), None))?;
    Ok(CallToolResult::success(vec![Content::text(json)]))
}

/// Serialize a wait outcome as the tool's pretty-JSON payload.
fn outcome_json(outcome: &WaitOutcome) -> Result<CallToolResult, McpError> {
    let json = serde_json::to_string_pretty(outcome)
        .map_err(|e| McpError::internal_error(format!("serialize: {e}"), None))?;
    Ok(CallToolResult::success(vec![Content::text(json)]))
}

/// Whether the user currently allows storing transcription history. Defaults to
/// enabled if settings can't be read, since the history file is the source of
/// truth and the app purges it when the user opts out. Uses the read-only
/// loader: this server must never write config recovery files the running app
/// could race with.
fn history_enabled() -> bool {
    Settings::default_path()
        .map(|path| Settings::load_readonly(&path).save_history)
        .unwrap_or(true)
}

/// [`history_enabled`] on the blocking pool for async handlers.
async fn history_enabled_blocking() -> bool {
    tokio::task::spawn_blocking(history_enabled)
        .await
        .unwrap_or(true)
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
