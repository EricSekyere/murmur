// VS Code glue: discovery path, status bar, commands, and settings around the
// MurmurClient state machine. Protocol and connection logic live elsewhere.

import { promises as fs } from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import * as vscode from 'vscode';
import { MurmurClient, type ConnectionState, type SocketLike } from './client';
import { parseRecordingState, parseTextPayload, truncate } from './protocol';

const PREVIEW_MAX_CHARS = 40;
// Murmur silently drops a second toggle within 500ms; hold ours slightly longer.
const TOGGLE_COOLDOWN_MS = 600;

// The extension host's built-in WebSocket (Node 22 / undici). Resolved via
// globalThis so compilation doesn't depend on @types/node declaring the global.
const WebSocketCtor = (globalThis as Record<string, unknown>)['WebSocket'] as
  | (new (url: string) => SocketLike)
  | undefined;

function discoveryPath(): string {
  if (process.platform === 'win32') {
    const appData = process.env['APPDATA'] ?? path.join(os.homedir(), 'AppData', 'Roaming');
    return path.join(appData, 'murmur', 'local-api.json');
  }
  if (process.platform === 'darwin') {
    return path.join(os.homedir(), 'Library', 'Application Support', 'murmur', 'local-api.json');
  }
  const configHome = process.env['XDG_CONFIG_HOME'] ?? path.join(os.homedir(), '.config');
  return path.join(configHome, 'murmur', 'local-api.json');
}

function insertAtCursor(text: string): void {
  const enabled = vscode.workspace
    .getConfiguration('murmur')
    .get<boolean>('insertFinalPhrases', false);
  const editor = vscode.window.activeTextEditor;
  if (!enabled || !editor) {
    return;
  }
  void editor.edit((edit) => edit.insert(editor.selection.active, text));
}

export function activate(context: vscode.ExtensionContext): void {
  const status = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Left);
  status.command = 'murmur.toggleDictation';
  context.subscriptions.push(status);

  if (!WebSocketCtor) {
    status.text = '$(debug-disconnect) Murmur';
    status.tooltip =
      'Murmur needs a VS Code whose extension host provides the WebSocket global (Node 22+).';
    status.show();
    return;
  }

  let state: ConnectionState = 'disconnected';
  let recording = false;
  let processing = false;
  let preview = '';
  let warnedAuthRejected = false;
  let toggleLockedUntil = 0;

  const render = (): void => {
    if (state !== 'connected') {
      status.text = '$(debug-disconnect) Murmur';
      status.tooltip =
        'Murmur is not connected. Start Murmur with the local API enabled, or run "Murmur: Reconnect". Click to retry.';
    } else if (processing) {
      status.text = '$(loading~spin) Thinking';
      status.tooltip = preview || 'Murmur is transcribing';
    } else if (recording) {
      const suffix = preview ? ` ${truncate(preview, PREVIEW_MAX_CHARS)}` : '';
      status.text = `$(record) Listening${suffix}`;
      status.tooltip = preview || 'Murmur is listening. Click to stop.';
    } else {
      status.text = '$(mic) Murmur';
      status.tooltip = 'Click to start dictation';
    }
    status.show();
  };

  const applyStatus = (payload: unknown): void => {
    const parsed = parseRecordingState(payload);
    if (parsed) {
      recording = parsed.recording;
      processing = parsed.processing;
      render();
    }
  };

  const client = new MurmurClient({
    readDiscovery: async () => {
      try {
        return await fs.readFile(discoveryPath(), 'utf8');
      } catch {
        return null;
      }
    },
    createSocket: (url) => new WebSocketCtor(url),
    schedule: (fn, delayMs) => setTimeout(fn, delayMs),
    cancel: (handle) => clearTimeout(handle as NodeJS.Timeout),
    callbacks: {
      onState: (next) => {
        state = next;
        if (next !== 'connected') {
          recording = false;
          processing = false;
          preview = '';
        }
        render();
        if (next === 'connected') {
          client.request('get_status').then(applyStatus, () => {});
        }
      },
      onEvent: (name, payload) => {
        if (name === 'recording-state') {
          applyStatus(payload);
          return;
        }
        if (name === 'streaming-partial' || name === 'streaming-phrase') {
          const text = parseTextPayload(payload);
          if (text !== null) {
            preview = text;
            if (name === 'streaming-phrase') {
              insertAtCursor(text);
            }
          }
        } else if (name === 'streaming-done') {
          preview = '';
        }
        render();
      },
      onAuthRejected: () => {
        if (warnedAuthRejected) {
          return;
        }
        warnedAuthRejected = true;
        void vscode.window.showWarningMessage(
          'Murmur rejected this extension\'s token; the discovery file may be stale. Restart Murmur, then run "Murmur: Reconnect".',
        );
      },
    },
  });

  context.subscriptions.push(
    vscode.commands.registerCommand('murmur.toggleDictation', () => {
      if (state !== 'connected') {
        client.reconnectNow();
        return;
      }
      const now = Date.now();
      if (now < toggleLockedUntil) {
        return;
      }
      toggleLockedUntil = now + TOGGLE_COOLDOWN_MS;
      client.request('toggle_recording').catch(() => {});
    }),
    vscode.commands.registerCommand('murmur.reconnect', () => client.reconnectNow()),
    { dispose: () => client.dispose() },
  );

  render();
  client.start();
}

export function deactivate(): void {}
