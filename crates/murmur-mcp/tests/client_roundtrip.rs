//! End-to-end MCP client round trip over an in-memory duplex transport: an
//! in-process rmcp server exposes one tool, and [`ActionBackend`] lists and
//! calls it through the same code paths a spawned server would use. No child
//! process is involved, so the test is hermetic and fast.

use anyhow::Result;
use murmur_core::command::RiskTier;
use murmur_mcp::{ActionBackend, ServerConfig};
use rmcp::{
    ErrorData as McpError, ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, Content, ServerCapabilities, ServerInfo},
    schemars, tool, tool_handler, tool_router,
};
use serde::Deserialize;
use serde_json::json;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct EchoRequest {
    /// Text to echo back unchanged.
    text: String,
}

#[derive(Clone)]
struct EchoServer {
    tool_router: ToolRouter<EchoServer>,
}

#[tool_router]
impl EchoServer {
    fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }

    #[tool(description = "Echo the input text back")]
    fn echo(&self, Parameters(req): Parameters<EchoRequest>) -> Result<CallToolResult, McpError> {
        Ok(CallToolResult::success(vec![Content::text(req.text)]))
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for EchoServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
    }
}

#[tokio::test]
async fn lists_and_calls_a_tool_over_in_memory_transport() -> Result<()> {
    let (client_io, server_io) = tokio::io::duplex(4096);
    let server = tokio::spawn(async move {
        // serve() completes the handshake; waiting() runs until the client
        // disconnects, which backend.shutdown() triggers below.
        if let Ok(service) = EchoServer::new().serve(server_io).await {
            let _ = service.waiting().await;
        }
    });

    let connection = ().serve(client_io).await?;
    let mut backend = ActionBackend::new(["echo".to_string()]);
    backend.attach("echo", connection)?;

    let tools = backend.list_tools().await?;
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "echo/echo");
    assert_eq!(tools[0].description, "Echo the input text back");
    // The test tool carries no annotations: the conservative default applies.
    assert_eq!(tools[0].risk, RiskTier::Mutating);

    let result = backend
        .call_tool("echo", "echo", json!({ "text": "hello murmur" }))
        .await?;
    let text = result
        .content
        .first()
        .and_then(|c| c.as_text())
        .map(|t| t.text.clone());
    assert_eq!(text.as_deref(), Some("hello murmur"));

    backend.shutdown().await;
    server.await?;
    Ok(())
}

#[tokio::test]
async fn connect_refuses_a_server_not_on_the_allowlist() {
    let mut backend = ActionBackend::new(std::iter::empty());
    let cfg = ServerConfig {
        name: "evil".into(),
        command: "definitely-not-a-real-binary".into(),
        args: Vec::new(),
        env: Default::default(),
    };
    // Denial must happen before any spawn attempt: the command does not
    // exist, so reaching the spawn would surface a different (io) error.
    let err = backend
        .connect(&cfg)
        .await
        .expect_err("default-deny must refuse the connection");
    assert!(err.to_string().contains("allowlist"), "got: {err}");
}

#[tokio::test]
async fn call_tool_on_an_unconnected_server_errors() {
    let backend = ActionBackend::new(["git".to_string()]);
    let err = backend
        .call_tool("git", "commit", json!({}))
        .await
        .expect_err("no connection was made");
    assert!(err.to_string().contains("not connected"), "got: {err}");
}
