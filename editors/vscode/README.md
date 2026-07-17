# Murmur Dictation for VS Code

First-party **reference client** for Murmur's [local WebSocket API](../../docs/local-api.md).
It is developed in-repo to prove the API from a real editor and to seed
third-party integrations; it is not published to the marketplace.

## Prerequisites

- Murmur >= v0.12.0 running on the same machine, with **"Local API for editor
  plugins"** enabled in Settings under "Editor integration (MCP)", then Murmur
  restarted. Murmur then writes the discovery file
  (`%APPDATA%\murmur\local-api.json` on Windows, `~/.config/murmur/` on Linux,
  `~/Library/Application Support/murmur/` on macOS) that this extension reads.
- A VS Code whose extension host provides the built-in `WebSocket` global
  (Node.js 21+; current VS Code releases ship Node 22). The extension has
  **zero runtime dependencies** because of this: no `ws`, no bundler.
- Node.js 22 + npm to build from source.

## Running from source

```
cd editors/vscode
npm install
npm run build
```

Then open `editors/vscode/` in VS Code and press **F5** to launch an Extension
Development Host with the extension loaded.

## Features

- **Status bar item** (left side) showing connection and microphone state:
  `$(mic) Murmur` idle, `$(record) Listening` while recording,
  `$(loading~spin) Thinking` while a phrase is transcribed, and
  `$(debug-disconnect) Murmur` when disconnected. Clicking it toggles
  dictation (or retries the connection when disconnected).
- **Live preview**: while dictating, the latest partial text appears next to
  the status text (truncated to ~40 chars) and in full in the tooltip.
- **Commands**: `Murmur: Toggle Dictation` and `Murmur: Reconnect`.
- **Automatic reconnect** with capped backoff (2s doubling to 30s). The
  discovery file is re-read on every attempt because the port and token
  rotate on each Murmur start.
- The toggle is briefly ignored after use because Murmur debounces recording
  toggles within 500ms.

## Optional: insert final phrases at the cursor

The `murmur.insertFinalPhrases` setting (default **off**) inserts each final
dictated phrase at the active editor's cursor.

**Warning (double-typing):** Murmur itself already types into the focused
window. Enabling this setting while Murmur's normal keyboard output is active
will insert every phrase **twice**. It exists for users who point Murmur's
per-app profile away from VS Code and want insertion through the editor API
instead. Leaving it off is the safe reference behavior.

## Development

- `npm run build` compiles `src/` to `out/` with plain `tsc` (strict mode).
- `npm test` builds and runs the unit tests (`node --test`), which cover the
  protocol parsing (`src/protocol.ts`) and the connection state machine
  (`src/client.ts`) against fake sockets and timers; no network involved.
- `src/extension.ts` is the only file touching the VS Code API.

Protocol details live in [`docs/local-api.md`](../../docs/local-api.md).
