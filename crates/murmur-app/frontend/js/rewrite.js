// Command Mode selection rewrite: pick a style, click,
// and the backend copies the selection from the previously focused app,
// rewrites it with the local LLM, and pastes the result over the selection.
// Loaded after dom.js (invoke) and ui.js (showToast).

(function () {
  const modeSelect = document.getElementById('rewrite-mode-select');
  const button = document.getElementById('rewrite-selection-btn');
  const status = document.getElementById('rewrite-status');
  if (!modeSelect || !button) return;

  const idleHint = status ? status.textContent.trim() : '';
  // Guards double-activation while a rewrite invoke is in flight.
  let busy = false;

  function setStatus(text) {
    if (status) status.textContent = text;
  }

  async function doRewrite() {
    if (busy) return;
    busy = true;
    button.disabled = true;
    setStatus('Rewriting selection… keep the target window as it is.');
    try {
      const outcome = await invoke('rewrite_selection', { mode: modeSelect.value });
      const kind = outcome && outcome.kind;
      if (kind === 'rewritten') {
        showToast('Selection rewritten', 'success');
      } else if (kind === 'no_selection') {
        showToast('Nothing selected. Select text in the target app, then try again.', 'error');
      } else if (kind === 'unavailable') {
        showToast(outcome.reason || 'Rewrite is not available in this build.', 'error', 6000);
      }
    } catch (err) {
      showToast(`Rewrite failed: ${err}`, 'error');
    } finally {
      busy = false;
      button.disabled = false;
      setStatus(idleHint);
    }
  }

  button.addEventListener('click', doRewrite);

  // Clean up event listeners if the window is torn down.
  window.addEventListener('beforeunload', () => {
    button.removeEventListener('click', doRewrite);
  });
})();
