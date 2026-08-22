//! Discovery and the real spawned-process connect path.
//!
//! `client_roundtrip.rs` covers the protocol over an in-memory transport, so
//! nothing there ever starts a child. This exercises the other half: reading
//! the server list out of a user's MCP client config, and connecting to a
//! server that is genuinely another process.

use murmur_mcp::{ActionBackend, ServerConfig, discover_servers, parse_mcp_servers};
use std::collections::BTreeMap;

/// Point config discovery at a scratch directory so a developer's real Cursor
/// and Claude Desktop configs are never read by the test suite.
///
/// This mutates process-wide state, so only the single discovery test may use
/// it. Tests that spawn a server isolate the child through its own environment
/// instead, which cannot race.
fn isolate(home: &std::path::Path) {
    unsafe {
        std::env::set_var("MURMUR_HOME_DIR", home);
        std::env::set_var("MURMUR_CONFIG_DIR", home);
    }
}

/// Environment that keeps a spawned server inside `dir`, so a test server
/// never reads or writes the developer's real history.
fn child_env(dir: &std::path::Path) -> BTreeMap<String, String> {
    let path = dir.display().to_string();
    BTreeMap::from([
        ("MURMUR_CONFIG_DIR".to_string(), path.clone()),
        ("MURMUR_HOME_DIR".to_string(), path.clone()),
        ("MURMUR_DATA_DIR".to_string(), path),
    ])
}

fn write_cursor_config(home: &std::path::Path, body: &str) {
    let dir = home.join(".cursor");
    std::fs::create_dir_all(&dir).expect("cursor dir");
    std::fs::write(dir.join("mcp.json"), body).expect("write config");
}

/// All config-discovery cases in one test, deliberately.
///
/// `isolate` sets process-wide environment variables, so separate test
/// functions would race each other's scratch directory under the default
/// parallel runner and fail for reasons that have nothing to do with the code.
#[test]
fn config_discovery_across_the_cases_that_matter() {
    let home = tempfile::tempdir().expect("tempdir");
    isolate(home.path());

    // Nothing configured at all.
    assert!(
        discover_servers().is_empty(),
        "a missing config is not an error"
    );

    // The ordinary case: a server the user already set up for Cursor.
    write_cursor_config(
        home.path(),
        r#"{"mcpServers":{"git":{"command":"git-mcp","args":["--stdio"]}}}"#,
    );
    let found = discover_servers();
    let git = found.iter().find(|s| s.name == "git").expect("git server");
    assert_eq!(git.command, "git-mcp");
    assert_eq!(git.args, vec!["--stdio"]);

    // These files are hand-edited, so a corrupt one must not panic or hide
    // anything, and one unusable entry must not discard the usable ones.
    write_cursor_config(home.path(), "{ this is not json");
    assert!(
        discover_servers().is_empty(),
        "malformed config must not panic"
    );

    write_cursor_config(home.path(), "{}");
    assert!(discover_servers().is_empty(), "empty config yields nothing");

    write_cursor_config(
        home.path(),
        r#"{"mcpServers":{
            "remote":{"type":"http","url":"https://example.invalid"},
            "no_command":{"args":["x"]},
            "good":{"command":"real-server"}
        }}"#,
    );
    let found = discover_servers();
    assert_eq!(found.len(), 1, "only the usable entry survives: {found:?}");
    assert_eq!(found[0].name, "good");
}

#[test]
fn a_server_name_with_the_namespace_separator_is_refused() {
    // '/' separates server from tool, so such a name could never be addressed.
    let err =
        parse_mcp_servers(r#"{"mcpServers":{"a/b":{"command":"x"}}}"#).expect_err("must refuse");
    assert!(format!("{err:#}").contains('/'), "unhelpful error: {err:#}");
}

/// The real thing: spawn `murmur-mcp` as a child, complete the handshake, and
/// list its tools over stdio. Everything before this exercised parsing or an
/// in-memory transport; only this proves a separate process is reachable.
#[tokio::test]
async fn connects_to_a_real_spawned_server_and_lists_its_tools() {
    let home = tempfile::tempdir().expect("tempdir");

    let mut backend = ActionBackend::new(["murmur".to_string()]);
    let cfg = ServerConfig {
        name: "murmur".to_string(),
        command: env!("CARGO_BIN_EXE_murmur-mcp").to_string(),
        args: Vec::new(),
        env: child_env(home.path()),
    };

    backend
        .connect(&cfg)
        .await
        .expect("handshake with a real child process");
    let tools = backend.list_tools().await.expect("list tools");

    assert!(!tools.is_empty(), "a real server offered no tools");
    // Namespacing uses the connection's key, so every tool must carry it.
    for tool in &tools {
        assert!(
            tool.name.starts_with("murmur/"),
            "tool {} is not namespaced to its server",
            tool.name
        );
    }
    backend.shutdown().await;
}

#[tokio::test]
async fn an_unallowlisted_server_is_refused_before_anything_is_spawned() {
    let mut backend = ActionBackend::new(std::iter::empty::<String>());
    let cfg = ServerConfig {
        name: "murmur".to_string(),
        // A command that would fail loudly if it were ever run, so a refusal
        // that happens after spawning would look different from one before.
        command: "definitely-not-a-real-binary-xyz".to_string(),
        args: Vec::new(),
        env: BTreeMap::new(),
    };
    let err = backend.connect(&cfg).await.expect_err("default-deny");
    let msg = format!("{err:#}");
    assert!(msg.contains("allowlist"), "wrong refusal reason: {msg}");
}

/// Startup spawns these, so every failure has to be bounded and reported
/// rather than left hanging. A server the user allowlisted but that is
/// missing, silent, or not speaking MCP must not wedge the connect task.
#[tokio::test]
async fn a_server_that_cannot_serve_fails_instead_of_hanging() {
    let home = tempfile::tempdir().expect("tempdir");
    let cases: [(&str, &str, Vec<String>); 3] = [
        ("missing", "definitely-not-a-real-binary-xyz", Vec::new()),
        // Exits immediately: the handshake gets EOF rather than a reply.
        ("exits", "cmd", vec!["/c".into(), "exit".into()]),
        // Alive and talking, but not the protocol.
        ("garbage", "cmd", vec!["/c".into(), "echo not-mcp".into()]),
    ];

    for (name, command, args) in cases {
        let mut backend = ActionBackend::new([name.to_string()]);
        let cfg = ServerConfig {
            name: name.to_string(),
            command: command.to_string(),
            args,
            env: child_env(home.path()),
        };
        let started = std::time::Instant::now();
        let result = backend.connect(&cfg).await;
        let elapsed = started.elapsed();

        assert!(result.is_err(), "'{name}' should not have connected");
        assert!(
            elapsed < std::time::Duration::from_secs(40),
            "'{name}' took {elapsed:?}, which would stall the connect task"
        );
        // A failed connect must leave nothing behind for a later call to find.
        assert!(
            backend
                .list_tools()
                .await
                .map(|t| t.is_empty())
                .unwrap_or(true),
            "'{name}' left a half-open connection"
        );
    }
}
