// Shared mutable state and core UI state machine.

let uiState = 'idle';
let modelReady = false;
let modelName = 'small.en';
let recordingStartTime = null;
let durationTimerHandle = null;
let lastTranscription = '';
let history = [];                  // max 10, newest first
let transcriptionHandled = false;  // guard: prevent double-display from invoke + event
let currentSession = null;
let sessionPhrases = [];           // delivered segments this session; '\n' marks line breaks
let interimText = '';              // live partial for the phrase currently being spoken

let vizActive = false;
let animationFrameHandle = null;
let currentRms = 0;
let targetRms = 0;
let lastTranscriptionErrorAt = 0;
let lastTranscriptionError = '';

const diagnostics = {
  liveRms: 0,
  peakRms: 0,
  accepted: 0,
  rejected: 0,
  reasons: {
    too_short: 0,
    too_quiet: 0,
    hallucination: 0,
    engine: 0,
    no_signal: 0,
    other: 0,
  },
};

const STATE_CONFIG = {
  idle: {
    badgeClass: 'badge--idle',
    badgeText:  'Idle',
    micClass:   'mic-btn--idle',
    wrapperClass: '',
    ariaLabel:  'Start recording',
    ariaPressed: 'false',
    disabled: false,
  },
  recording: {
    badgeClass: 'badge--recording',
    badgeText:  'Recording',
    micClass:   'mic-btn--recording',
    wrapperClass: 'mic-wrapper--recording',
    ariaLabel:  'Stop recording',
    ariaPressed: 'true',
    disabled: false,
  },
  processing: {
    badgeClass: 'badge--processing',
    badgeText:  'Processing',
    micClass:   'mic-btn--processing',
    wrapperClass: 'mic-wrapper--processing',
    ariaLabel:  'Processing…',
    ariaPressed: 'false',
    disabled: true,
  },
  done: {
    badgeClass: 'badge--done',
    badgeText:  'Done',
    micClass:   'mic-btn--done',
    wrapperClass: '',
    ariaLabel:  'Start recording',
    ariaPressed: 'false',
    disabled: false,
  },
  error: {
    badgeClass: 'badge--error',
    badgeText:  'Error',
    micClass:   'mic-btn--error',
    wrapperClass: '',
    ariaLabel:  'Start recording',
    ariaPressed: 'false',
    disabled: false,
  },
};

function applyState(newState) {
  uiState = newState;
  const cfg = STATE_CONFIG[newState];
  if (!cfg) return;

  statusBadge.className = `badge ${cfg.badgeClass}`;
  statusBadge.textContent = cfg.badgeText;

  micBtn.className = `mic-btn ${cfg.micClass}`;
  micBtn.setAttribute('aria-label', cfg.ariaLabel);
  micBtn.setAttribute('aria-pressed', cfg.ariaPressed);
  micBtn.disabled = cfg.disabled || !modelReady;

  micWrapper.className = `mic-wrapper${cfg.wrapperClass ? ' ' + cfg.wrapperClass : ''}`;

  durationDisplay.hidden = newState !== 'recording';
  wordCount.hidden        = newState !== 'done';
  procTime.hidden         = newState !== 'done';
}

function updateModelBanner(status) {
  modelReady = !!status.model_ready;
  modelName  = status.model || 'small.en';

  modelBanner.hidden = modelReady;
  if (!modelReady) {
    modelBannerText.textContent = `Downloading ${modelName}...`;
  }
  modelInfo.textContent = `Model: ${modelName}`;
  micBtn.disabled = !modelReady || uiState === 'processing';
}

function showError(msg) {
  errorMessage.textContent = msg;
  errorBanner.hidden = false;
  applyState('error');
}

function clearError() {
  errorBanner.hidden = true;
  applyState('idle');
}

dismissError.addEventListener('click', clearError);

function startDurationTimer() {
  recordingStartTime = Date.now();
  durationDisplay.hidden = false;
  durationDisplay.textContent = '0:00';
  durationTimerHandle = setInterval(() => {
    const elapsed = Math.floor((Date.now() - recordingStartTime) / 1000);
    const m = Math.floor(elapsed / 60);
    const s = elapsed % 60;
    durationDisplay.textContent = `${m}:${s.toString().padStart(2, '0')}`;
  }, 1000);
}

function stopDurationTimer() {
  if (durationTimerHandle !== null) {
    clearInterval(durationTimerHandle);
    durationTimerHandle = null;
  }
  durationDisplay.hidden = true;
  recordingStartTime = null;
}

function showToast(message, type = 'success', durationMs = 3000) {
  const toast = document.createElement('div');
  toast.className = `toast toast--${type}`;
  toast.textContent = message;
  toastContainer.appendChild(toast);

  setTimeout(() => {
    toast.classList.add('toast--dismissing');
    toast.addEventListener('animationend', () => toast.remove());
  }, durationMs - 250);
}

function copyToClipboard(text, btn) {
  if (!navigator.clipboard) return;
  const original = btn.textContent;
  navigator.clipboard.writeText(text).then(() => {
    btn.textContent = '✓';
    setTimeout(() => { btn.textContent = original; }, 1200);
  }).catch(err => {
    console.warn('Clipboard write failed:', err);
  });
}
