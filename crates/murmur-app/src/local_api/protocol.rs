//! Wire protocol for the local API: parse client frames, build server frames,
//! and dispatch requests against a backend. Everything here is a pure function
//! over strings and JSON so the protocol is unit-testable without a socket.

use serde_json::{Value, json};

/// Frontend event names mirrored onto the WebSocket with verbatim payloads.
pub(crate) const FORWARDED_EVENTS: [&str; 4] = [
    "streaming-partial",
    "streaming-phrase",
    "streaming-done",
    "recording-state",
];

/// One frontend event fanned out to every connected client.
#[derive(Debug, Clone)]
pub(crate) struct ApiEvent {
    pub name: &'static str,
    pub payload: Value,
}

/// What the app answers for client requests. The Tauri impl drives the real
/// session; tests substitute a mock.
pub(crate) trait ApiBackend: Send + Sync {
    fn toggle_recording(&self);
    fn status(&self) -> Value;
}

/// A parsed client frame.
#[derive(Debug, PartialEq)]
pub(crate) enum ClientMsg {
    Auth { token: String },
    Request { id: Value, method: String },
    Malformed(String),
}

pub(crate) fn parse_client_message(text: &str) -> ClientMsg {
    let value: Value = match serde_json::from_str(text) {
        Ok(value) => value,
        Err(e) => return ClientMsg::Malformed(format!("invalid JSON: {e}")),
    };
    match value.get("type").and_then(Value::as_str) {
        Some("auth") => match value.get("token").and_then(Value::as_str) {
            Some(token) => ClientMsg::Auth {
                token: token.to_string(),
            },
            None => ClientMsg::Malformed("auth message missing string token".to_string()),
        },
        Some("request") => match value.get("method").and_then(Value::as_str) {
            Some(method) => ClientMsg::Request {
                // The id is opaque to the server: any JSON, echoed back verbatim.
                id: value.get("id").cloned().unwrap_or(Value::Null),
                method: method.to_string(),
            },
            None => ClientMsg::Malformed("request missing string method".to_string()),
        },
        Some(other) => ClientMsg::Malformed(format!("unknown message type: {other}")),
        None => ClientMsg::Malformed("missing message type".to_string()),
    }
}

/// First-frame auth gate: only an exact token match opens the session.
pub(crate) fn auth_ok(msg: &ClientMsg, expected: &str) -> bool {
    matches!(msg, ClientMsg::Auth { token } if token == expected)
}

/// Browsers always send an Origin header on WebSocket handshakes; native
/// editor plugins don't. Rejecting any Origin keeps malicious web pages from
/// ever reaching the token check.
pub(crate) fn handshake_allowed(has_origin_header: bool) -> bool {
    !has_origin_header
}

pub(crate) fn ready_message() -> String {
    json!({ "type": "ready" }).to_string()
}

pub(crate) fn event_message(event: &ApiEvent) -> String {
    json!({ "type": "event", "name": event.name, "payload": event.payload }).to_string()
}

fn response_ok(id: &Value, result: Value) -> String {
    json!({ "type": "response", "id": id, "result": result }).to_string()
}

fn response_error(id: &Value, error: &str) -> String {
    json!({ "type": "response", "id": id, "error": error }).to_string()
}

/// A frame-level error (malformed JSON and the like): reported without an id,
/// and the connection stays open.
fn protocol_error(error: &str) -> String {
    json!({ "type": "error", "error": error }).to_string()
}

/// Route one request. Unknown methods answer an error response instead of
/// closing, so a newer plugin degrades gracefully against an older app.
fn handle_request(backend: &dyn ApiBackend, id: &Value, method: &str) -> String {
    match method {
        "toggle_recording" => {
            backend.toggle_recording();
            response_ok(id, json!({ "ok": true }))
        }
        "get_status" => response_ok(id, backend.status()),
        other => response_error(id, &format!("unknown method: {other}")),
    }
}

/// Reply to one text frame from an already-authenticated client.
pub(crate) fn handle_client_text(backend: &dyn ApiBackend, text: &str) -> String {
    match parse_client_message(text) {
        ClientMsg::Request { id, method } => handle_request(backend, &id, &method),
        ClientMsg::Auth { .. } => protocol_error("already authenticated"),
        ClientMsg::Malformed(error) => protocol_error(&error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct MockBackend {
        toggles: AtomicUsize,
    }

    impl MockBackend {
        fn new() -> Self {
            Self {
                toggles: AtomicUsize::new(0),
            }
        }
    }

    impl ApiBackend for MockBackend {
        fn toggle_recording(&self) {
            self.toggles.fetch_add(1, Ordering::SeqCst);
        }

        fn status(&self) -> Value {
            json!({ "recording": true, "processing": false })
        }
    }

    fn as_json(text: &str) -> Value {
        serde_json::from_str(text).expect("server frames must be valid JSON")
    }

    #[test]
    fn parses_auth_and_request_frames() {
        assert_eq!(
            parse_client_message(r#"{"type":"auth","token":"abc"}"#),
            ClientMsg::Auth {
                token: "abc".to_string()
            }
        );
        assert_eq!(
            parse_client_message(r#"{"type":"request","id":7,"method":"get_status"}"#),
            ClientMsg::Request {
                id: json!(7),
                method: "get_status".to_string()
            }
        );
        // The id is any JSON, and a missing id defaults to null.
        assert_eq!(
            parse_client_message(r#"{"type":"request","id":{"seq":1},"method":"m"}"#),
            ClientMsg::Request {
                id: json!({ "seq": 1 }),
                method: "m".to_string()
            }
        );
        assert_eq!(
            parse_client_message(r#"{"type":"request","method":"m"}"#),
            ClientMsg::Request {
                id: Value::Null,
                method: "m".to_string()
            }
        );
    }

    #[test]
    fn rejects_malformed_frames_without_panicking() {
        for text in [
            "{not json",
            r#"{"no":"type"}"#,
            r#"{"type":"unknown"}"#,
            r#"{"type":"auth"}"#,
            r#"{"type":"auth","token":5}"#,
            r#"{"type":"request","id":1}"#,
        ] {
            assert!(
                matches!(parse_client_message(text), ClientMsg::Malformed(_)),
                "expected Malformed for {text}"
            );
        }
    }

    #[test]
    fn auth_requires_an_exact_token_match() {
        let auth = parse_client_message(r#"{"type":"auth","token":"secret"}"#);
        assert!(auth_ok(&auth, "secret"));
        assert!(!auth_ok(&auth, "SECRET"));
        assert!(!auth_ok(&auth, "secret2"));
        // A non-auth first frame never authenticates.
        let request = parse_client_message(r#"{"type":"request","method":"get_status"}"#);
        assert!(!auth_ok(&request, "secret"));
    }

    #[test]
    fn any_origin_header_fails_the_handshake() {
        assert!(handshake_allowed(false));
        assert!(!handshake_allowed(true));
    }

    #[test]
    fn server_frames_have_the_documented_shapes() {
        assert_eq!(as_json(&ready_message()), json!({ "type": "ready" }));
        let event = ApiEvent {
            name: "streaming-partial",
            payload: json!({ "text": "hello" }),
        };
        assert_eq!(
            as_json(&event_message(&event)),
            json!({ "type": "event", "name": "streaming-partial", "payload": { "text": "hello" } })
        );
    }

    #[test]
    fn toggle_request_routes_to_the_backend_and_echoes_the_id() {
        let backend = MockBackend::new();
        let reply = handle_client_text(
            &backend,
            r#"{"type":"request","id":"req-1","method":"toggle_recording"}"#,
        );
        assert_eq!(backend.toggles.load(Ordering::SeqCst), 1);
        assert_eq!(
            as_json(&reply),
            json!({ "type": "response", "id": "req-1", "result": { "ok": true } })
        );
    }

    #[test]
    fn get_status_returns_the_backend_status() {
        let backend = MockBackend::new();
        let reply = handle_client_text(
            &backend,
            r#"{"type":"request","id":2,"method":"get_status"}"#,
        );
        assert_eq!(
            as_json(&reply),
            json!({
                "type": "response",
                "id": 2,
                "result": { "recording": true, "processing": false }
            })
        );
        assert_eq!(backend.toggles.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn unknown_method_answers_an_error_response_with_the_id() {
        let backend = MockBackend::new();
        let reply = handle_client_text(&backend, r#"{"type":"request","id":3,"method":"reboot"}"#);
        let value = as_json(&reply);
        assert_eq!(value["type"], "response");
        assert_eq!(value["id"], 3);
        assert_eq!(value["error"], "unknown method: reboot");
        assert!(value.get("result").is_none());
    }

    #[test]
    fn malformed_text_answers_a_frame_error() {
        let backend = MockBackend::new();
        let value = as_json(&handle_client_text(&backend, "{not json"));
        assert_eq!(value["type"], "error");
        assert!(value["error"].as_str().is_some_and(|e| !e.is_empty()));
        // A second auth on a live session is a protocol error, not a re-auth.
        let value = as_json(&handle_client_text(
            &backend,
            r#"{"type":"auth","token":"x"}"#,
        ));
        assert_eq!(value["type"], "error");
    }
}
