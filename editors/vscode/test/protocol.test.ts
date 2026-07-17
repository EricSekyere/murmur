import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  buildAuthFrame,
  buildRequestFrame,
  createIdSource,
  parseDiscovery,
  parseRecordingState,
  parseServerMessage,
  parseTextPayload,
  truncate,
} from '../src/protocol';

test('parses the documented server frames', () => {
  assert.deepEqual(parseServerMessage('{"type":"ready"}'), { type: 'ready' });

  assert.deepEqual(
    parseServerMessage('{"type":"event","name":"streaming-partial","payload":{"text":"hel"}}'),
    { type: 'event', name: 'streaming-partial', payload: { text: 'hel' } },
  );
  // streaming-done ships an empty payload; the parser must not require fields.
  assert.deepEqual(parseServerMessage('{"type":"event","name":"streaming-done","payload":{}}'), {
    type: 'event',
    name: 'streaming-done',
    payload: {},
  });

  assert.deepEqual(
    parseServerMessage('{"type":"response","id":1,"result":{"recording":false,"processing":false}}'),
    { type: 'response', id: 1, result: { recording: false, processing: false }, error: undefined },
  );
  assert.deepEqual(parseServerMessage('{"type":"response","id":2,"error":"unknown method: reboot"}'), {
    type: 'response',
    id: 2,
    result: undefined,
    error: 'unknown method: reboot',
  });
  // The id is opaque JSON and may be null when the request omitted it.
  assert.deepEqual(parseServerMessage('{"type":"response","id":null,"result":{"ok":true}}'), {
    type: 'response',
    id: null,
    result: { ok: true },
    error: undefined,
  });

  assert.deepEqual(parseServerMessage('{"type":"error","error":"invalid JSON"}'), {
    type: 'error',
    error: 'invalid JSON',
  });
});

test('rejects malformed server frames', () => {
  for (const text of [
    '{not json',
    '"just a string"',
    '[1,2]',
    '{"no":"type"}',
    '{"type":"unknown"}',
    '{"type":"event","payload":{}}',
    '{"type":"event","name":7,"payload":{}}',
    '{"type":"event","name":"streaming-done"}',
    '{"type":"response","id":1}',
    '{"type":"response","id":1,"error":42}',
    '{"type":"response","id":1,"result":{},"error":"both"}',
    '{"type":"error"}',
    '{"type":"error","error":7}',
  ]) {
    assert.equal(parseServerMessage(text), null, `expected null for ${text}`);
  }
});

test('auth and request frames match the wire shapes', () => {
  assert.deepEqual(JSON.parse(buildAuthFrame('3f9c')), { type: 'auth', token: '3f9c' });
  assert.deepEqual(JSON.parse(buildRequestFrame(7, 'get_status')), {
    type: 'request',
    id: 7,
    method: 'get_status',
  });
});

test('id source is monotonic from 1', () => {
  const next = createIdSource();
  assert.deepEqual([next(), next(), next()], [1, 2, 3]);
  // A fresh source starts over, independent of the first.
  assert.equal(createIdSource()(), 1);
});

test('discovery file validation', () => {
  assert.deepEqual(parseDiscovery('{"port":52341,"token":"3f9c0a1b8d2e4c6f9a0b1c2d3e4f5a6b"}'), {
    port: 52341,
    token: '3f9c0a1b8d2e4c6f9a0b1c2d3e4f5a6b',
  });
  for (const text of [
    '{not json',
    '[]',
    '{}',
    '{"token":"abc"}',
    '{"port":"52341","token":"abc"}',
    '{"port":0,"token":"abc"}',
    '{"port":65536,"token":"abc"}',
    '{"port":1234.5,"token":"abc"}',
    '{"port":1234}',
    '{"port":1234,"token":""}',
    '{"port":1234,"token":42}',
  ]) {
    assert.equal(parseDiscovery(text), null, `expected null for ${text}`);
  }
});

test('recording-state and streaming payload helpers', () => {
  assert.deepEqual(parseRecordingState({ recording: true, processing: false }), {
    recording: true,
    processing: false,
  });
  assert.equal(parseRecordingState({ recording: 'yes', processing: false }), null);
  assert.equal(parseRecordingState({ recording: true }), null);
  assert.equal(parseRecordingState(null), null);

  assert.equal(parseTextPayload({ text: 'hello world.', processing_time_ms: 412 }), 'hello world.');
  assert.equal(parseTextPayload({}), null);
  assert.equal(parseTextPayload('hello'), null);
});

test('truncate caps preview length with a marker', () => {
  assert.equal(truncate('short', 40), 'short');
  const long = 'x'.repeat(60);
  const cut = truncate(long, 40);
  assert.equal(cut.length, 40);
  assert.ok(cut.endsWith('...'));
});
