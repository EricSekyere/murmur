import assert from 'node:assert/strict';
import { test } from 'node:test';
import { MurmurClient, type ConnectionState, type SocketLike } from '../src/client';

const TOKEN = '3f9c0a1b8d2e4c6f9a0b1c2d3e4f5a6b';

class FakeSocket implements SocketLike {
  readonly sent: string[] = [];
  closed = false;
  onopen: (() => void) | null = null;
  onmessage: ((event: { data: unknown }) => void) | null = null;
  onclose: (() => void) | null = null;
  onerror: (() => void) | null = null;

  constructor(readonly url: string) {}

  send(data: string): void {
    this.sent.push(data);
  }

  close(): void {
    this.closed = true;
  }

  serverOpen(): void {
    this.onopen?.();
  }

  serverSend(text: string): void {
    this.onmessage?.({ data: text });
  }

  serverClose(): void {
    this.onclose?.();
  }
}

interface ScheduledTimer {
  fn: () => void;
  delay: number;
}

class FakeTimers {
  readonly scheduled: ScheduledTimer[] = [];

  schedule = (fn: () => void, delay: number): unknown => {
    const entry: ScheduledTimer = { fn, delay };
    this.scheduled.push(entry);
    return entry;
  };

  cancel = (handle: unknown): void => {
    const index = this.scheduled.indexOf(handle as ScheduledTimer);
    if (index >= 0) {
      this.scheduled.splice(index, 1);
    }
  };

  fireNext(): number {
    const entry = this.scheduled.shift();
    if (!entry) {
      throw new Error('nothing scheduled');
    }
    entry.fn();
    return entry.delay;
  }
}

// Flush the readDiscovery await inside MurmurClient.attempt().
const tick = () => new Promise<void>((resolve) => setImmediate(resolve));

function harness(discovery: () => string | null = () => JSON.stringify({ port: 4242, token: TOKEN })) {
  const sockets: FakeSocket[] = [];
  const timers = new FakeTimers();
  const states: ConnectionState[] = [];
  const events: { name: string; payload: unknown }[] = [];
  let authRejections = 0;
  let discoveryReads = 0;
  const client = new MurmurClient({
    readDiscovery: async () => {
      discoveryReads++;
      return discovery();
    },
    createSocket: (url) => {
      const socket = new FakeSocket(url);
      sockets.push(socket);
      return socket;
    },
    schedule: timers.schedule,
    cancel: timers.cancel,
    callbacks: {
      onState: (state) => states.push(state),
      onEvent: (name, payload) => events.push({ name, payload }),
      onAuthRejected: () => authRejections++,
    },
  });
  return {
    client,
    sockets,
    timers,
    states,
    events,
    authRejections: () => authRejections,
    discoveryReads: () => discoveryReads,
  };
}

async function connectHappy(h: ReturnType<typeof harness>): Promise<FakeSocket> {
  h.client.start();
  await tick();
  const socket = h.sockets[h.sockets.length - 1]!;
  socket.serverOpen();
  socket.serverSend('{"type":"ready"}');
  return socket;
}

test('happy path: discovery, auth, ready, events', async () => {
  const h = harness();
  const socket = await connectHappy(h);

  assert.equal(socket.url, 'ws://127.0.0.1:4242/');
  assert.deepEqual(JSON.parse(socket.sent[0]!), { type: 'auth', token: TOKEN });
  assert.deepEqual(h.states, ['connecting', 'connected']);

  socket.serverSend('{"type":"event","name":"streaming-partial","payload":{"text":"hel"}}');
  assert.deepEqual(h.events, [{ name: 'streaming-partial', payload: { text: 'hel' } }]);
});

test('requests correlate responses by id, even out of order', async () => {
  const h = harness();
  const socket = await connectHappy(h);

  const first = h.client.request('get_status');
  const second = h.client.request('toggle_recording');
  const firstId = JSON.parse(socket.sent[1]!).id as number;
  const secondId = JSON.parse(socket.sent[2]!).id as number;

  socket.serverSend(JSON.stringify({ type: 'response', id: secondId, result: { ok: true } }));
  socket.serverSend(
    JSON.stringify({ type: 'response', id: firstId, result: { recording: true, processing: false } }),
  );
  assert.deepEqual(await second, { ok: true });
  assert.deepEqual(await first, { recording: true, processing: false });
});

test('an error response rejects its request only', async () => {
  const h = harness();
  const socket = await connectHappy(h);

  const bad = h.client.request('reboot');
  const good = h.client.request('get_status');
  const badId = JSON.parse(socket.sent[1]!).id as number;
  const goodId = JSON.parse(socket.sent[2]!).id as number;

  socket.serverSend(JSON.stringify({ type: 'response', id: badId, error: 'unknown method: reboot' }));
  socket.serverSend(
    JSON.stringify({ type: 'response', id: goodId, result: { recording: false, processing: false } }),
  );
  await assert.rejects(bad, /unknown method: reboot/);
  assert.deepEqual(await good, { recording: false, processing: false });
});

test('requests before ready are rejected', () => {
  const h = harness();
  return assert.rejects(h.client.request('get_status'), /not connected/);
});

test('close after auth but before ready reports a rejected token and retries', async () => {
  const h = harness();
  h.client.start();
  await tick();
  const socket = h.sockets[0]!;
  socket.serverOpen();
  socket.serverClose();

  assert.equal(h.authRejections(), 1);
  assert.deepEqual(h.states, ['connecting', 'disconnected']);
  assert.equal(h.timers.scheduled.length, 1);
  assert.equal(h.timers.scheduled[0]!.delay, 2000);
});

test('close before auth was sent is not an auth rejection', async () => {
  const h = harness();
  h.client.start();
  await tick();
  h.sockets[0]!.serverClose();

  assert.equal(h.authRejections(), 0);
  assert.equal(h.timers.scheduled.length, 1);
});

test('backoff doubles from 2s and caps at 30s while discovery is absent', async () => {
  const h = harness(() => null);
  h.client.start();
  await tick();

  const delays: number[] = [];
  for (let i = 0; i < 6; i++) {
    assert.equal(h.timers.scheduled.length, 1);
    delays.push(h.timers.fireNext());
    await tick();
  }
  assert.deepEqual(delays, [2000, 4000, 8000, 16000, 30000, 30000]);
  assert.equal(h.sockets.length, 0);
});

test('backoff resets once a connection reaches ready', async () => {
  let available = false;
  const h = harness(() => (available ? JSON.stringify({ port: 4242, token: TOKEN }) : null));
  h.client.start();
  await tick();
  h.timers.fireNext();
  await tick();
  // Next retry would be 8s; a successful connect must reset that.
  available = true;
  h.timers.fireNext();
  await tick();
  const socket = h.sockets[0]!;
  socket.serverOpen();
  socket.serverSend('{"type":"ready"}');

  socket.serverClose();
  assert.equal(h.timers.scheduled[0]!.delay, 2000);
});

test('discovery is re-read on every attempt so rotated ports are picked up', async () => {
  let port = 1111;
  const h = harness(() => JSON.stringify({ port, token: TOKEN }));
  h.client.start();
  await tick();
  assert.equal(h.sockets[0]!.url, 'ws://127.0.0.1:1111/');

  // App restarted: old socket dies, new discovery advertises a new port.
  port = 2222;
  h.sockets[0]!.serverClose();
  h.timers.fireNext();
  await tick();

  assert.equal(h.discoveryReads(), 2);
  assert.equal(h.sockets[1]!.url, 'ws://127.0.0.1:2222/');
});

test('reconnectNow cancels the pending retry, resets backoff, and retries at once', async () => {
  const h = harness(() => null);
  h.client.start();
  await tick();
  h.timers.fireNext();
  await tick();
  h.timers.fireNext();
  await tick();
  // Escalated to 8s by now; reconnectNow must start over immediately.
  h.client.reconnectNow();
  await tick();

  assert.equal(h.timers.scheduled.length, 1);
  assert.equal(h.timers.scheduled[0]!.delay, 2000);
  assert.equal(h.discoveryReads(), 4);
});

test('dispose closes the socket, cancels timers, and rejects pending requests', async () => {
  const h = harness();
  const socket = await connectHappy(h);
  const pending = h.client.request('get_status');

  h.client.dispose();
  await assert.rejects(pending, /connection closed/);
  assert.ok(socket.closed);
  assert.equal(h.timers.scheduled.length, 0);

  // A close event after dispose must not schedule anything or change state.
  const statesBefore = h.states.length;
  socket.serverClose();
  assert.equal(h.timers.scheduled.length, 0);
  assert.equal(h.states.length, statesBefore);
});

test('a stale socket cannot interfere after reconnectNow', async () => {
  const h = harness();
  const stale = await connectHappy(h);
  h.client.reconnectNow();
  await tick();
  const fresh = h.sockets[1]!;

  assert.ok(stale.closed);
  stale.serverSend('{"type":"event","name":"streaming-done","payload":{}}');
  assert.equal(h.events.length, 0);

  fresh.serverOpen();
  fresh.serverSend('{"type":"ready"}');
  assert.equal(h.states[h.states.length - 1], 'connected');
});
