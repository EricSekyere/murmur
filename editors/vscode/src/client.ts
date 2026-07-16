// Connection state machine for Murmur's local API. All effects (discovery
// reads, sockets, timers) are injected so the machine tests without a network.

import {
  buildAuthFrame,
  buildRequestFrame,
  createIdSource,
  parseDiscovery,
  parseServerMessage,
} from './protocol';

export type ConnectionState = 'disconnected' | 'connecting' | 'connected';

/** Minimal surface of the WHATWG WebSocket the client relies on. */
export interface SocketLike {
  send(data: string): void;
  close(): void;
  onopen: (() => void) | null;
  onmessage: ((event: { data: unknown }) => void) | null;
  onclose: (() => void) | null;
  onerror: (() => void) | null;
}

export interface ClientCallbacks {
  onState(state: ConnectionState): void;
  onEvent(name: string, payload: unknown): void;
  /** Server closed before `ready` after auth was sent: stale discovery token. */
  onAuthRejected(): void;
}

export interface ClientDeps {
  /** Discovery file contents, or null when absent (API off / app not running). */
  readDiscovery(): Promise<string | null>;
  createSocket(url: string): SocketLike;
  schedule(fn: () => void, delayMs: number): unknown;
  cancel(handle: unknown): void;
  callbacks: ClientCallbacks;
}

const INITIAL_BACKOFF_MS = 2_000;
const MAX_BACKOFF_MS = 30_000;

interface Pending {
  resolve(value: unknown): void;
  reject(reason: Error): void;
}

export class MurmurClient {
  private readonly nextId = createIdSource();
  private readonly pending = new Map<number, Pending>();
  private socket: SocketLike | null = null;
  private ready = false;
  private authSent = false;
  private backoffMs = INITIAL_BACKOFF_MS;
  private retryHandle: unknown = null;
  // Guards stale async work (discovery reads) after a reconnect or dispose.
  private generation = 0;
  private disposed = false;

  constructor(private readonly deps: ClientDeps) {}

  start(): void {
    void this.attempt();
  }

  /** Drop any current socket or pending retry and connect immediately. */
  reconnectNow(): void {
    if (this.disposed) {
      return;
    }
    this.backoffMs = INITIAL_BACKOFF_MS;
    this.clearRetry();
    this.dropSocket();
    void this.attempt();
  }

  request(method: string): Promise<unknown> {
    const socket = this.socket;
    if (!this.ready || !socket) {
      return Promise.reject(new Error('not connected'));
    }
    const id = this.nextId();
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      socket.send(buildRequestFrame(id, method));
    });
  }

  dispose(): void {
    this.disposed = true;
    this.generation++;
    this.clearRetry();
    this.dropSocket();
  }

  private async attempt(): Promise<void> {
    if (this.disposed || this.socket) {
      return;
    }
    const generation = ++this.generation;
    let text: string | null;
    try {
      text = await this.deps.readDiscovery();
    } catch {
      text = null;
    }
    if (this.disposed || generation !== this.generation || this.socket) {
      return;
    }
    // Port and token rotate every app start, so parse fresh on every attempt.
    const discovery = text === null ? null : parseDiscovery(text);
    if (!discovery) {
      this.deps.callbacks.onState('disconnected');
      this.scheduleRetry();
      return;
    }
    let socket: SocketLike;
    try {
      socket = this.deps.createSocket(`ws://127.0.0.1:${discovery.port}/`);
    } catch {
      this.deps.callbacks.onState('disconnected');
      this.scheduleRetry();
      return;
    }
    this.socket = socket;
    this.ready = false;
    this.authSent = false;
    this.deps.callbacks.onState('connecting');
    socket.onopen = () => {
      if (this.socket !== socket) {
        return;
      }
      this.authSent = true;
      socket.send(buildAuthFrame(discovery.token));
    };
    socket.onmessage = (event) => {
      if (this.socket === socket && typeof event.data === 'string') {
        this.handleFrame(event.data);
      }
    };
    socket.onclose = () => this.handleClose(socket);
    // An error is always followed by close; close drives the retry.
    socket.onerror = () => {};
  }

  private handleFrame(text: string): void {
    const message = parseServerMessage(text);
    if (!message) {
      return;
    }
    if (!this.ready) {
      if (message.type === 'ready') {
        this.ready = true;
        this.backoffMs = INITIAL_BACKOFF_MS;
        this.deps.callbacks.onState('connected');
      }
      return;
    }
    if (message.type === 'event') {
      this.deps.callbacks.onEvent(message.name, message.payload);
      return;
    }
    if (message.type === 'response' && typeof message.id === 'number') {
      const pending = this.pending.get(message.id);
      if (!pending) {
        return;
      }
      this.pending.delete(message.id);
      if (message.error !== undefined) {
        pending.reject(new Error(message.error));
      } else {
        pending.resolve(message.result);
      }
    }
    // Frame-level `error` messages signal a client bug; nothing to route.
  }

  private handleClose(socket: SocketLike): void {
    if (this.socket !== socket) {
      return;
    }
    const authRejected = this.authSent && !this.ready;
    this.dropSocket();
    if (this.disposed) {
      return;
    }
    if (authRejected) {
      this.deps.callbacks.onAuthRejected();
    }
    this.deps.callbacks.onState('disconnected');
    this.scheduleRetry();
  }

  private scheduleRetry(): void {
    if (this.disposed || this.retryHandle !== null) {
      return;
    }
    const delay = this.backoffMs;
    this.backoffMs = Math.min(this.backoffMs * 2, MAX_BACKOFF_MS);
    this.retryHandle = this.deps.schedule(() => {
      this.retryHandle = null;
      void this.attempt();
    }, delay);
  }

  private clearRetry(): void {
    if (this.retryHandle !== null) {
      this.deps.cancel(this.retryHandle);
      this.retryHandle = null;
    }
  }

  private dropSocket(): void {
    this.rejectPending(new Error('connection closed'));
    const socket = this.socket;
    if (!socket) {
      return;
    }
    this.socket = null;
    this.ready = false;
    this.authSent = false;
    socket.onopen = null;
    socket.onmessage = null;
    socket.onclose = null;
    socket.onerror = null;
    try {
      socket.close();
    } catch {
      // Already closed.
    }
  }

  private rejectPending(reason: Error): void {
    for (const pending of this.pending.values()) {
      pending.reject(reason);
    }
    this.pending.clear();
  }
}
