const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { getCurrentWindow } = window.__TAURI__.window;

const widget = document.getElementById('widget');
const micBtn = document.getElementById('mic-btn');
const stateLabel = document.getElementById('state-label');

// ─── State Management ────────────────────────────────────────────────────────

function applyWidgetState(name, label) {
  widget.className = `widget widget--${name}`;
  stateLabel.textContent = label;
}

// ─── Dragging ───────────────────────────────────────────────────────────────

widget.addEventListener('mousedown', async (e) => {
  if (e.button !== 0 || e.target.closest('.widget__btn')) return;
  await getCurrentWindow().startDragging();
});

// ─── Mic Button Click ────────────────────────────────────────────────────────

micBtn.addEventListener('click', async () => {
  try {
    await invoke('toggle_recording');
  } catch (err) {
    console.error('toggle_recording failed:', err);
    applyWidgetState('idle', 'Error');
    setTimeout(() => applyWidgetState('idle', 'Idle'), 2000);
  }
});

// ─── Event Listeners ─────────────────────────────────────────────────────────

listen('recording-state', (event) => {
  const { recording, processing } = event.payload;
  if (recording) {
    applyWidgetState('recording', 'Listening');
  } else if (processing) {
    applyWidgetState('processing', 'Thinking');
  } else {
    applyWidgetState('idle', 'Idle');
  }
});

listen('hotkey-transcribed', () => {
  applyWidgetState('idle', 'Idle');
});

listen('hotkey-error', () => {
  applyWidgetState('idle', 'Error');
  setTimeout(() => applyWidgetState('idle', 'Idle'), 2000);
});

// ─── Streaming Events ─────────────────────────────────────────────────────

listen('streaming-phrase', () => {
  // Flash back to recording/listening after a brief processing state
  applyWidgetState('recording', 'Listening');
});

listen('streaming-done', () => {
  applyWidgetState('idle', 'Idle');
});
