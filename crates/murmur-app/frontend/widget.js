const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { getCurrentWindow } = window.__TAURI__.window;

const widget = document.getElementById('widget');
const micBtn = document.getElementById('mic-btn');
const stateLabel = document.getElementById('state-label');
const bars = document.querySelectorAll('.bar');

// ─── State Management ────────────────────────────────────────────────────────

function applyWidgetState(name, label) {
  widget.className = `pill pill--${name}`;
  stateLabel.textContent = label;
  
  if (name !== 'recording') {
    bars.forEach(bar => {
      bar.style.height = '4px';
    });
  }
}

// ─── Audio Level ─────────────────────────────────────────────────────────────

listen('audio-level', (event) => {
  const level = event.payload;
  if (typeof level !== 'number' || !widget.classList.contains('pill--recording')) return;

  // Level is a float from the audio input. Usually quite small, so multiply to boost.
  const normalized = Math.min(1, level * 5);
  
  bars.forEach((bar, index) => {
    // Make center bars react more than outer bars
    const factor = 1 - Math.abs((index - 2) / 2); // Pattern: 0, 0.5, 1, 0.5, 0
    // Base height 4px, max additional height 12px
    const height = 4 + (normalized * 12 * factor) + (Math.random() * 2 * factor);
    bar.style.height = `${Math.min(16, Math.max(4, height))}px`;
  });
});

// ─── Dragging ───────────────────────────────────────────────────────────────

widget.addEventListener('mousedown', async (e) => {
  // Prevent drag if clicking the mic button
  if (e.button !== 0 || e.target.closest('.pill__icon-btn')) return;
  await getCurrentWindow().startDragging();
});

// ─── Mic Button Click ────────────────────────────────────────────────────────

micBtn.addEventListener('click', async () => {
  try {
    await invoke('toggle_recording');
  } catch (err) {
    console.error('toggle_recording failed:', err);
    applyWidgetState('error', 'Error');
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
  applyWidgetState('error', 'Error');
  setTimeout(() => applyWidgetState('idle', 'Idle'), 2000);
});

listen('streaming-phrase', () => {
  applyWidgetState('recording', 'Listening');
});

listen('streaming-done', () => {
  applyWidgetState('idle', 'Idle');
});
