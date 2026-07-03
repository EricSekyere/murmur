// Command mode UI: the visible mode badge and the physical-confirm dialog.
// The dialog is the security
// gate for gated actions: it echoes the parsed tool and arguments so the
// user can see exactly what the ASR produced, and confirmation only ever
// happens through a real click or keypress here, never by voice. Loaded
// after dom.js (shared invoke/listen) and ui.js (showToast).

(function () {
  const badge = document.getElementById('command-mode-badge');
  const overlay = document.getElementById('command-confirm');
  const dialog = overlay ? overlay.querySelector('[role="dialog"]') : null;
  const toolEl = document.getElementById('command-confirm-tool');
  const argsEl = document.getElementById('command-confirm-args');
  const warningEl = document.getElementById('command-confirm-warning');
  const confirmBtn = document.getElementById('command-confirm-btn');
  const cancelBtn = document.getElementById('command-cancel-btn');
  if (!badge || !overlay || !dialog || !toolEl || !argsEl || !confirmBtn || !cancelBtn) return;

  // Where focus returns when the dialog closes.
  let lastFocused = null;
  // Guards double-activation while a confirm/cancel invoke is in flight.
  let busy = false;

  function setBadge(active) {
    badge.hidden = !active;
  }

  // Initial state (covers a webview reload while command mode is active);
  // the event below covers every later toggle.
  invoke('get_status')
    .then((status) => setBadge(!!(status && status.command_mode)))
    .catch(() => setBadge(false));

  const unlistenMode = listen('command-mode-changed', (event) => {
    setBadge(!!(event.payload && event.payload.active));
  });

  function focusables() {
    return Array.from(dialog.querySelectorAll('button:not([disabled])'));
  }

  function openDialog(pending) {
    toolEl.textContent = pending.tool || '';
    // Echo the parsed arguments verbatim; textContent keeps ASR-derived
    // text inert (never interpreted as markup).
    let rendered = '';
    try {
      rendered = JSON.stringify(pending.args === undefined ? {} : pending.args, null, 2);
    } catch (err) {
      rendered = String(pending.args);
    }
    argsEl.textContent = rendered;
    if (warningEl) warningEl.hidden = !!pending.reversible;
    lastFocused = document.activeElement;
    overlay.hidden = false;
    confirmBtn.focus();
  }

  function closeDialog() {
    overlay.hidden = true;
    if (lastFocused && typeof lastFocused.focus === 'function') lastFocused.focus();
    lastFocused = null;
  }

  async function doCancel() {
    if (busy) return;
    busy = true;
    try {
      await invoke('cancel_pending');
    } catch (err) {
      console.error('Failed to cancel pending action:', err);
    } finally {
      busy = false;
      closeDialog();
    }
  }

  async function doConfirm() {
    if (busy) return;
    busy = true;
    try {
      await invoke('confirm_pending');
      showToast('Action completed', 'success');
    } catch (err) {
      showToast(`Action failed: ${err}`, 'error');
    } finally {
      busy = false;
      closeDialog();
    }
  }

  // Focus trap + Esc-to-cancel while the dialog is open.
  function onDialogKeydown(event) {
    if (event.key === 'Escape') {
      event.preventDefault();
      doCancel();
      return;
    }
    if (event.key !== 'Tab') return;
    const items = focusables();
    if (!items.length) return;
    const first = items[0];
    const last = items[items.length - 1];
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first.focus();
    }
  }

  function onOverlayClick(event) {
    // Clicking the backdrop is a physical dismissal, same as Cancel.
    if (event.target === overlay) doCancel();
  }

  confirmBtn.addEventListener('click', doConfirm);
  cancelBtn.addEventListener('click', doCancel);
  overlay.addEventListener('keydown', onDialogKeydown);
  overlay.addEventListener('click', onOverlayClick);

  /** Route a command-mode transcript through the backend executor and drive
   *  the UI for the outcome. Exposed for the audio-pipeline wiring that
   *  follows Phase 0. Returns the outcome DTO. */
  async function runTranscript(transcript) {
    const outcome = await invoke('run_command', { transcript });
    const kind = outcome && outcome.kind;
    if (kind === 'pending') {
      openDialog(outcome);
    } else if (kind === 'executed') {
      showToast('Command executed', 'success');
    } else if (kind === 'blocked') {
      showToast('Command blocked by your permission settings', 'error');
    } else if (kind === 'no_action') {
      showToast('No matching command', 'error');
    }
    return outcome;
  }

  window.murmurRunCommand = runTranscript;

  // The audio pipeline emits this when a phrase is finalized while command
  // mode is active: route it through the executor and drive the confirm UI.
  const unlistenTranscript = listen('command-transcript', (event) => {
    const text = event.payload && event.payload.text;
    if (text) {
      runTranscript(text).catch((err) => showToast(`Command error: ${err}`, 'error'));
    }
  });

  // Clean up event listeners if the window is torn down.
  window.addEventListener('beforeunload', () => {
    unlistenMode.then((off) => off()).catch(() => {});
    unlistenTranscript.then((off) => off()).catch(() => {});
    confirmBtn.removeEventListener('click', doConfirm);
    cancelBtn.removeEventListener('click', doCancel);
    overlay.removeEventListener('keydown', onDialogKeydown);
    overlay.removeEventListener('click', onOverlayClick);
  });
})();
