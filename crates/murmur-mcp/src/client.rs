//! MCP client: discovers and calls tools on external stdio MCP servers, so the
//! existing MCP server ecosystem becomes Murmur's voice-command action surface.
//!
//! Security model:
//! - Servers are default-denied: [`ActionBackend`] only connects to names on
//!   its allowlist, so a server added to the user's config (or planted there)
//!   gets no connection until explicitly allowed.
//! - Tool names, descriptions, and annotations are UNTRUSTED input. Real 2025
//!   tool-poisoning attacks hide instructions inside tool descriptions; this
//!   module carries them as opaque display/routing data and never interprets
//!   them as instructions. Annotation hints only ever pick a [`RiskTier`],
//!   which the permission layer gates further.
//! - This module does not decide whether a tool may run. The caller must put
//!   every invocation through `PermissionStore::decision_for` and collect a
//!   physical (keyboard or mouse, never voice) confirmation for Destructive
//!   tools before [`ActionBackend::call_tool`].

use std::collections::{BTreeMap, HashMap, HashSet};

use anyhow::{Context, Result, anyhow, bail};
use murmur_core::command::{RiskTier, Tool};
use rmcp::{
    RoleClient, ServiceExt,
    model::{CallToolRequestParams, CallToolResult, Tool as McpTool, ToolAnnotations},
    service::RunningService,
    transport::{ConfigureCommandExt, TokioChildProcess},
};
use serde::Deserialize;
use serde_json::Value;

/// One stdio MCP server from an `mcpServers` config block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerConfig {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
}

/// The Cursor / Claude Desktop config shape that `install.rs` writes:
/// `{ "mcpServers": { "<name>": { "command", "args", "env", "type" } } }`.
#[derive(Deserialize)]
struct McpServersFile {
    #[serde(default, rename = "mcpServers")]
    mcp_servers: BTreeMap<String, RawServer>,
}

#[derive(Deserialize)]
struct RawServer {
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    /// Transport type; Cursor and Claude Desktop omit it for stdio entries.
    #[serde(default, rename = "type")]
    transport: Option<String>,
}

/// Parse an `mcpServers` JSON config into stdio server configs, sorted by name.
///
/// Non-stdio entries (an explicit non-`"stdio"` `type`, or no `command`, as in
/// URL-based remote servers) are skipped with a warning rather than an error,
/// so one unsupported entry doesn't hide the rest of the user's servers.
/// Malformed JSON is an error.
pub fn parse_mcp_servers(json: &str) -> Result<Vec<ServerConfig>> {
    let file: McpServersFile =
        serde_json::from_str(json).context("mcpServers config is not valid JSON")?;
    let mut servers = Vec::new();
    for (name, raw) in file.mcp_servers {
        if raw.transport.as_deref().is_some_and(|t| t != "stdio") {
            tracing::warn!(server = %name, transport = ?raw.transport, "skipping non-stdio MCP server");
            continue;
        }
        let Some(command) = raw.command.filter(|c| !c.is_empty()) else {
            tracing::warn!(server = %name, "skipping MCP server entry without a command");
            continue;
        };
        servers.push(ServerConfig {
            name,
            command,
            args: raw.args,
            env: raw.env,
        });
    }
    Ok(servers)
}

/// Map MCP tool annotations to Murmur's intrinsic risk tier.
///
/// Annotations are unverified hints from an untrusted server, so the mapping
/// is conservative: `destructiveHint` wins over `readOnlyHint` when both are
/// set (a contradictory server gets the stricter gate), and an unannotated
/// tool is Mutating, never ReadOnly.
pub fn risk_from_annotations(annotations: Option<&ToolAnnotations>) -> RiskTier {
    match annotations {
        Some(a) if a.destructive_hint == Some(true) => RiskTier::Destructive,
        Some(a) if a.read_only_hint == Some(true) => RiskTier::ReadOnly,
        _ => RiskTier::Mutating,
    }
}

/// Namespace a tool name with its server so tools from different servers
/// cannot collide: `commit` from server `git` becomes `git/commit`.
pub fn namespaced_tool_name(server: &str, tool: &str) -> String {
    format!("{server}/{tool}")
}

fn to_core_tool(server: &str, tool: &McpTool) -> Tool {
    Tool {
        name: namespaced_tool_name(server, &tool.name),
        // Untrusted text from the server: display/routing data only.
        description: tool.description.as_deref().unwrap_or_default().to_string(),
        input_schema: Value::Object(tool.input_schema.as_ref().clone()),
        risk: risk_from_annotations(tool.annotations.as_ref()),
    }
}

type Connection = RunningService<RoleClient, ()>;

/// Async action backend over one or more connected stdio MCP servers.
///
/// Connecting is allowlist-gated (default-deny). Executing is NOT permission
/// gated here: the caller owns the `PermissionStore` check and the physical
/// confirmation for Destructive tools before every [`Self::call_tool`].
pub struct ActionBackend {
    allowed: HashSet<String>,
    connections: HashMap<String, Connection>,
}

impl ActionBackend {
    /// Create a backend that may only connect to the given server names.
    pub fn new(allowed_servers: impl IntoIterator<Item = String>) -> Self {
        Self {
            allowed: allowed_servers.into_iter().collect(),
            connections: HashMap::new(),
        }
    }

    /// Whether a server name is on the allowlist.
    pub fn is_allowed(&self, server: &str) -> bool {
        self.allowed.contains(server)
    }

    fn ensure_allowed(&self, server: &str) -> Result<()> {
        if self.is_allowed(server) {
            return Ok(());
        }
        bail!("MCP server '{server}' is not on the allowlist (default-deny); refusing to connect");
    }

    /// Spawn an allowlisted stdio server as a child process and complete the
    /// MCP handshake. A server not on the allowlist is refused before any
    /// process is spawned.
    pub async fn connect(&mut self, cfg: &ServerConfig) -> Result<()> {
        self.ensure_allowed(&cfg.name)?;
        let transport =
            TokioChildProcess::new(tokio::process::Command::new(&cfg.command).configure(|cmd| {
                cmd.args(&cfg.args);
                cmd.envs(&cfg.env);
            }))
            .with_context(|| format!("spawning MCP server '{}' ({})", cfg.name, cfg.command))?;
        let service = ()
            .serve(transport)
            .await
            .with_context(|| format!("MCP handshake with server '{}' failed", cfg.name))?;
        self.attach(&cfg.name, service)
    }

    /// Register an already-running client connection under a server name,
    /// with the same allowlist gate as [`Self::connect`]. Replaces an
    /// existing connection with the same name (reconnect).
    pub fn attach(&mut self, server: &str, service: Connection) -> Result<()> {
        self.ensure_allowed(server)?;
        if self
            .connections
            .insert(server.to_string(), service)
            .is_some()
        {
            tracing::info!(server, "replaced existing MCP server connection");
        } else {
            tracing::info!(server, "connected MCP server");
        }
        Ok(())
    }

    /// Discover every tool on every connected server as a core [`Tool`],
    /// names namespaced `server/tool`.
    ///
    /// Descriptions and annotations come from untrusted servers (see module
    /// docs): treat them as data, and gate execution through the caller's
    /// `PermissionStore`.
    pub async fn list_tools(&self) -> Result<Vec<Tool>> {
        let mut tools = Vec::new();
        for (server, conn) in &self.connections {
            let listed = conn
                .list_all_tools()
                .await
                .with_context(|| format!("listing tools on MCP server '{server}'"))?;
            tools.extend(listed.iter().map(|t| to_core_tool(server, t)));
        }
        Ok(tools)
    }

    /// Invoke `tool_name` (the server-local name, without the namespace
    /// prefix) on a connected server with a JSON-object argument.
    ///
    /// This executes unconditionally: the caller must already have resolved
    /// the permission decision (`PermissionStore::decision_for`) and collected
    /// a physical confirmation for Destructive tools.
    pub async fn call_tool(
        &self,
        server: &str,
        tool_name: &str,
        args: Value,
    ) -> Result<CallToolResult> {
        let conn = self
            .connections
            .get(server)
            .ok_or_else(|| anyhow!("MCP server '{server}' is not connected"))?;
        let arguments = match args {
            Value::Object(map) => Some(map),
            Value::Null => None,
            other => bail!("tool arguments must be a JSON object, got: {other}"),
        };
        let mut params = CallToolRequestParams::new(tool_name.to_string());
        params.arguments = arguments;
        conn.call_tool(params)
            .await
            .with_context(|| format!("calling tool '{tool_name}' on MCP server '{server}'"))
    }

    /// Shut down every connection, terminating the spawned server processes.
    pub async fn shutdown(mut self) {
        for (server, conn) in self.connections.drain() {
            if let Err(e) = conn.cancel().await {
                tracing::warn!(server = %server, error = %e, "MCP server connection shutdown failed");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const MULTI_SERVER: &str = r#"{
        "mcpServers": {
            "git": { "command": "uvx", "args": ["mcp-server-git"], "type": "stdio" },
            "files": {
                "command": "npx",
                "args": ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"],
                "env": { "LOG_LEVEL": "info" }
            },
            "remote": { "type": "http", "url": "https://example.com/mcp" }
        }
    }"#;

    #[test]
    fn parses_stdio_servers_and_skips_non_stdio() {
        let servers = parse_mcp_servers(MULTI_SERVER).expect("valid config");
        assert_eq!(servers.len(), 2, "the http entry must be skipped");
        assert_eq!(servers[0].name, "files");
        assert_eq!(servers[0].command, "npx");
        assert_eq!(
            servers[0].args,
            vec!["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]
        );
        assert_eq!(
            servers[0].env.get("LOG_LEVEL").map(String::as_str),
            Some("info")
        );
        assert_eq!(servers[1].name, "git");
        assert_eq!(servers[1].args, vec!["mcp-server-git"]);
        assert!(servers[1].env.is_empty());
    }

    #[test]
    fn entry_without_command_is_skipped() {
        let json = r#"{ "mcpServers": { "broken": { "args": ["x"] } } }"#;
        assert!(parse_mcp_servers(json).expect("valid json").is_empty());
    }

    #[test]
    fn malformed_json_is_an_error_not_a_panic() {
        assert!(parse_mcp_servers("{ not json").is_err());
        assert!(parse_mcp_servers(r#"{"mcpServers": []}"#).is_err());
    }

    #[test]
    fn empty_or_absent_mcp_servers_yields_no_servers() {
        assert!(parse_mcp_servers("{}").expect("valid").is_empty());
        assert!(
            parse_mcp_servers(r#"{"mcpServers":{}}"#)
                .expect("valid")
                .is_empty()
        );
    }

    #[test]
    fn risk_mapping_follows_annotation_hints() {
        let read_only = ToolAnnotations::new().read_only(true);
        let destructive = ToolAnnotations::new().destructive(true);
        assert_eq!(risk_from_annotations(Some(&read_only)), RiskTier::ReadOnly);
        assert_eq!(
            risk_from_annotations(Some(&destructive)),
            RiskTier::Destructive
        );
        assert_eq!(
            risk_from_annotations(Some(&ToolAnnotations::new())),
            RiskTier::Mutating
        );
        assert_eq!(risk_from_annotations(None), RiskTier::Mutating);
    }

    #[test]
    fn contradictory_hints_take_the_stricter_tier() {
        // Hints are untrusted; a server claiming both readOnly and destructive
        // gets the stricter gate.
        let both = ToolAnnotations::new().read_only(true).destructive(true);
        assert_eq!(risk_from_annotations(Some(&both)), RiskTier::Destructive);
    }

    #[test]
    fn tools_are_namespaced_by_server() {
        assert_eq!(namespaced_tool_name("git", "commit"), "git/commit");
    }

    #[test]
    fn mcp_tool_converts_to_core_tool() {
        // Deserialize from the wire shape so camelCase hint names are covered.
        let mcp: McpTool = serde_json::from_value(json!({
            "name": "commit",
            "description": "Record changes to the repository",
            "inputSchema": {
                "type": "object",
                "properties": { "message": { "type": "string" } }
            },
            "annotations": { "readOnlyHint": true }
        }))
        .expect("valid tool json");
        let tool = to_core_tool("git", &mcp);
        assert_eq!(tool.name, "git/commit");
        assert_eq!(tool.description, "Record changes to the repository");
        assert_eq!(tool.risk, RiskTier::ReadOnly);
        assert_eq!(tool.input_schema["properties"]["message"]["type"], "string");
    }

    #[test]
    fn allowlist_defaults_to_deny() {
        let backend = ActionBackend::new(std::iter::empty());
        assert!(!backend.is_allowed("anything"));
        let backend = ActionBackend::new(["git".to_string()]);
        assert!(backend.is_allowed("git"));
        assert!(!backend.is_allowed("shell"));
    }
}
