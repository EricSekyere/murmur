const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { getCurrentWindow } = window.__TAURI__.window;
// LogicalSize lives in the `dpi` module and is re-exported by `window`;
// resolve from whichever the runtime exposes.
const LogicalSize =
  (window.__TAURI__.dpi && window.__TAURI__.dpi.LogicalSize) ||
  window.__TAURI__.window.LogicalSize;

const widget = document.getElementById('widget');
const micBtn = document.getElementById('mic-btn');
const label  = document.getElementById('state-label');
const canvas = document.getElementById('waveform');
const ctx    = canvas.getContext('2d');
const caption     = document.getElementById('caption');
const captionText = document.getElementById('caption-text');

// ─── Live caption: grow the window to show interim text, shrink when done ────

const appWindow = getCurrentWindow();
// Must match tauri.conf.json's widget window size.
const COMPACT_SIZE = { w: 176, h: 50 };
const EXPANDED_SIZE = { w: 320, h: 128 };
let captionExpanded = false;

async function resizeWidget(size) {
  try {
    await appWindow.setSize(new LogicalSize(size.w, size.h));
  } catch (err) {
    console.warn('Widget resize failed:', err);
  }
}

function showCaption(text) {
  captionText.textContent = text;
  if (!captionExpanded) {
    captionExpanded = true;
    caption.hidden = false;
    resizeWidget(EXPANDED_SIZE);
  }
}

function hideCaption() {
  if (!captionExpanded) return;
  captionExpanded = false;
  caption.hidden = true;
  captionText.textContent = '';
  resizeWidget(COMPACT_SIZE);
}

// ─── State ───────────────────────────────────────────────────────────────────

const BAR_COUNT = 22;
const PUSH_INTERVAL_MS = 70; // ~14 new bars per second scroll speed

let currentState = 'idle';
// Monotonic token: every state change bumps it, so delayed resets
// (error flashes) can detect they're stale and skip stomping a newer state.
let stateToken = 0;

let levels = new Array(BAR_COUNT).fill(0);
let targetRms = 0;
let smoothRms = 0;
let lastPush = 0;
let animHandle = null;

let timerHandle = null;
let recordStart = 0;
let heardSpeech = false;

function setLabel(text) {
  label.textContent = text;
}

function applyState(name, text) {
  stateToken += 1;
  if (name !== currentState) {
    // Swap only the state class so transient classes (e.g. the locate flash)
    // survive a state change mid-animation.
    widget.classList.remove(`pill--${currentState}`);
    widget.classList.add(`pill--${name}`);
    currentState = name;
  }
  if (text !== undefined) setLabel(text);

  micBtn.setAttribute(
    'aria-label',
    name === 'recording' ? 'Stop recording' : 'Start recording'
  );

  if (name === 'recording') {
    startRecordingUi();
  } else {
    stopRecordingUi();
    // The caption is only meaningful while actively dictating.
    hideCaption();
  }
}

/** Show a transient state, then fall back to idle unless something newer happened. */
function flashState(name, text, ms) {
  applyState(name, text);
  const token = stateToken;
  setTimeout(() => {
    if (stateToken === token) applyState('idle', 'murmur');
  }, ms);
}

// ─── Recording UI: timer + waveform ──────────────────────────────────────────

function startRecordingUi() {
  if (!animHandle) {
    sizeCanvas();
    levels.fill(0);
    smoothRms = 0;
    targetRms = 0;
    lastPush = 0;
    animHandle = requestAnimationFrame(drawWave);
  }
  if (!timerHandle) {
    recordStart = performance.now();
    heardSpeech = false;
    setLabel('listening');
    timerHandle = setInterval(updateTimer, 250);
  }
}

function stopRecordingUi() {
  if (animHandle) {
    cancelAnimationFrame(animHandle);
    animHandle = null;
  }
  if (timerHandle) {
    clearInterval(timerHandle);
    timerHandle = null;
  }
  targetRms = 0;
  smoothRms = 0;
}

function updateTimer() {
  // Keep showing "listening" until the backend confirms it hears speech.
  if (!heardSpeech) return;
  const total = Math.floor((performance.now() - recordStart) / 1000);
  const mm = Math.floor(total / 60);
  const ss = String(total % 60).padStart(2, '0');
  setLabel(`${mm}:${ss}`);
}

// ─── Waveform: scrolling level bars ──────────────────────────────────────────

function sizeCanvas() {
  const dpr = window.devicePixelRatio || 1;
  const r = canvas.getBoundingClientRect();
  canvas.width = Math.max(1, Math.round(r.width * dpr));
  canvas.height = Math.max(1, Math.round(r.height * dpr));
  // Setting width/height resets the transform; set it explicitly so
  // repeated calls never accumulate scale.
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
}

function drawWave(ts) {
  animHandle = requestAnimationFrame(drawWave);

  smoothRms += (targetRms - smoothRms) * 0.25;
  if (!lastPush || ts - lastPush >= PUSH_INTERVAL_MS) {
    levels.push(smoothRms);
    levels.shift();
    lastPush = ts;
  }

  const r = canvas.getBoundingClientRect();
  const w = r.width;
  const h = r.height;
  ctx.clearRect(0, 0, w, h);

  const gap = 2.5;
  const bw = (w - gap * (BAR_COUNT - 1)) / BAR_COUNT;

  for (let i = 0; i < BAR_COUNT; i++) {
    // Perceptual curve: RMS speech levels are small; lift them so a
    // normal voice fills most of the bar height.
    const v = Math.min(1, Math.pow(levels[i] * 6, 0.75));
    const bh = Math.max(2, v * (h - 4));
    const x = i * (bw + gap);
    const y = (h - bh) / 2;

    // Newest bars (right) are brightest; history fades out to the left.
    const recency = i / (BAR_COUNT - 1);
    ctx.globalAlpha = 0.22 + recency * 0.78;
    ctx.fillStyle = '#f87171';
    if (typeof ctx.roundRect === 'function') {
      ctx.beginPath();
      ctx.roundRect(x, y, bw, bh, bw / 2);
      ctx.fill();
    } else {
      ctx.fillRect(x, y, bw, bh);
    }
  }
  ctx.globalAlpha = 1;
}

// ─── Mic Button ──────────────────────────────────────────────────────────────

micBtn.addEventListener('click', async () => {
  try {
    await invoke('toggle_recording');
  } catch (err) {
    console.error('toggle_recording failed:', err);
    flashState('error', 'error', 2200);
  }
});

// ─── Backend Events ──────────────────────────────────────────────────────────

listen('audio-level', (event) => {
  if (typeof event.payload === 'number' && currentState === 'recording') {
    targetRms = event.payload;
  }
});

listen('audio-signal-detected', () => {
  heardSpeech = true;
});

listen('streaming-partial', (event) => {
  const text = event.payload?.text;
  if (!text || currentState !== 'recording') return;
  showCaption(text);
});

listen('streaming-phrase', (event) => {
  const text = event.payload?.text;
  if (!text || currentState !== 'recording') return;
  // Keep the confirmed phrase on screen until the next interim replaces it.
  showCaption(text);
});

listen('recording-state', (event) => {
  const { recording, processing } = event.payload;
  if (recording) {
    // Recording (possibly with a mid-session transcription in flight) —
    // keep the live waveform/timer rather than flashing a spinner.
    applyState('recording');
  } else if (processing) {
    applyState('processing', 'thinking');
  } else {
    applyState('idle', 'murmur');
  }
});


listen('hotkey-error', (event) => {
  const msg = event.payload?.error || '';
  // "No speech" is an expected outcome, not an error — go idle quietly.
  if (msg.includes('No speech')) {
    applyState('idle', 'murmur');
    return;
  }
  if (msg.includes('still loading')) {
    flashState('processing', 'loading model', 2200);
    return;
  }
  flashState('error', 'error', 2200);
});

listen('streaming-done', () => {
  applyState('idle', 'murmur');
});

// "Find pill" from the dashboard — flash so the user can spot the widget.
listen('locate-pill', () => {
  widget.classList.remove('locating');
  void widget.offsetWidth; // reflow so re-adding restarts the animation
  widget.classList.add('locating');
  setTimeout(() => widget.classList.remove('locating'), 2600);
});

listen('transcription-error', (event) => {
  const msg = event.payload?.error || 'transcription error';
  console.error('Transcription error:', msg);
  // Ignore chunk-level warnings while actively recording.
  if (currentState === 'recording') {
    return;
  }
  flashState('error', 'error', 2200);
});

// ─── Initial sync ────────────────────────────────────────────────────────────
// If the widget (re)loads mid-session, reflect the real backend state
// instead of assuming idle.

(async () => {
  try {
    const status = await invoke('get_status');
    if (status && status.recording) {
      applyState('recording');
    }
  } catch (_) {
    // Backend not ready yet — stay idle; events will correct us.
  }
})();
