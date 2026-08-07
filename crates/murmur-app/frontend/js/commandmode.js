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
  const chooseOverlay = document.getElementById('command-choose');
  const chooseDialog = chooseOverlay ? chooseOverlay.querySelector('[role="dialog"]') : null;
  const chooseList = document.getElementById('command-choose-list');
  const chooseCancelBtn = document.getElementById('command-choose-cancel');
  if (!badge || !overlay || !dialog || !toolEl || !argsEl || !confirmBtn || !cancelBtn) return;
  if (!chooseOverlay || !chooseDialog || !chooseList || !chooseCancelBtn) return;

  // Where focus returns when the dialog closes.
  let lastFocused = null;
  // Guards double-activation while a confirm/cancel invoke is in flight.
  let busy = false;
  // Nonce of the action the dialog currently shows; confirm/cancel echo it so
  // the backend refuses clicks that race a superseding utterance.
  let shownNonce = null;

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
    shownNonce = pending.nonce;
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
    shownNonce = null;
    if (lastFocused && typeof lastFocused.focus === 'function') lastFocused.focus();
    lastFocused = null;
  }

  async function doCancel() {
    if (busy || shownNonce === null) return;
    busy = true;
    try {
      await invoke('cancel_pending', { nonce: shownNonce });
    } catch (err) {
      console.error('Failed to cancel pending action:', err);
    } finally {
      busy = false;
      closeDialog();
    }
  }

  async function doConfirm() {
    if (busy || shownNonce === null) return;
    busy = true;
    try {
      await invoke('confirm_pending', { nonce: shownNonce });
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

  // --- Spoken path picker ---------------------------------------------
  // Same gate discipline as the confirm dialog: a nonce binds the click to
  // the exact candidate set on screen, and the backend refuses a click that
  // races a superseding utterance.

  let chooseLastFocused = null;
  let chooseBusy = false;
  let chooseNonce = null;

  function chooseOptions() {
    return Array.from(chooseDialog.querySelectorAll('button:not([disabled])'));
  }

  function openChooser(outcome) {
    const candidates = Array.isArray(outcome.candidates) ? outcome.candidates : [];
    if (!candidates.length) return;
    chooseNonce = outcome.nonce;
    chooseList.textContent = '';
    candidates.forEach((path, index) => {
      const item = document.createElement('li');
      const button = document.createElement('button');
      button.type = 'button';
      button.className = 'cmdchoose__option';
      button.dataset.index = String(index);
      const key = document.createElement('span');
      key.className = 'cmdchoose__key';
      key.setAttribute('aria-hidden', 'true');
      key.textContent = String(index + 1);
      const label = document.createElement('span');
      // textContent only: candidate paths are ASR-adjacent data and must
      // never be interpreted as markup.
      label.textContent = path;
      button.append(key, label);
      item.appendChild(button);
      chooseList.appendChild(item);
    });
    chooseLastFocused = document.activeElement;
    chooseOverlay.hidden = false;
    const first = chooseList.querySelector('button');
    if (first) first.focus();
  }

  function closeChooser() {
    chooseOverlay.hidden = true;
    chooseNonce = null;
    chooseList.textContent = '';
    if (chooseLastFocused && typeof chooseLastFocused.focus === 'function') {
      chooseLastFocused.focus();
    }
    chooseLastFocused = null;
  }

  async function cancelChoice() {
    if (chooseBusy || chooseNonce === null) return;
    chooseBusy = true;
    try {
      await invoke('cancel_choice', { nonce: chooseNonce });
    } catch (err) {
      console.error('Failed to cancel path suggestions:', err);
    } finally {
      chooseBusy = false;
      closeChooser();
    }
  }

  async function pickCandidate(index) {
    if (chooseBusy || chooseNonce === null) return;
    chooseBusy = true;
    try {
      const delivery = await invoke('choose_candidate', { nonce: chooseNonce, index });
      // The backend diverts to the clipboard when it cannot hand focus back
      // to the window the user was dictating into.
      showToast(delivery === 'copied' ? 'Path copied to clipboard' : 'Path inserted', 'success');
    } catch (err) {
      showToast(`Could not insert path: ${err}`, 'error');
    } finally {
      chooseBusy = false;
      closeChooser();
    }
  }

  function onChooseClick(event) {
    if (event.target === chooseOverlay) {
      cancelChoice();
      return;
    }
    const option = event.target.closest('.cmdchoose__option');
    if (option) pickCandidate(Number(option.dataset.index));
  }

  // Digits pick directly, arrows walk the list, Enter activates the focused
  // option, Escape dismisses; Tab is trapped inside the dialog.
  function onChooseKeydown(event) {
    if (event.key === 'Escape') {
      event.preventDefault();
      cancelChoice();
      return;
    }
    const options = Array.from(chooseList.querySelectorAll('button'));
    if (event.key >= '1' && event.key <= '5') {
      const index = Number(event.key) - 1;
      if (index < options.length) {
        event.preventDefault();
        pickCandidate(index);
      }
      return;
    }
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      if (!options.length) return;
      event.preventDefault();
      const current = options.indexOf(document.activeElement);
      const step = event.key === 'ArrowDown' ? 1 : -1;
      const next = (current + step + options.length) % options.length;
      options[current === -1 ? 0 : next].focus();
      return;
    }
    if (event.key !== 'Tab') return;
    const items = chooseOptions();
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

  chooseCancelBtn.addEventListener('click', cancelChoice);
  chooseOverlay.addEventListener('click', onChooseClick);
  chooseOverlay.addEventListener('keydown', onChooseKeydown);

  /** Route a command-mode transcript through the backend executor and drive
   *  the UI for the outcome. Exposed for the audio-pipeline wiring that
   *  follows Phase 0. Returns the outcome DTO. */
  async function runTranscript(transcript) {
    const outcome = await invoke('run_command', { transcript });
    const kind = outcome && outcome.kind;
    // The backend cleared both gates for this utterance, so whichever dialog
    // is still on screen is dead; take it down rather than leave a click that
    // can only fail.
    if (chooseNonce !== null && kind !== 'choose') closeChooser();
    if (shownNonce !== null && kind !== 'pending') closeDialog();
    if (kind === 'choose') {
      openChooser(outcome);
    } else if (kind === 'pending') {
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
    chooseCancelBtn.removeEventListener('click', cancelChoice);
    chooseOverlay.removeEventListener('click', onChooseClick);
    chooseOverlay.removeEventListener('keydown', onChooseKeydown);
  });
})();
