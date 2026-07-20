# Local WebSocket API

Murmur can expose a local WebSocket API that streams live dictation events and
accepts a small set of control requests. It exists so editor plugins (VS Code,
Neovim, and similar) can integrate precisely instead of relying on synthetic
keystrokes.

Security posture:

- **Off by default.** Enable it in Settings under "Editor integration (MCP)":
  "Local API for editor plugins". Toggling takes effect after restarting
  Murmur.
- **Localhost only.** The server binds `127.0.0.1` on an ephemeral port and is
  never reachable from another machine.
- **Token-authenticated.** A fresh random token is generated on every app
  start. The first message on a connection must present it.
- **Browser-proof.** Any WebSocket handshake carrying an `Origin` header
  (browsers always send one) is refused with HTTP 403 before authentication.

## Discovery file

On startup with the API enabled, Murmur writes the endpoint to:

- Windows: `%APPDATA%\murmur\local-api.json`
- Linux: `~/.config/murmur/local-api.json`
- macOS: `~/Library/Application Support/murmur/local-api.json`

Schema:

```json
{ "port": 52341, "token": "3f9c0a1b8d2e4c6f9a0b1c2d3e4f5a6b" }
```

The file is written atomically, so it is always either absent or complete.
When the API is disabled (or the app is started with it off), the file is
deleted so a stale endpoint is never advertised. Clients should re-read it on
every reconnect: the port and token change on each app start.

## Connecting and authenticating

1. Open `ws://127.0.0.1:<port>/` without an `Origin` header.
2. Send the auth message as the first text frame, within 5 seconds:

```json
{ "type": "auth", "token": "3f9c0a1b8d2e4c6f9a0b1c2d3e4f5a6b" }
```

3. On success the server replies:

```json
{ "type": "ready" }
```

A wrong token, a non-auth first message, or 5 seconds of silence closes the
connection. At most 8 clients may be connected at once; further connections
are closed immediately after accept.

## Messages

All frames are JSON text frames.

### Events (server to client)

`{ "type": "event", "name": <event>, "payload": <json> }` — the same payloads
the Murmur frontend receives, forwarded verbatim:

| Name | Payload | Meaning |
|------|---------|---------|
| `recording-state` | `{ "recording": bool, "processing": bool }` | Session started/stopped, or a phrase is being transcribed |
| `streaming-partial` | `{ "text": "so far..." }` | Live preview of the phrase in progress (superseded by the final text) |
| `streaming-phrase` | `{ "text": "final text", "processing_time_ms": 412 }` | A finished phrase, as delivered |
| `streaming-done` | `{}` | The session's streaming worker finished |

Example:

```json
{ "type": "event", "name": "streaming-phrase", "payload": { "text": "hello world.", "processing_time_ms": 412 } }
```

A client that reads too slowly loses the oldest events rather than being
disconnected.

### Requests (client to server)

`{ "type": "request", "id": <any json>, "method": <name> }` — the `id` is
opaque and echoed back verbatim (it may be omitted; the response then carries
`"id": null`). Methods:

- `toggle_recording` — same as the hotkey; starts or stops a dictation
  session. Result: `{ "ok": true }`.
- `get_status` — result: `{ "recording": bool, "processing": bool }`.
- `start_meeting` — begin a meeting recording (microphone plus system audio
  where supported), identical to the dashboard's Start Meeting button.
  Refusals (dictation active, meeting already running, model still loading)
  come back as an error response.
- `stop_meeting` — signal the running meeting to stop. Returns immediately;
  the final chunk transcribes and the record saves in the background.

```json
{ "type": "request", "id": 1, "method": "get_status" }
{ "type": "response", "id": 1, "result": { "recording": false, "processing": false } }
```

An unknown method answers an error response and the connection stays open:

```json
{ "type": "response", "id": 2, "error": "unknown method: reboot" }
```

A frame that isn't valid JSON (or isn't a known message shape) answers a
frame-level error, also without closing:

```json
{ "type": "error", "error": "invalid JSON: expected value at line 1 column 2" }
```

## Reference clients

- [`editors/vscode/`](../editors/vscode/) — first-party VS Code extension consuming this API.
