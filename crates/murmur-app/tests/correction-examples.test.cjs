const assert = require('node:assert/strict');
const { readFileSync } = require('node:fs');
const { join } = require('node:path');
const { test } = require('node:test');
const vm = require('node:vm');

class Element {
  constructor(tag) { this.tag = tag; this.children = []; this.listeners = {}; }
  append(...items) { this.children.push(...items); }
  appendChild(item) { this.append(item); }
  addEventListener(name, fn) { this.listeners[name] = fn; }
  setAttribute() {}
  showModal() {}
  focus() {}
  close() { this.listeners.close?.(); }
  remove() { this.removed = true; }
}

function harness(invoke) {
  const body = new Element('body');
  const manage = new Element('button');
  const context = vm.createContext({
    document: { body, createElement: tag => new Element(tag), getElementById: () => manage },
    invoke, showToast() {},
  });
  vm.runInContext(readFileSync(join(__dirname, '../frontend/js/correction-examples.js'), 'utf8'), context);
  return { body, manage, open: () => vm.runInContext('openCorrectionExample({ id: "retained-id", text: "do delete it" })', context) };
}

test('opening and closing the editor never saves audio; Save uses the reviewed text once', async () => {
  const calls = [];
  const app = harness(async (command, args) => { calls.push({ command, args }); return 'local/example'; });
  app.open();
  let dialog = app.body.children.at(-1);
  dialog.children.at(-1).listeners.click();
  assert.equal(calls.length, 0);
  app.open();
  dialog = app.body.children.at(-1);
  dialog.children.find(e => e.tag === 'textarea').value = 'do not delete it';
  const save = dialog.children.find(e => e.textContent === 'Save audio and correction locally');
  const first = save.listeners.click();
  await save.listeners.click();
  await first;
  assert.equal(calls.length, 1);
  assert.equal(calls[0].command, 'save_correction_example');
  assert.equal(calls[0].args.entryId, 'retained-id');
  assert.equal(calls[0].args.expected, 'do not delete it');
});

test('saving failure leaves the reviewed text available for retry', async () => {
  const app = harness(async () => { throw new Error('Audio expired'); });
  app.open();
  const dialog = app.body.children.at(-1);
  const input = dialog.children.find(e => e.tag === 'textarea');
  input.value = 'corrected';
  const save = dialog.children.find(e => e.textContent === 'Save audio and correction locally');
  await save.listeners.click();
  assert.equal(input.value, 'corrected');
  assert.equal(input.disabled, false);
  assert.equal(save.disabled, false);
  assert.notEqual(dialog.removed, true);
});
