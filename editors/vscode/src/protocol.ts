// Pure helpers for Murmur's local WebSocket API (see docs/local-api.md).
// No vscode or network imports: everything here is testable strings and JSON.

export interface Discovery {
  port: number;
  token: string;
}

export type ServerMessage =
  | { type: 'ready' }
  | { type: 'event'; name: string; payload: unknown }
  | { type: 'response'; id: unknown; result?: unknown; error?: string }
  | { type: 'error'; error: string };

export interface RecordingState {
  recording: boolean;
  processing: boolean;
}

function asRecord(value: unknown): Record<string, unknown> | null {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null;
}

function parseJson(text: string): unknown {
  try {
    return JSON.parse(text);
  } catch {
    return undefined;
  }
}

/** Parse one server frame; null for anything that isn't a documented shape. */
export function parseServerMessage(text: string): ServerMessage | null {
  const obj = asRecord(parseJson(text));
  if (!obj) {
    return null;
  }
  switch (obj['type']) {
    case 'ready':
      return { type: 'ready' };
    case 'event': {
      const name = obj['name'];
      if (typeof name !== 'string' || !('payload' in obj)) {
        return null;
      }
      return { type: 'event', name, payload: obj['payload'] };
    }
    case 'response': {
      const error = obj['error'];
      if (error !== undefined && typeof error !== 'string') {
        return null;
      }
      // A response carries exactly one of result/error; anything else is malformed.
      if ('result' in obj === (error !== undefined)) {
        return null;
      }
      return { type: 'response', id: obj['id'] ?? null, result: obj['result'], error };
    }
    case 'error': {
      const error = obj['error'];
      return typeof error === 'string' ? { type: 'error', error } : null;
    }
    default:
      return null;
  }
}

/** Validate the discovery file (`{"port": u16, "token": "hex"}`); null if unusable. */
export function parseDiscovery(text: string): Discovery | null {
  const obj = asRecord(parseJson(text));
  if (!obj) {
    return null;
  }
  const port = obj['port'];
  const token = obj['token'];
  if (typeof port !== 'number' || !Number.isInteger(port) || port < 1 || port > 65535) {
    return null;
  }
  if (typeof token !== 'string' || token.length === 0) {
    return null;
  }
  return { port, token };
}

export function buildAuthFrame(token: string): string {
  return JSON.stringify({ type: 'auth', token });
}

export function buildRequestFrame(id: number, method: string): string {
  return JSON.stringify({ type: 'request', id, method });
}

/** Monotonic request-id source, one per connection-independent client. */
export function createIdSource(): () => number {
  let next = 1;
  return () => next++;
}

/** `recording-state` payload (also the `get_status` result); null if malformed. */
export function parseRecordingState(value: unknown): RecordingState | null {
  const obj = asRecord(value);
  if (!obj || typeof obj['recording'] !== 'boolean' || typeof obj['processing'] !== 'boolean') {
    return null;
  }
  return { recording: obj['recording'], processing: obj['processing'] };
}

/** `text` field of streaming-partial / streaming-phrase payloads. */
export function parseTextPayload(value: unknown): string | null {
  const obj = asRecord(value);
  const text = obj?.['text'];
  return typeof text === 'string' ? text : null;
}

export function truncate(text: string, max: number): string {
  return text.length <= max ? text : `${text.slice(0, Math.max(0, max - 3))}...`;
}
