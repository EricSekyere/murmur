//! Accept loop and per-client tasks for the local API. Each client runs on
//! its own task: a disconnect, a slow reader, or a bad frame affects that
//! client only, never the app or its peers.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt, stream::SplitStream};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::{StatusCode, header::ORIGIN};
use tokio_tungstenite::tungstenite::protocol::Message;

use super::protocol::{self, ApiBackend, ApiEvent};

/// Editor plugins come one or two per machine; anything past this cap is a
/// bug or abuse and is refused.
const MAX_CLIENTS: usize = 8;
/// A client that hasn't authenticated within this window is cut loose.
const AUTH_TIMEOUT: Duration = Duration::from_secs(5);
/// Broadcast depth per client. A slow client lags (drops oldest events)
/// rather than back-pressuring the app or losing its connection.
pub(super) const EVENT_BUFFER: usize = 256;

type WsReader = SplitStream<WebSocketStream<TcpStream>>;

/// Serve until the app exits. Accept errors are transient (a client vanishing
/// mid-handshake), so the loop logs and keeps listening.
pub(super) async fn run(
    listener: TcpListener,
    token: Arc<str>,
    events: broadcast::Sender<ApiEvent>,
    backend: Arc<dyn ApiBackend>,
) {
    let clients = Arc::new(AtomicUsize::new(0));
    loop {
        let (stream, _) = match listener.accept().await {
            Ok(conn) => conn,
            Err(e) => {
                tracing::warn!("Local API accept failed: {e}");
                continue;
            }
        };
        if clients.fetch_add(1, Ordering::AcqRel) >= MAX_CLIENTS {
            clients.fetch_sub(1, Ordering::AcqRel);
            tracing::debug!("local API connection refused: at capacity");
            // Dropping the stream closes it before any handshake work.
            continue;
        }
        let token = Arc::clone(&token);
        let receiver = events.subscribe();
        let backend = Arc::clone(&backend);
        let clients = Arc::clone(&clients);
        tauri::async_runtime::spawn(async move {
            if let Err(e) = serve_client(stream, &token, receiver, backend).await {
                // Client disconnects mid-frame are routine, not app problems.
                tracing::debug!("local API client ended: {e:#}");
            }
            clients.fetch_sub(1, Ordering::AcqRel);
        });
    }
}

/// One client's lifetime: handshake, first-frame auth, then a select loop
/// pushing broadcast events out while answering requests.
async fn serve_client(
    stream: TcpStream,
    token: &str,
    mut events: broadcast::Receiver<ApiEvent>,
    backend: Arc<dyn ApiBackend>,
) -> Result<()> {
    let ws = tokio_tungstenite::accept_hdr_async(stream, reject_browser_origins)
        .await
        .context("websocket handshake")?;
    let (mut sink, mut reader) = ws.split();

    if !await_auth(&mut reader, token).await? {
        let _ = sink.send(Message::Close(None)).await;
        return Ok(());
    }
    sink.send(Message::text(protocol::ready_message())).await?;

    loop {
        tokio::select! {
            event = events.recv() => match event {
                Ok(event) => {
                    sink.send(Message::text(protocol::event_message(&event)))
                        .await
                        .context("push event")?;
                }
                // Lagging costs the oldest events only, never the connection.
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            },
            frame = reader.next() => {
                let Some(frame) = frame else { break };
                match frame.context("read client frame")? {
                    Message::Text(text) => {
                        let reply = protocol::handle_client_text(backend.as_ref(), text.as_str());
                        sink.send(Message::text(reply)).await.context("send reply")?;
                    }
                    Message::Close(_) => break,
                    // Ping/pong are answered by tungstenite itself; binary
                    // frames carry nothing in this protocol.
                    _ => {}
                }
            }
        }
    }
    let _ = sink.send(Message::Close(None)).await;
    Ok(())
}

/// First-frame auth gate: the client's first text frame must carry the token
/// within [`AUTH_TIMEOUT`]. The token is never logged, matched or not.
async fn await_auth(reader: &mut WsReader, token: &str) -> Result<bool> {
    match tokio::time::timeout(AUTH_TIMEOUT, first_text_frame(reader)).await {
        Ok(Ok(Some(text))) => Ok(protocol::auth_ok(
            &protocol::parse_client_message(&text),
            token,
        )),
        // Closed before authenticating.
        Ok(Ok(None)) => Ok(false),
        Ok(Err(e)) => Err(e).context("read auth frame"),
        // Timed out silent: close rather than hold an unauthenticated socket.
        Err(_) => Ok(false),
    }
}

/// The next text frame, skipping control frames; `None` once the client
/// closes without sending one.
async fn first_text_frame(reader: &mut WsReader) -> Result<Option<String>> {
    while let Some(frame) = reader.next().await {
        match frame.context("read frame")? {
            Message::Text(text) => return Ok(Some(text.as_str().to_string())),
            Message::Close(_) => return Ok(None),
            _ => {}
        }
    }
    Ok(None)
}

/// Handshake gate: refuse any request carrying an Origin header (always sent
/// by browsers, never by native editor plugins) before the socket upgrades.
// The Err type (a full HTTP response) is dictated by tungstenite's Callback
// trait, so its size is not ours to shrink.
#[allow(clippy::result_large_err)]
fn reject_browser_origins(
    request: &Request,
    response: Response,
) -> std::result::Result<Response, ErrorResponse> {
    if !protocol::handshake_allowed(request.headers().contains_key(ORIGIN)) {
        let mut refuse = ErrorResponse::new(Some("browser origins are not allowed".to_string()));
        *refuse.status_mut() = StatusCode::FORBIDDEN;
        return Err(refuse);
    }
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;

    struct MockBackend;

    impl ApiBackend for MockBackend {
        fn toggle_recording(&self) {}

        fn status(&self) -> Value {
            json!({ "recording": false, "processing": false })
        }

        fn start_meeting(&self) -> Result<(), String> {
            Ok(())
        }

        fn stop_meeting(&self) -> Result<(), String> {
            Ok(())
        }
    }

    /// Bind an ephemeral loopback listener and run the server on it.
    async fn start_test_server(token: &str) -> (u16, broadcast::Sender<ApiEvent>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let (events, _) = broadcast::channel(EVENT_BUFFER);
        let sender = events.clone();
        let token: Arc<str> = token.into();
        tokio::spawn(run(listener, token, events, Arc::new(MockBackend)));
        (port, sender)
    }

    async fn connect(port: u16) -> WebSocketStream<TcpStream> {
        let stream = TcpStream::connect(("127.0.0.1", port)).await.expect("tcp");
        let (ws, _) = tokio_tungstenite::client_async(format!("ws://127.0.0.1:{port}/"), stream)
            .await
            .expect("websocket client handshake");
        ws
    }

    async fn next_json(ws: &mut WebSocketStream<TcpStream>) -> Value {
        let frame = tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .expect("server reply in time")
            .expect("connection open")
            .expect("clean frame");
        match frame {
            Message::Text(text) => serde_json::from_str(text.as_str()).expect("json"),
            other => panic!("expected text frame, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn authed_client_gets_ready_events_and_responses() {
        let (port, events) = start_test_server("tok-123").await;
        let mut ws = connect(port).await;

        ws.send(Message::text(r#"{"type":"auth","token":"tok-123"}"#))
            .await
            .expect("send auth");
        assert_eq!(next_json(&mut ws).await, json!({ "type": "ready" }));

        ws.send(Message::text(
            r#"{"type":"request","id":1,"method":"get_status"}"#,
        ))
        .await
        .expect("send request");
        assert_eq!(
            next_json(&mut ws).await,
            json!({
                "type": "response",
                "id": 1,
                "result": { "recording": false, "processing": false }
            })
        );

        events
            .send(ApiEvent {
                name: "streaming-partial",
                payload: json!({ "text": "hel" }),
            })
            .expect("client subscribed");
        assert_eq!(
            next_json(&mut ws).await,
            json!({
                "type": "event",
                "name": "streaming-partial",
                "payload": { "text": "hel" }
            })
        );
    }

    #[tokio::test]
    async fn wrong_token_is_closed_without_a_ready() {
        let (port, _events) = start_test_server("right").await;
        let mut ws = connect(port).await;

        ws.send(Message::text(r#"{"type":"auth","token":"wrong"}"#))
            .await
            .expect("send auth");
        let frame = tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .expect("server acts in time")
            .expect("a close frame, not silence");
        assert!(
            matches!(frame, Ok(Message::Close(_))),
            "expected close, got {frame:?}"
        );
    }

    #[tokio::test]
    async fn handshake_with_an_origin_header_is_refused() {
        let (port, _events) = start_test_server("tok").await;
        let stream = TcpStream::connect(("127.0.0.1", port)).await.expect("tcp");
        let mut request = format!("ws://127.0.0.1:{port}/")
            .into_client_request()
            .expect("request");
        request
            .headers_mut()
            .insert(ORIGIN, "https://evil.example".parse().expect("header"));

        let result = tokio_tungstenite::client_async(request, stream).await;
        assert!(result.is_err(), "browser-origin handshake must be refused");
    }
}
