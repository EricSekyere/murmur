const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { getCurrentWindow } = window.__TAURI__.window;

const widget   = document.getElementById('widget');
const micBtn   = document.getElementById('mic-btn');
const label    = document.getElementById('state-label');
const canvas   = document.getElementById('waveform');
const ctx      = canvas.getContext('2d');

// ─── Waveform State ──────────────────────────────────────────────────────────
let currentRms  = 0;
let targetRms   = 0;
let wavePhase   = 0;
let animHandle  = null;
let currentState = 'idle';

// ─── State ───────────────────────────────────────────────────────────────────

function applyState(name, text) {
  if (name === currentState && name !== 'error') return;
  currentState = name;
  widget.className = `pill pill--${name}`;
  label.textContent = text;

  if (name === 'recording') {
    startWaveform();
  } else {
    stopWaveform();
  }
}

// ─── Waveform ────────────────────────────────────────────────────────────────

function sizeCanvas() {
  const dpr = window.devicePixelRatio || 1;
  const r = canvas.getBoundingClientRect();
  canvas.width  = r.width * dpr;
  canvas.height = r.height * dpr;
  ctx.scale(dpr, dpr);
}

function drawWave() {
  animHandle = requestAnimationFrame(drawWave);

  currentRms += (targetRms - currentRms) * 0.2;
  wavePhase  += 0.055;

  const w   = canvas.getBoundingClientRect().width;
  const h   = canvas.getBoundingClientRect().height;
  const mid = h / 2;
  const amp = Math.min(1, currentRms * 5);

  ctx.clearRect(0, 0, w, h);

  // Three layered sine waves
  const layers = [
    { freq: 0.09, shift: 0,   alpha: 0.85, peak: 12 },
    { freq: 0.13, shift: 1.4, alpha: 0.45, peak: 9  },
    { freq: 0.06, shift: 3.0, alpha: 0.22, peak: 7  },
  ];

  for (const l of layers) {
    ctx.beginPath();
    for (let x = 0; x <= w; x++) {
      const env = Math.sin((x / w) * Math.PI);  // fade edges
      const y = mid + Math.sin(x * l.freq + wavePhase + l.shift) * l.peak * amp * env;
      x === 0 ? ctx.moveTo(x, y) : ctx.lineTo(x, y);
    }
    ctx.strokeStyle = `rgba(239, 68, 68, ${l.alpha})`;
    ctx.lineWidth = 1.8;
    ctx.stroke();
  }
}

function startWaveform() {
  if (animHandle) return;
  canvas.style.display = 'block';
  sizeCanvas();
  currentRms = 0;
  targetRms  = 0;
  wavePhase  = 0;
  animHandle = requestAnimationFrame(drawWave);
}

function stopWaveform() {
  if (animHandle) {
    cancelAnimationFrame(animHandle);
    animHandle = null;
  }
  canvas.style.display = 'none';
  currentRms = 0;
  targetRms  = 0;
}

// ─── Audio Level ─────────────────────────────────────────────────────────────

listen('audio-level', (event) => {
  if (typeof event.payload === 'number' && currentState === 'recording') {
    targetRms = event.payload;
  }
});

// ─── Mic Button ──────────────────────────────────────────────────────────────

micBtn.addEventListener('click', async () => {
  try {
    await invoke('toggle_recording');
  } catch (err) {
    console.error('toggle_recording failed:', err);
    applyState('error', 'error');
    setTimeout(() => applyState('idle', 'murmur'), 2000);
  }
});

// ─── Backend Events ──────────────────────────────────────────────────────────

listen('recording-state', (event) => {
  const { recording, processing } = event.payload;
  if (recording && processing) {
    // Brief processing flash mid-session — keep showing recording
    applyState('recording', '');
  } else if (recording) {
    applyState('recording', '');
  } else if (processing) {
    applyState('processing', 'thinking');
  } else {
    applyState('idle', 'murmur');
  }
});

listen('hotkey-transcribed', () => {
  applyState('idle', 'murmur');
});

listen('hotkey-error', (event) => {
  const msg = event.payload?.error;
  // Don't flash error for "no speech" — just go idle quietly
  if (msg && msg.includes('No speech')) {
    applyState('idle', 'murmur');
    return;
  }
  applyState('error', 'error');
  setTimeout(() => applyState('idle', 'murmur'), 2000);
});

listen('streaming-phrase', () => {
  // Stay in recording if we're still going
  if (currentState !== 'recording') {
    applyState('recording', '');
  }
});

listen('streaming-done', () => {
  applyState('idle', 'murmur');
});

listen('transcription-error', (event) => {
  const msg = event.payload?.error || 'transcription error';
  console.error('Transcription error:', msg);
  applyState('error', 'error');
  setTimeout(() => {
    if (currentState === 'error') applyState('idle', 'murmur');
  }, 2000);
});
