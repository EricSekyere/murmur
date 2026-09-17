const assert = require('node:assert/strict');
const { readFileSync } = require('node:fs');
const { join } = require('node:path');
const { test } = require('node:test');
const vm = require('node:vm');

class Element {
  constructor() {
    this.children = [];
    this.listeners = {};
    this.attributes = {};
    this.textContent = '';
  }
  set innerHTML(value) { this.html = value; this.children = []; }
  get innerHTML() { return this.html ?? this.textContent; }
  appendChild(child) { this.children.push(child); }
  addEventListener(name, handler) { this.listeners[name] = handler; }
  setAttribute(name, value) { this.attributes[name] = value; }
  getAttribute(name) { return this.attributes[name]; }
  querySelector() { return null; }
}

function harness(invoke = async () => ({ entries: [] })) {
  const handlers = {};
  const copied = [];
  const context = vm.createContext({
    console: { error() {}, warn() {} },
    document: { createElement: () => new Element() },
    navigator: { clipboard: { writeText: async text => copied.push(text) } },
    invoke,
    listen: (name, handler) => { handlers[name] = handler; },
    setTimeout: () => 1,
    clearTimeout() {},
    startDurationTimer() {}, stopDurationTimer() {},
    startVisualization() {}, stopVisualization() {},
    showToast() {}, finalizeSessionAnalytics() {},
    copyToClipboard: text => copied.push(text),
    history: [], historyQuery: '', uiState: 'idle',
    lastTranscription: '', sessionPhrases: [], interimText: '', currentSession: null,
    lastTranscriptionError: '', lastTranscriptionErrorAt: 0,
  });
  for (const name of ['historyList', 'historyCount', 'historyToggle', 'historyControls',
    'historySearch', 'historyClear', 'copyTranscription', 'transcriptionOutput', 'wordCount', 'procTime']) {
    context[name] = new Element();
  }
  context.applyState = state => { context.uiState = state; };
  vm.runInContext(readFileSync(join(__dirname, '../frontend/js/transcription.js'), 'utf8'), context);
  return {
    context, copied,
    emit: (name, payload = {}) => handlers[name]({ payload }),
    run: source => vm.runInContext(source, context),
  };
}

const flush = () => new Promise(resolve => setImmediate(resolve));

test('history loads on opening the app and copies the complete saved text', async () => {
  const text = 'A long saved transcript that must remain available in its entirety. '.repeat(4);
  const app = harness(async () => ({ entries: [{ text, timestamp_ms: Date.now() }] }));
  await flush();
  const row = app.context.historyList.children[0];
  assert.equal(row.children[0].textContent, text);
  row.children.at(-1).listeners.click();
  assert.equal(app.copied[0], text);
});

for (const outputFails of [false, true]) {
  test(`transcript and live history accumulate${outputFails ? ' through output errors' : ''}`, async () => {
    const entries = [];
    const app = harness(async () => ({ entries: [...entries] }));
    await flush();
    app.emit('recording-state', { recording: true, processing: false });
    for (const text of ['First phrase.', 'Second phrase.']) {
      entries.unshift({ text, timestamp_ms: Date.now() });
      app.emit('streaming-phrase', { text, processing_time_ms: 50 });
      if (outputFails) {
        app.emit('transcription-error', { error: 'Could not insert text into the other app.' });
      }
      app.emit('recording-state', { recording: true, processing: false });
      await flush();
    }
    assert.equal(app.context.lastTranscription, 'First phrase. Second phrase.');
    assert.equal(app.context.history.length, 2);
    assert.equal(app.context.uiState, 'recording');
    app.emit('recording-state', { recording: false, processing: true });
    app.emit('streaming-done');
    assert.equal(app.context.lastTranscription, 'First phrase. Second phrase.');
    app.context.copyTranscription.listeners.click();
    await flush();
    assert.equal(app.copied[0], 'First phrase. Second phrase.');
  });
}

test('copy preserves dictated paragraph breaks', () => {
  const app = harness();
  app.emit('streaming-phrase', { text: 'First paragraph.' });
  app.emit('voice-command', { command: 'new paragraph' });
  app.emit('streaming-phrase', { text: 'Second paragraph.' });
  assert.equal(app.context.lastTranscription, 'First paragraph.\n\nSecond paragraph.');
});

test('older history responses cannot replace newer results', async () => {
  const requests = [];
  const app = harness(() => new Promise(resolve => requests.push(resolve)));
  const latest = app.run('loadHistory()');
  requests[1]({ entries: [{ text: 'Newest', timestamp_ms: Date.now() }] });
  await latest;
  requests[0]({ entries: [] });
  await flush();
  assert.equal(app.context.history[0].text, 'Newest');
});

test('a failed refresh retains the last successfully loaded history', async () => {
  let fail = false;
  const app = harness(async () => {
    if (fail) throw new Error('Temporary IPC failure');
    return { entries: [{ text: 'Saved words', timestamp_ms: Date.now() }] };
  });
  await flush();
  fail = true;
  await app.run('loadHistory()');
  assert.equal(app.context.history[0].text, 'Saved words');
});

test('clearing history invalidates a pending refresh', async () => {
  let resolveHistory;
  const app = harness(command => command === 'clear_history'
    ? Promise.resolve()
    : new Promise(resolve => { resolveHistory = resolve; }));
  await app.run('clearHistory()');
  resolveHistory({ entries: [{ text: 'Deleted words', timestamp_ms: Date.now() }] });
  await flush();
  assert.equal(app.context.history.length, 0);
});

test('session completion refreshes history even after scratch removes the preview', async () => {
  let calls = 0;
  const app = harness(async () => { calls++; return { entries: [] }; });
  await flush();
  app.emit('streaming-phrase', { text: 'Saved phrase.' });
  app.emit('voice-command', { command: 'scratch that' });
  assert.equal(app.context.lastTranscription, '');
  const beforeDone = calls;
  app.emit('streaming-done');
  await flush();
  assert.equal(calls, beforeDone + 1);
});
