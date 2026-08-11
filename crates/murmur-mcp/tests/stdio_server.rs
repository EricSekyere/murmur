//! Integration tests that drive the real MCP server binary over stdio, the
//! way an editor would: spawn `murmur-mcp`, speak newline-delimited JSON-RPC
//! on stdin, and read responses from stdout. Every test points the server at
//! a private fixture directory via `MURMUR_CONFIG_DIR`, so the user's real
//! config is never read or written. Every line the server emits on stdout is
//! asserted to parse as JSON: stdout purity is enforced on every receive, in
//! every test, including under `RUST_LOG=trace` and on error paths.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

const RECV_TIMEOUT: Duration = Duration::from_secs(15);

/// A spawned server child plus channels for its output streams.
struct Server {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout_rx: mpsc::Receiver<String>,
    stderr_rx: mpsc::Receiver<String>,
}

impl Server {
    /// Spawn the server binary against `config_base`, with optional extra env.
    fn spawn(config_base: &Path, extra_env: &[(&str, &str)]) -> Self {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_murmur-mcp"));
        cmd.env("MURMUR_CONFIG_DIR", config_base)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (k, v) in extra_env {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().expect("spawn murmur-mcp");
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().expect("stdout piped");
        let stderr = child.stderr.take().expect("stderr piped");

        let (stdout_tx, stdout_rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if stdout_tx.send(line).is_err() {
                    return;
                }
            }
        });
        let (stderr_tx, stderr_rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if stderr_tx.send(line).is_err() {
                    return;
                }
            }
        });
        Self {
            child,
            stdin,
            stdout_rx,
            stderr_rx,
        }
    }

    /// Write one raw line to the server's stdin.
    fn send_raw(&mut self, line: &str) {
        let stdin = self.stdin.as_mut().expect("stdin open");
        writeln!(stdin, "{line}").expect("write to server stdin");
        stdin.flush().expect("flush server stdin");
    }

    fn send(&mut self, message: &Value) {
        self.send_raw(&message.to_string());
    }

    /// Receive the next stdout line and require it to be valid JSON. This is
    /// where stdout purity is enforced for every test in the file.
    fn recv(&mut self) -> Value {
        let line = self
            .stdout_rx
            .recv_timeout(RECV_TIMEOUT)
            .expect("timed out waiting for a server response");
        serde_json::from_str(&line).unwrap_or_else(|e| panic!("non-JSON on stdout ({e}): {line:?}"))
    }

    /// Assert that nothing arrives on stdout within `window`.
    fn assert_silent_for(&mut self, window: Duration) {
        if let Ok(line) = self.stdout_rx.recv_timeout(window) {
            panic!("expected no response, got: {line:?}");
        }
    }

    /// Standard MCP handshake; the returned value is the initialize result.
    fn initialize(&mut self) -> Value {
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": 0,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "murmur-tests", "version": "0"}
            }
        }));
        let resp = self.recv();
        assert_eq!(resp["id"], 0);
        self.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        resp["result"].clone()
    }

    fn call_tool(&mut self, id: u64, name: &str, arguments: Value) -> Value {
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": {"name": name, "arguments": arguments}
        }));
        let resp = self.recv();
        assert_eq!(resp["id"], id, "response id must echo the request id");
        resp
    }

    fn close_stdin(&mut self) {
        drop(self.stdin.take());
    }

    /// Wait for the child to exit on its own, up to `timeout`.
    fn wait_exit(&mut self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            match self.child.try_wait().expect("try_wait") {
                Some(_) => return true,
                None => std::thread::sleep(Duration::from_millis(50)),
            }
        }
        false
    }

    /// Drain whatever stderr produced so far and assert no panic happened.
    fn assert_no_panic_on_stderr(&mut self) {
        while let Ok(line) = self.stderr_rx.try_recv() {
            assert!(
                !line.contains("panicked"),
                "server panicked on stderr: {line}"
            );
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Write a history fixture (newest first) under `base/murmur/history.json`.
fn write_history(base: &Path, entries: &[(&str, u64)]) {
    let dir = base.join("murmur");
    std::fs::create_dir_all(&dir).expect("mkdir");
    let entries: Vec<Value> = entries
        .iter()
        .map(|(text, ts)| json!({"text": text, "timestamp_ms": ts}))
        .collect();
    std::fs::write(
        dir.join("history.json"),
        json!({"entries": entries}).to_string(),
    )
    .expect("write history fixture");
}

fn write_config(base: &Path, body: &str) {
    let dir = base.join("murmur");
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(dir.join("config.toml"), body).expect("write config fixture");
}

/// A tool failure is acceptable as either a JSON-RPC error response or a
/// CallToolResult with `isError` set; both keep the protocol intact.
fn assert_tool_failed(resp: &Value) {
    let is_rpc_error = resp.get("error").is_some();
    let is_tool_error = resp["result"]["isError"].as_bool() == Some(true);
    assert!(
        is_rpc_error || is_tool_error,
        "expected a tool failure, got: {resp}"
    );
}

/// Extract the text payload of a successful tools/call response.
fn tool_text(resp: &Value) -> String {
    assert!(resp.get("error").is_none(), "expected success, got: {resp}");
    resp["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("no text content in: {resp}"))
        .to_string()
}

fn tool_json(resp: &Value) -> Value {
    serde_json::from_str(&tool_text(resp)).expect("tool payload must be JSON")
}

// --- protocol shape -------------------------------------------------------

#[test]
fn initialize_reports_the_murmur_server() {
    let base = tempfile::tempdir().expect("tempdir");
    let mut server = Server::spawn(base.path(), &[]);
    let result = server.initialize();
    assert_eq!(result["serverInfo"]["name"], "murmur");
    assert!(result["capabilities"]["tools"].is_object());
}

#[test]
fn tools_list_exposes_exactly_the_four_tools() {
    let base = tempfile::tempdir().expect("tempdir");
    let mut server = Server::spawn(base.path(), &[]);
    server.initialize();
    server.send(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}));
    let resp = server.recv();
    let mut names: Vec<&str> = resp["result"]["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    names.sort_unstable();
    assert_eq!(
        names,
        vec![
            "get_recent_transcripts",
            "request_dictation",
            "search_transcripts",
            "wait_for_next_dictation",
        ]
    );
}

#[test]
fn unknown_method_returns_method_not_found_with_the_id_echoed() {
    let base = tempfile::tempdir().expect("tempdir");
    let mut server = Server::spawn(base.path(), &[]);
    server.initialize();
    server.send(&json!({"jsonrpc": "2.0", "id": 7, "method": "no/such_method"}));
    let resp = server.recv();
    assert_eq!(resp["id"], 7);
    assert_eq!(resp["error"]["code"], -32601);
}

#[test]
fn unknown_tool_fails_without_killing_the_server() {
    let base = tempfile::tempdir().expect("tempdir");
    let mut server = Server::spawn(base.path(), &[]);
    server.initialize();
    let resp = server.call_tool(2, "no_such_tool", json!({}));
    assert_tool_failed(&resp);
    // The connection stays usable afterwards.
    server.send(&json!({"jsonrpc": "2.0", "id": 3, "method": "tools/list"}));
    assert_eq!(server.recv()["id"], 3);
}

#[test]
fn string_and_large_number_ids_are_echoed_back() {
    let base = tempfile::tempdir().expect("tempdir");
    let mut server = Server::spawn(base.path(), &[]);
    server.initialize();
    server.send(&json!({"jsonrpc": "2.0", "id": "req-abc", "method": "tools/list"}));
    assert_eq!(server.recv()["id"], "req-abc");
    server.send(&json!({"jsonrpc": "2.0", "id": 9_007_199_254_740_991u64, "method": "tools/list"}));
    assert_eq!(server.recv()["id"], 9_007_199_254_740_991u64);
}

#[test]
fn a_request_without_an_id_gets_no_response() {
    let base = tempfile::tempdir().expect("tempdir");
    let mut server = Server::spawn(base.path(), &[]);
    server.initialize();
    // No id makes this a notification; a response would be a protocol bug.
    server.send(&json!({"jsonrpc": "2.0", "method": "tools/list"}));
    server.assert_silent_for(Duration::from_millis(500));
    // A null id must not wedge or crash the connection either.
    server.send(&json!({"jsonrpc": "2.0", "id": null, "method": "tools/list"}));
    server.send(&json!({"jsonrpc": "2.0", "id": 5, "method": "tools/list"}));
    assert_eq!(server.recv()["id"], 5);
}

#[test]
fn malformed_and_hostile_input_lines_do_not_kill_the_server() {
    let base = tempfile::tempdir().expect("tempdir");
    let mut server = Server::spawn(base.path(), &[("RUST_LOG", "trace")]);
    server.initialize();
    for hostile in [
        "this is not json",
        "{\"jsonrpc\":\"2.0\"",
        "42",
        "\"just a string\"",
        "[]",
        "{}",
        "{\"jsonrpc\":\"1.0\",\"id\":1,\"method\":\"x\"}",
        "{\"jsonrpc\":\"2.0\",\"id\":{},\"method\":\"tools/list\"}",
    ] {
        server.send_raw(hostile);
    }
    // Drain any error responses the hostile lines produced; recv() itself
    // asserts each is valid JSON (stdout purity on error paths).
    while let Ok(line) = server.stdout_rx.recv_timeout(Duration::from_millis(300)) {
        let _: Value = serde_json::from_str(&line)
            .unwrap_or_else(|e| panic!("non-JSON on stdout ({e}): {line:?}"));
    }
    // The server is still alive and serving.
    server.send(&json!({"jsonrpc": "2.0", "id": 11, "method": "tools/list"}));
    assert_eq!(server.recv()["id"], 11);
    server.assert_no_panic_on_stderr();
}

#[test]
fn a_partial_line_then_eof_shuts_down_cleanly() {
    let base = tempfile::tempdir().expect("tempdir");
    let mut server = Server::spawn(base.path(), &[("RUST_LOG", "trace")]);
    server.initialize();
    {
        let stdin = server.stdin.as_mut().expect("stdin open");
        // No trailing newline: the line is torn mid-message at EOF.
        write!(stdin, "{{\"jsonrpc\":\"2.0\",\"id\":1,\"meth").expect("write partial");
        stdin.flush().expect("flush");
    }
    server.close_stdin();
    assert!(
        server.wait_exit(Duration::from_secs(10)),
        "server must exit after EOF on stdin"
    );
    // Anything that reached stdout must still be JSON.
    while let Ok(line) = server.stdout_rx.try_recv() {
        let _: Value = serde_json::from_str(&line)
            .unwrap_or_else(|e| panic!("non-JSON on stdout ({e}): {line:?}"));
    }
    server.assert_no_panic_on_stderr();
}

// --- history tools --------------------------------------------------------

#[test]
fn recent_transcripts_returns_fixture_entries_newest_first() {
    let base = tempfile::tempdir().expect("tempdir");
    write_history(
        base.path(),
        &[("newest phrase", 2_000), ("older phrase", 1_000)],
    );
    let mut server = Server::spawn(base.path(), &[]);
    server.initialize();
    let payload = tool_json(&server.call_tool(1, "get_recent_transcripts", json!({})));
    let items = payload.as_array().expect("array payload");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["text"], "newest phrase");
    assert_eq!(items[0]["timestamp_ms"], 2_000);
    assert_eq!(items[1]["text"], "older phrase");
}

#[test]
fn recent_transcripts_clamps_the_limit() {
    let base = tempfile::tempdir().expect("tempdir");
    write_history(base.path(), &[("a", 3_000), ("b", 2_000), ("c", 1_000)]);
    let mut server = Server::spawn(base.path(), &[]);
    server.initialize();
    // limit 0 clamps up to 1 rather than erroring or returning everything.
    let low = tool_json(&server.call_tool(1, "get_recent_transcripts", json!({"limit": 0})));
    assert_eq!(low.as_array().expect("array").len(), 1);
    // An absurdly large limit is served (clamped) rather than rejected.
    let high = tool_json(&server.call_tool(2, "get_recent_transcripts", json!({"limit": 100_000})));
    assert_eq!(high.as_array().expect("array").len(), 3);
}

#[test]
fn recent_transcripts_rejects_a_wrongly_typed_limit() {
    let base = tempfile::tempdir().expect("tempdir");
    write_history(base.path(), &[("a", 1_000)]);
    let mut server = Server::spawn(base.path(), &[]);
    server.initialize();
    for (id, bad) in [(1, json!({"limit": "five"})), (2, json!({"limit": -3}))] {
        let resp = server.call_tool(id, "get_recent_transcripts", bad);
        assert_tool_failed(&resp);
    }
}

#[test]
fn search_requires_a_query_and_matches_case_insensitively() {
    let base = tempfile::tempdir().expect("tempdir");
    write_history(
        base.path(),
        &[("Deploy the Server", 2_000), ("order coffee", 1_000)],
    );
    let mut server = Server::spawn(base.path(), &[]);
    server.initialize();
    // Missing required argument fails validation.
    assert_tool_failed(&server.call_tool(1, "search_transcripts", json!({})));
    // A wrongly typed query fails too.
    assert_tool_failed(&server.call_tool(2, "search_transcripts", json!({"query": 42})));
    let hits = tool_json(&server.call_tool(3, "search_transcripts", json!({"query": "SERVER"})));
    let items = hits.as_array().expect("array");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["text"], "Deploy the Server");
    let none = tool_json(&server.call_tool(4, "search_transcripts", json!({"query": "nonsense"})));
    assert_eq!(none.as_array().expect("array").len(), 0);
}

#[test]
fn corrupt_history_degrades_to_empty_without_touching_the_file() {
    let base = tempfile::tempdir().expect("tempdir");
    let dir = base.path().join("murmur");
    std::fs::create_dir_all(&dir).expect("mkdir");
    let history_path = dir.join("history.json");
    std::fs::write(&history_path, "{definitely not json").expect("write");
    let mut server = Server::spawn(base.path(), &[]);
    server.initialize();
    let payload = tool_json(&server.call_tool(1, "get_recent_transcripts", json!({})));
    assert_eq!(payload.as_array().expect("array").len(), 0);
    // Read-only degradation: the corrupt file is not renamed or rewritten.
    assert_eq!(
        std::fs::read_to_string(&history_path).expect("read"),
        "{definitely not json"
    );
    assert!(!history_path.with_extension("json.bak").exists());
}

#[test]
fn history_rewritten_by_another_process_is_picked_up_per_call() {
    let base = tempfile::tempdir().expect("tempdir");
    write_history(base.path(), &[("first", 1_000)]);
    let mut server = Server::spawn(base.path(), &[]);
    server.initialize();
    let before = tool_json(&server.call_tool(1, "get_recent_transcripts", json!({})));
    assert_eq!(before.as_array().expect("array").len(), 1);
    // Simulate the app appending concurrently: the server reads fresh state
    // on every call rather than caching the log.
    write_history(base.path(), &[("second", 2_000), ("first", 1_000)]);
    let after = tool_json(&server.call_tool(2, "get_recent_transcripts", json!({})));
    let items = after.as_array().expect("array");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["text"], "second");
    // A torn/garbage overwrite degrades to empty instead of erroring.
    std::fs::write(base.path().join("murmur").join("history.json"), "{torn").expect("write");
    let torn = tool_json(&server.call_tool(3, "get_recent_transcripts", json!({})));
    assert_eq!(torn.as_array().expect("array").len(), 0);
}

#[test]
fn disabled_history_hides_transcripts_and_blocks_waits() {
    let base = tempfile::tempdir().expect("tempdir");
    write_config(base.path(), "save_history = false\n");
    write_history(base.path(), &[("leftover entry", 1_000)]);
    let mut server = Server::spawn(base.path(), &[]);
    server.initialize();
    let payload = tool_json(&server.call_tool(1, "get_recent_transcripts", json!({})));
    assert_eq!(payload.as_array().expect("array").len(), 0);
    let wait = tool_json(&server.call_tool(2, "wait_for_next_dictation", json!({})));
    assert_eq!(wait["status"], "history_disabled");
}

// --- wait/request tools ---------------------------------------------------

#[test]
fn wait_for_next_dictation_honors_its_timeout() {
    let base = tempfile::tempdir().expect("tempdir");
    write_history(base.path(), &[("existing", 1_000)]);
    let mut server = Server::spawn(base.path(), &[]);
    server.initialize();
    let started = Instant::now();
    let outcome =
        tool_json(&server.call_tool(1, "wait_for_next_dictation", json!({"timeout_secs": 1})));
    assert_eq!(outcome["status"], "timed_out");
    assert_eq!(outcome["waited_secs"], 1);
    let elapsed = started.elapsed();
    assert!(
        elapsed >= Duration::from_millis(900) && elapsed < Duration::from_secs(10),
        "wait took {elapsed:?}, expected about 1s"
    );
    // The pre-existing entry must not be re-delivered as "new".
}

#[test]
fn wait_for_next_dictation_sees_an_entry_written_mid_wait() {
    let base = tempfile::tempdir().expect("tempdir");
    let old_ms = now_ms();
    write_history(base.path(), &[("existing", old_ms)]);
    let mut server = Server::spawn(base.path(), &[]);
    server.initialize();
    server.send(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {"name": "wait_for_next_dictation", "arguments": {"timeout_secs": 10}}
    }));
    // Let the wait baseline settle, then play the role of the app appending.
    std::thread::sleep(Duration::from_millis(700));
    write_history(
        base.path(),
        &[("the spoken answer", old_ms + 60_000), ("existing", old_ms)],
    );
    let resp = server.recv();
    assert_eq!(resp["id"], 1);
    let outcome: Value =
        serde_json::from_str(resp["result"]["content"][0]["text"].as_str().expect("text"))
            .expect("outcome JSON");
    assert_eq!(outcome["status"], "received");
    assert_eq!(outcome["text"], "the spoken answer");
}

#[test]
fn wait_survives_a_client_disconnect_without_hanging_or_panicking() {
    let base = tempfile::tempdir().expect("tempdir");
    write_history(base.path(), &[("existing", 1_000)]);
    let mut server = Server::spawn(base.path(), &[("RUST_LOG", "trace")]);
    server.initialize();
    server.send(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {"name": "wait_for_next_dictation", "arguments": {"timeout_secs": 3}}
    }));
    // The client goes away mid-wait. The server must wind down on its own
    // (at worst after the 3s tool timeout), not hang or panic.
    server.close_stdin();
    assert!(
        server.wait_exit(Duration::from_secs(15)),
        "server must exit after the client disconnects"
    );
    while let Ok(line) = server.stdout_rx.try_recv() {
        let _: Value = serde_json::from_str(&line)
            .unwrap_or_else(|e| panic!("non-JSON on stdout ({e}): {line:?}"));
    }
    server.assert_no_panic_on_stderr();
}

#[test]
fn request_dictation_times_out_and_retires_its_own_trigger() {
    let base = tempfile::tempdir().expect("tempdir");
    write_history(base.path(), &[("existing", 1_000)]);
    let mut server = Server::spawn(base.path(), &[]);
    server.initialize();
    let outcome = tool_json(&server.call_tool(
        1,
        "request_dictation",
        json!({"prompt": "which branch?", "timeout_secs": 1}),
    ));
    assert_eq!(outcome["status"], "timed_out");
    // The trigger must not stay armed: a leftover request file would make
    // the app open the microphone unprompted minutes later.
    assert!(
        !base
            .path()
            .join("murmur")
            .join("dictation-request.json")
            .exists(),
        "an expired request must clear its trigger"
    );
}
