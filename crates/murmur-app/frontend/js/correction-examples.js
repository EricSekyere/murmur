// Only an explicit Save click persists a selected utterance.
function exampleDialog(title) {
  const dialog = document.createElement('dialog');
  dialog.className = 'correction-dialog';
  const heading = document.createElement('h2');
  heading.textContent = title;
  heading.id = 'correction-dialog-title';
  dialog.setAttribute('aria-labelledby', heading.id);
  dialog.appendChild(heading);
  const close = document.createElement('button');
  close.className = 'btn-small';
  close.textContent = 'Close';
  close.addEventListener('click', () => dialog.close());
  dialog.addEventListener('close', () => dialog.remove(), { once: true });
  document.body.appendChild(dialog);
  return { dialog, close };
}

function openCorrectionExample(entry) {
  const { dialog, close } = exampleDialog('Save a correction example');
  const explanation = document.createElement('p');
  explanation.textContent = 'Save this audio clip and your corrected text on this computer for accuracy testing. Nothing is uploaded or learned automatically. Saved examples remain until you delete them under Saved examples, even if you clear history.';
  const label = document.createElement('label');
  label.textContent = 'What you actually said';
  label.htmlFor = 'corrected-example-text';
  const input = document.createElement('textarea');
  input.id = label.htmlFor;
  input.className = 'settings-textarea';
  input.rows = 5;
  input.maxLength = 8000;
  input.value = entry.text;
  const save = document.createElement('button');
  save.className = 'btn-small';
  save.textContent = 'Save audio and correction locally';
  let busy = false;
  dialog.addEventListener('cancel', event => { if (busy) event.preventDefault(); });
  save.addEventListener('click', async () => {
    if (busy || !input.value.trim()) return;
    busy = true;
    save.disabled = close.disabled = input.disabled = true;
    try {
      const path = await invoke('save_correction_example', { entryId: entry.id, expected: input.value.trim() });
      showToast(`Example saved locally: ${path}`, 'success');
      dialog.close();
    } catch (err) {
      showToast(`Could not save example: ${err}`, 'error');
    } finally {
      busy = false;
      save.disabled = close.disabled = input.disabled = false;
    }
  });
  dialog.append(explanation, label, input, save, close);
  dialog.showModal();
  input.focus();
}

document.getElementById('examples-manage').addEventListener('click', async () => {
  try {
    const { directory, examples } = await invoke('list_correction_examples');
    const { dialog, close } = exampleDialog('Saved correction examples');
    const location = document.createElement('p');
    location.textContent = `Stored locally in ${directory}. These files remain when history is cleared.`;
    dialog.appendChild(location);
    const list = document.createElement('ul');
    if (!examples.length) {
      const empty = document.createElement('li');
      empty.textContent = 'No saved examples.';
      list.appendChild(empty);
    }
    for (const example of examples) {
      const row = document.createElement('li');
      const text = document.createElement('span');
      text.textContent = example.expected;
      const remove = document.createElement('button');
      remove.className = 'btn-small';
      remove.textContent = 'Delete audio and text';
      remove.addEventListener('click', async () => {
        remove.disabled = true;
        try {
          await invoke('delete_correction_example', { id: example.id });
          row.remove();
        } catch (err) {
          remove.disabled = false;
          showToast(`Could not delete example: ${err}`, 'error');
        }
      });
      row.append(text, remove);
      list.appendChild(row);
    }
    dialog.append(list, close);
    dialog.showModal();
  } catch (err) {
    showToast(`Could not load examples: ${err}`, 'error');
  }
});
