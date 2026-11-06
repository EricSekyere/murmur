const assert = require('node:assert/strict');
const { readFileSync } = require('node:fs');
const { join } = require('node:path');
const { test } = require('node:test');
const vm = require('node:vm');

test('reviewed corrections reject incomplete rows and preserve unsaved edits during a save', async () => {
  const input = { value: '', listeners: {}, addEventListener(name, fn) { this.listeners[name] = fn; } };
  const button = { disabled: true, listeners: {}, addEventListener(name, fn) { this.listeners[name] = fn; } };
  const calls = [];
  let complete;
  const context = vm.createContext({
    document: { getElementById: id => id === 'corrections-input' ? input : button },
    invoke: (command, args) => { calls.push({ command, args }); return new Promise(resolve => { complete = resolve; }); },
    showToast() {},
  });
  vm.runInContext(readFileSync(join(__dirname, '../frontend/js/corrections.js'), 'utf8'), context);
  input.value = 'broken rule';
  await button.listeners.click();
  assert.equal(calls.length, 0);
  assert.equal(button.disabled, false);
  input.value = 'open super base => open Supabase';
  const saving = button.listeners.click();
  assert.equal(calls[0].command, 'update_settings');
  assert.equal(calls[0].args.vocabulary_replacements[0].heard, 'open super base');
  assert.equal(calls[0].args.vocabulary_replacements[0].replacement, 'open Supabase');
  input.value = '';
  input.listeners.input();
  complete();
  await saving;
  assert.equal(input.value, '');
  assert.equal(button.disabled, false);
});
