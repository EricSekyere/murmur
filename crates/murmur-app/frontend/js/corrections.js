// Explicit phrase replacements; no transcript or history mining.
const correctionsInput = document.getElementById('corrections-input');
const correctionsSave = document.getElementById('corrections-save');
let correctionsVersion = 0;

function loadCorrections(rules) {
  correctionsInput.value = rules.map(rule => `${rule.heard} => ${rule.replacement}`).join('\n');
  correctionsSave.disabled = true;
}

function parseCorrections(text) {
  return text.split('\n').map(line => line.trim()).filter(Boolean).map((line, index) => {
    const separator = line.indexOf('=>');
    if (separator < 1 || !line.slice(separator + 2).trim()) {
      throw new Error(`Line ${index + 1} needs heard phrase => intended phrase.`);
    }
    return { heard: line.slice(0, separator).trim(), replacement: line.slice(separator + 2).trim() };
  });
}

correctionsInput.addEventListener('input', () => {
  correctionsVersion += 1;
  correctionsSave.disabled = false;
});
correctionsSave.addEventListener('click', async () => {
  const version = correctionsVersion;
  correctionsSave.disabled = true;
  try {
    const rules = parseCorrections(correctionsInput.value);
    await invoke('update_settings', { vocabulary_replacements: rules });
    showToast(`Saved ${rules.length} correction pairs`, 'success');
    correctionsSave.disabled = version === correctionsVersion;
  } catch (err) {
    correctionsSave.disabled = false;
    showToast(`Could not save corrections: ${err}`, 'error');
  }
});
