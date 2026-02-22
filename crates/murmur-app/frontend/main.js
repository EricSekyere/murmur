const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

// ─── State ──────────────────────────────────────────────────────────────────
let uiState = 'idle';
let modelReady = false;
let modelName = 'small.en';
let recordingStartTime = null;
let durationTimerHandle = null;
let lastTranscription = '';
let history = [];           // max 10, newest first
let transcriptionHandled = false;  // guard: prevent double-display from invoke + event

// Analytics - current session
let currentSession = null;

// Web Audio
let audioContext = null;
let analyserNode = null;
let micStream = null;
let animationFrameHandle = null;
let vizActive = false;

// ─── DOM Refs ────────────────────────────────────────────────────────────────
const modelBanner       = document.getElementById('model-banner');
const modelBannerText   = document.getElementById('model-banner-text');
const modelProgressWrap = document.getElementById('model-progress-wrap');
const modelProgressFill = document.getElementById('model-progress-fill');
const modelProgressPct  = document.getElementById('model-progress-pct');
const errorBanner       = document.getElementById('error-banner');
const errorMessage      = document.getElementById('error-message');
const dismissError      = document.getElementById('dismiss-error');
const statusBadge       = document.getElementById('status-badge');
const micWrapper        = document.getElementById('mic-wrapper');
const micBtn            = document.getElementById('mic-btn');
const visualization     = document.getElementById('visualization');
const voiceBarsContainer = document.getElementById('voice-bars');
const levelFill         = document.getElementById('level-fill');
const durationDisplay   = document.getElementById('duration-display');
const wordCount         = document.getElementById('word-count');
const procTime          = document.getElementById('proc-time');
const transcriptionOutput = document.getElementById('transcription-output');
const copyTranscription = document.getElementById('copy-transcription');
const historyToggle     = document.getElementById('history-toggle');
const historyList       = document.getElementById('history-list');
const historyCount      = document.getElementById('history-count');
const modelInfo         = document.getElementById('model-info');
const hotkeyDisplay     = document.getElementById('hotkey-display');
const outputModeDisplay = document.getElementById('output-mode-display');
const toastContainer    = document.getElementById('toast-container');
const settingsToggle    = document.getElementById('settings-toggle');
const settingsPanel     = document.getElementById('settings-panel');
const modelList         = document.getElementById('model-list');
const analyticsToggle   = document.getElementById('analytics-toggle');
const analyticsPanel    = document.getElementById('analytics-panel');
const devModeBadge      = document.getElementById('dev-mode-badge');
const developerModeToggle = document.getElementById('developer-mode-toggle');

// Settings controls
const hotkeyInput       = document.getElementById('hotkey-input');
const hotkeySave        = document.getElementById('hotkey-save');
const outputModeSelect  = document.getElementById('output-mode-select');
const audioDeviceSelect = document.getElementById('audio-device-select');
const phrasePauseRange  = document.getElementById('phrase-pause-range');
const phrasePauseValue  = document.getElementById('phrase-pause-value');
const sessionTimeoutRange = document.getElementById('session-timeout-range');
const sessionTimeoutValue = document.getElementById('session-timeout-value');
const clickToStopToggle = document.getElementById('click-to-stop-toggle');
const showWidgetToggle  = document.getElementById('show-widget-toggle');
const micQuality        = document.getElementById('mic-quality');
const micQualityText    = document.getElementById('mic-quality-text');

// ─── State Machine ───────────────────────────────────────────────────────────
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
    ariaLabel:  'Processing\u2026',
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

// ─── Model Banner ────────────────────────────────────────────────────────────
function updateModelBanner(status) {
  modelReady = !!status.model_ready;
  modelName  = status.model || 'small.en';

  modelBanner.hidden = modelReady;
  if (!modelReady) {
    modelBannerText.textContent = `Downloading ${modelName}...`;
  }
  modelInfo.textContent = `Model: ${modelName}`;

  // Re-apply disabled state based on modelReady
  micBtn.disabled = !modelReady || uiState === 'processing';
}

// ─── Error Banner ────────────────────────────────────────────────────────────
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

// ─── Duration Timer ──────────────────────────────────────────────────────────
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

// ─── Transcription Display ───────────────────────────────────────────────────
function displayTranscription(text, processingTimeMs) {
  lastTranscription = text || '';

  transcriptionOutput.innerHTML = '';
  if (lastTranscription) {
    transcriptionOutput.textContent = lastTranscription;
    copyTranscription.disabled = false;
  } else {
    const ph = document.createElement('span');
    ph.className = 'placeholder';
    ph.textContent = 'No speech detected.';
    transcriptionOutput.appendChild(ph);
    copyTranscription.disabled = true;
  }

  const words = lastTranscription.trim()
    ? lastTranscription.trim().split(/\s+/).length
    : 0;
  wordCount.textContent = `${words} word${words !== 1 ? 's' : ''}`;

  if (processingTimeMs != null) {
    procTime.textContent = `${(processingTimeMs / 1000).toFixed(1)}s`;
  } else {
    procTime.textContent = '';
  }

  addToHistory(lastTranscription, words);
  applyState('done');

  setTimeout(() => {
    if (uiState === 'done') applyState('idle');
  }, 2000);
}

// ─── History ─────────────────────────────────────────────────────────────────
function addToHistory(text, words) {
  if (!text.trim()) return;
  history.unshift({ text, words, timestamp: Date.now() });
  if (history.length > 10) history.pop();
  renderHistory();
}

function relativeTime(timestamp) {
  const delta = Math.floor((Date.now() - timestamp) / 1000);
  if (delta < 60)   return 'just now';
  if (delta < 3600) return `${Math.floor(delta / 60)}m ago`;
  return `${Math.floor(delta / 3600)}h ago`;
}

function renderHistory() {
  historyList.innerHTML = '';

  if (history.length === 0) {
    historyCount.hidden = true;
    return;
  }

  historyCount.textContent = history.length;
  historyCount.hidden = false;

  for (const entry of history) {
    const li = document.createElement('li');
    li.className = 'history-item';

    const textSpan = document.createElement('span');
    textSpan.className = 'history-item__text';
    textSpan.textContent = entry.text.length > 60
      ? entry.text.slice(0, 60) + '\u2026'
      : entry.text;
    textSpan.title = entry.text;

    const timeSpan = document.createElement('span');
    timeSpan.className = 'history-item__time';
    timeSpan.textContent = relativeTime(entry.timestamp);

    const copyBtn = document.createElement('button');
    copyBtn.className = 'history-item__copy';
    copyBtn.textContent = 'Copy';
    copyBtn.setAttribute('aria-label', 'Copy this entry');
    copyBtn.addEventListener('click', () => copyToClipboard(entry.text, copyBtn));

    li.appendChild(textSpan);
    li.appendChild(timeSpan);
    li.appendChild(copyBtn);
    historyList.appendChild(li);
  }
}

// ─── Copy to Clipboard ───────────────────────────────────────────────────────
function copyToClipboard(text, btn) {
  if (!navigator.clipboard) return;
  const original = btn.textContent;
  navigator.clipboard.writeText(text).then(() => {
    btn.textContent = '\u2713';
    setTimeout(() => { btn.textContent = original; }, 1200);
  }).catch(err => {
    console.warn('Clipboard write failed:', err);
  });
}

// ─── Copy Transcription Button ───────────────────────────────────────────────
copyTranscription.addEventListener('click', () => {
  if (!lastTranscription || !navigator.clipboard) return;
  const svgEl = copyTranscription.querySelector('svg');
  navigator.clipboard.writeText(lastTranscription).then(() => {
    copyTranscription.innerHTML = '\u2713';
    setTimeout(() => {
      copyTranscription.innerHTML = '';
      if (svgEl) copyTranscription.appendChild(svgEl);
    }, 1200);
  }).catch(err => console.warn('Copy transcription failed:', err));
});

// ─── Model Download Progress ─────────────────────────────────────────────────
listen('model-download-progress', (event) => {
  const data = event.payload;

  if (data.error) {
    modelBanner.hidden = false;
    modelBannerText.textContent = data.message || 'Download failed';
    modelProgressWrap.hidden = true;
    modelProgressPct.hidden = true;
    // Reset inline model card progress
    if (changingModelId) {
      const progressEl = modelList.querySelector(`.model-card__progress[data-model-id="${changingModelId}"]`);
      if (progressEl) progressEl.hidden = true;
      changingModelId = null;
      loadModelList();
    }
    return;
  }

  if (data.done) {
    modelReady = true;
    modelBanner.hidden = true;
    micBtn.disabled = uiState === 'processing';
    // Hide inline progress
    if (changingModelId) {
      const progressEl = modelList.querySelector(`.model-card__progress[data-model-id="${changingModelId}"]`);
      if (progressEl) progressEl.hidden = true;
    }
    return;
  }

  // In progress — update top banner
  modelBanner.hidden = false;
  modelBannerText.textContent = data.message || `Downloading...`;
  modelProgressWrap.hidden = false;
  modelProgressPct.hidden = false;
  modelProgressFill.style.width = `${data.percent}%`;
  modelProgressPct.textContent = `${data.percent}%`;

  // Update inline model card progress bar
  if (changingModelId) {
    const progressEl = modelList.querySelector(`.model-card__progress[data-model-id="${changingModelId}"]`);
    if (progressEl) {
      progressEl.hidden = false;
      const fill = progressEl.querySelector('.model-card__progress-fill');
      if (fill) fill.style.width = `${data.percent}%`;
    }
  }
});

// ─── Toast Notifications ─────────────────────────────────────────────────
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

// ─── Recording State Events ──────────────────────────────────────────────
listen('recording-state', (event) => {
  const { recording, processing } = event.payload;
  if (recording) {
    // Only reset UI on a fresh recording start — not on processing updates
    // during streaming (which would clear accumulated transcription text).
    if (uiState !== 'recording' && uiState !== 'processing') {
      transcriptionHandled = false;
      transcriptionOutput.innerHTML = '';
      lastTranscription = '';
      copyTranscription.disabled = true;
      wordCount.hidden = true;
      procTime.hidden = true;
      currentSession = {
        startTime: Date.now(),
        endTime: null,
        phraseCount: 0,
        wordCount: 0,
        phraseTimestamps: [],
        processingTimes: [],
      };
      startDurationTimer();
      startVisualization();
    }
    // Stay in 'recording' even during phrase processing so mic button
    // remains clickable for the user to stop.
    applyState('recording');
  } else if (processing) {
    applyState('processing');
    stopDurationTimer();
    stopVisualization();
  } else {
    // idle — only reset if we're in processing or recording
    if (uiState === 'processing' || uiState === 'recording') {
      stopDurationTimer();
      stopVisualization();
      applyState('idle');
    }
  }
});

listen('hotkey-transcribed', (event) => {
  const data = event.payload;
  stopDurationTimer();
  stopVisualization();

  if (data.text) {
    transcriptionHandled = true;
    displayTranscription(data.text, data.processing_time_ms);
    const preview = data.text.length > 40 ? data.text.slice(0, 40) + '\u2026' : data.text;
    showToast(`Typed: ${preview}`, 'success');
  } else {
    applyState('idle');
  }
});

listen('hotkey-error', (event) => {
  const data = event.payload;
  stopDurationTimer();
  stopVisualization();
  showToast(data.error || 'Error', 'error');
  applyState('error');
});

// ─── Streaming Events ────────────────────────────────────────────────────
listen('streaming-phrase', (event) => {
  const { text, processing_time_ms } = event.payload;
  if (!text) return;

  // Append phrase to the transcription output (streaming accumulation)
  if (transcriptionOutput.querySelector('.placeholder')) {
    transcriptionOutput.innerHTML = '';
  }
  // Append with a space separator
  const existing = transcriptionOutput.textContent;
  transcriptionOutput.textContent = existing ? `${existing} ${text}` : text;
  lastTranscription = transcriptionOutput.textContent;
  copyTranscription.disabled = false;

  // Track analytics
  if (currentSession) {
    currentSession.phraseCount++;
    currentSession.wordCount += text.trim().split(/\s+/).length;
    currentSession.phraseTimestamps.push(Date.now());
    if (processing_time_ms != null) {
      currentSession.processingTimes.push(processing_time_ms);
    }
  }

  // Brief "done" flash then back to recording
  applyState('recording');
});

listen('streaming-done', () => {
  stopDurationTimer();
  stopVisualization();

  const finalText = transcriptionOutput.textContent.trim();
  if (finalText) {
    const words = finalText.split(/\s+/).length;
    wordCount.textContent = `${words} word${words !== 1 ? 's' : ''}`;
    addToHistory(finalText, words);
    applyState('done');
    setTimeout(() => {
      if (uiState === 'done') applyState('idle');
    }, 2000);
  } else {
    applyState('idle');
  }

  // Finalize session analytics
  if (currentSession) {
    currentSession.endTime = Date.now();
    finalizeSessionAnalytics(currentSession);
    currentSession = null;
  }
});

// ─── History Toggle ──────────────────────────────────────────────────────────
historyToggle.addEventListener('click', () => {
  const expanded = historyToggle.getAttribute('aria-expanded') === 'true';
  historyToggle.setAttribute('aria-expanded', String(!expanded));
  historyList.hidden = expanded;
});

// ─── Settings Toggle ─────────────────────────────────────────────────────────
settingsToggle.addEventListener('click', async () => {
  const expanded = settingsToggle.getAttribute('aria-expanded') === 'true';
  settingsToggle.setAttribute('aria-expanded', String(!expanded));
  settingsPanel.hidden = expanded;
  if (!expanded) {
    loadModelList();
    loadAudioDevices();
    try {
      const status = await invoke('get_status');
      if (status.hotkey) hotkeyInput.value = status.hotkey;
      if (status.output_mode) outputModeSelect.value = status.output_mode;
      if (status.phrase_pause_secs != null) {
        phrasePauseRange.value = status.phrase_pause_secs;
        phrasePauseValue.textContent = `${parseFloat(status.phrase_pause_secs).toFixed(1)}s`;
      }
      if (status.session_timeout_secs != null) {
        sessionTimeoutRange.value = status.session_timeout_secs;
        sessionTimeoutValue.textContent = `${status.session_timeout_secs}s`;
      }
      if (status.click_to_stop != null) {
        clickToStopToggle.checked = status.click_to_stop;
      }
      if (status.show_widget != null) {
        showWidgetToggle.checked = status.show_widget;
      }
      if (status.developer_mode) {
        developerModeToggle.checked = true;
        devModeBadge.hidden = false;
      }
    } catch (err) {
      console.error('Failed to get settings:', err);
    }
  }
});

// ─── Developer Mode Toggle ───────────────────────────────────────────────────
developerModeToggle.addEventListener('change', async () => {
  const enabled = developerModeToggle.checked;
  try {
    await invoke('set_developer_mode', { enabled });
    devModeBadge.hidden = !enabled;
    showToast(enabled ? 'Developer mode enabled' : 'Developer mode disabled', 'success');
  } catch (err) {
    developerModeToggle.checked = !enabled;
    showToast(`Failed to set developer mode: ${err}`, 'error');
  }
});

// ─── Hotkey Capture ──────────────────────────────────────────────────────────
let capturedHotkey = '';

hotkeyInput.addEventListener('keydown', (e) => {
  e.preventDefault();
  const parts = [];
  if (e.ctrlKey)  parts.push('ctrl');
  if (e.altKey)   parts.push('alt');
  if (e.shiftKey) parts.push('shift');
  if (e.metaKey)  parts.push('super');

  const key = e.key.toLowerCase();
  // Ignore standalone modifier keys
  if (['control', 'alt', 'shift', 'meta'].includes(key)) return;

  parts.push(key === ' ' ? 'space' : key);
  capturedHotkey = parts.join('+');
  hotkeyInput.value = capturedHotkey;
  hotkeySave.disabled = false;
});

hotkeySave.addEventListener('click', async () => {
  if (!capturedHotkey) return;
  hotkeySave.disabled = true;
  try {
    await invoke('update_settings', { hotkey: capturedHotkey });
    hotkeyDisplay.textContent = capturedHotkey;
    showToast('Hotkey updated', 'success');
  } catch (err) {
    showToast(`Hotkey failed: ${err}`, 'error');
  }
  capturedHotkey = '';
});

// ─── Output Mode ─────────────────────────────────────────────────────────────
outputModeSelect.addEventListener('change', async () => {
  const mode = outputModeSelect.value;
  try {
    await invoke('update_settings', { outputMode: mode });
    outputModeDisplay.textContent = mode;
    showToast(`Output: ${mode}`, 'success');
  } catch (err) {
    showToast(`Failed: ${err}`, 'error');
  }
});

// ─── Audio Device ────────────────────────────────────────────────────────────
async function loadAudioDevices() {
  try {
    const devices = await invoke('list_audio_devices');
    audioDeviceSelect.innerHTML = '';
    for (const d of devices) {
      const opt = document.createElement('option');
      opt.value = d;
      opt.textContent = d;
      audioDeviceSelect.appendChild(opt);
    }
  } catch (err) {
    console.warn('Failed to list audio devices:', err);
  }
}

// ─── Phrase Pause Slider ─────────────────────────────────────────────────────
phrasePauseRange.addEventListener('input', () => {
  phrasePauseValue.textContent = `${parseFloat(phrasePauseRange.value).toFixed(1)}s`;
});

phrasePauseRange.addEventListener('change', async () => {
  const val = parseFloat(phrasePauseRange.value);
  try {
    await invoke('update_settings', { phrasePauseSecs: val });
  } catch (err) {
    showToast(`Failed: ${err}`, 'error');
  }
});

// ─── Session Timeout Slider ──────────────────────────────────────────────────
sessionTimeoutRange.addEventListener('input', () => {
  sessionTimeoutValue.textContent = `${sessionTimeoutRange.value}s`;
});

sessionTimeoutRange.addEventListener('change', async () => {
  const val = parseInt(sessionTimeoutRange.value, 10);
  try {
    await invoke('update_settings', { sessionTimeoutSecs: val });
  } catch (err) {
    showToast(`Failed: ${err}`, 'error');
  }
});

// ─── Click to Stop Toggle ────────────────────────────────────────────────────
clickToStopToggle.addEventListener('change', async () => {
  const enabled = clickToStopToggle.checked;
  try {
    await invoke('update_settings', { clickToStop: enabled });
  } catch (err) {
    clickToStopToggle.checked = !enabled;
    showToast(`Failed: ${err}`, 'error');
  }
});

// ─── Show Widget Toggle ──────────────────────────────────────────────────────
showWidgetToggle.addEventListener('change', async () => {
  const visible = showWidgetToggle.checked;
  try {
    await invoke('set_widget_visible', { visible });
  } catch (err) {
    showWidgetToggle.checked = !visible;
    showToast(`Failed: ${err}`, 'error');
  }
});

// ─── Audio Level Events ──────────────────────────────────────────────────────
listen('audio-level', (event) => {
  const level = event.payload;
  if (typeof level !== 'number') return;

  micQuality.hidden = false;
  if (level > 0.15) {
    micQualityText.textContent = 'Good signal';
    micQuality.className = 'mic-quality mic-quality--good';
  } else if (level > 0.05) {
    micQualityText.textContent = 'Fair signal';
    micQuality.className = 'mic-quality mic-quality--fair';
  } else {
    micQualityText.textContent = 'Low signal';
    micQuality.className = 'mic-quality mic-quality--low';
  }
});

// ─── Analytics Toggle ────────────────────────────────────────────────────────
analyticsToggle.addEventListener('click', () => {
  const expanded = analyticsToggle.getAttribute('aria-expanded') === 'true';
  analyticsToggle.setAttribute('aria-expanded', String(!expanded));
  analyticsPanel.hidden = expanded;
  if (!expanded) {
    renderAnalytics();
  }
});

// ─── Model Selector ─────────────────────────────────────────────────────────
let changingModelId = null; // track which model is currently downloading/switching

async function loadModelList() {
  try {
    const models = await invoke('list_models');
    renderModelList(models);
  } catch (err) {
    console.error('Failed to list models:', err);
  }
}

function renderModelList(models) {
  modelList.innerHTML = '';
  for (const m of models) {
    const card = document.createElement('div');
    card.className = `model-card${m.active ? ' model-card--active' : ''}`;
    card.dataset.modelId = m.id;

    const info = document.createElement('div');
    info.className = 'model-card__info';

    const nameRow = document.createElement('div');
    nameRow.className = 'model-card__name';
    nameRow.textContent = m.name;
    if (m.active) {
      const dot = document.createElement('span');
      dot.className = 'model-card__active-dot';
      dot.setAttribute('aria-label', 'Active');
      nameRow.appendChild(dot);
    }

    const desc = document.createElement('div');
    desc.className = 'model-card__desc';
    desc.textContent = m.description;

    const meta = document.createElement('div');
    meta.className = 'model-card__meta';

    const backendBadge = document.createElement('span');
    backendBadge.className = 'model-card__badge';
    backendBadge.textContent = m.backend;

    const sizeBadge = document.createElement('span');
    sizeBadge.className = 'model-card__badge';
    sizeBadge.textContent = `${m.size_mb} MB`;

    meta.appendChild(backendBadge);
    meta.appendChild(sizeBadge);

    if (m.downloaded) {
      const dlBadge = document.createElement('span');
      dlBadge.className = 'model-card__badge';
      dlBadge.textContent = 'downloaded';
      meta.appendChild(dlBadge);
    }

    info.appendChild(nameRow);
    info.appendChild(desc);
    info.appendChild(meta);

    const action = document.createElement('div');
    action.className = 'model-card__action';

    const btn = document.createElement('button');
    btn.className = 'model-card__btn';
    if (m.active) {
      btn.className += ' model-card__btn--active';
      btn.textContent = 'Active';
      btn.disabled = true;
    } else if (m.downloaded) {
      btn.textContent = 'Switch';
      btn.addEventListener('click', () => handleChangeModel(m.id));
    } else {
      btn.textContent = 'Download & Switch';
      btn.addEventListener('click', () => handleChangeModel(m.id));
    }

    const progress = document.createElement('div');
    progress.className = 'model-card__progress';
    progress.hidden = true;
    progress.dataset.modelId = m.id;
    const progressFill = document.createElement('div');
    progressFill.className = 'model-card__progress-fill';
    progress.appendChild(progressFill);

    action.appendChild(btn);
    action.appendChild(progress);

    card.appendChild(info);
    card.appendChild(action);
    modelList.appendChild(card);
  }
}

async function handleChangeModel(modelId) {
  changingModelId = modelId;
  // Disable all model buttons while switching
  for (const btn of modelList.querySelectorAll('.model-card__btn')) {
    btn.disabled = true;
  }
  // Show progress bar for this model
  const progressEl = modelList.querySelector(`.model-card__progress[data-model-id="${modelId}"]`);
  if (progressEl) progressEl.hidden = false;

  try {
    await invoke('change_model', { modelId });
  } catch (err) {
    showToast(`Failed to switch model: ${err}`, 'error');
    changingModelId = null;
    loadModelList();
  }
}

// Listen for model-changed events to refresh the model list
listen('model-changed', (event) => {
  const data = event.payload;
  if (data.ready) {
    changingModelId = null;
    modelName = data.model_name;
    modelReady = true;
    modelBanner.hidden = true;
    modelInfo.textContent = `Model: ${data.model_name}`;
    micBtn.disabled = uiState === 'processing';
    showToast(`Switched to ${data.model_name}`, 'success');
    loadModelList();
  } else {
    modelReady = false;
    modelInfo.textContent = `Loading: ${data.model_name}...`;
    micBtn.disabled = true;
  }
});

// ─── Voice Bars Visualization ────────────────────────────────────────────────
const NUM_BARS = 32;
const BAR_HEIGHT_MAX = 48;
let voiceBars = [];

function createVoiceBars() {
  voiceBarsContainer.innerHTML = '';
  voiceBars = [];
  for (let i = 0; i < NUM_BARS; i++) {
    const bar = document.createElement('div');
    bar.className = 'voice-bar';
    voiceBarsContainer.appendChild(bar);
    voiceBars.push(bar);
  }
}

function resetVoiceBars() {
  for (const bar of voiceBars) {
    bar.style.height = '3px';
  }
  micWrapper.style.removeProperty('--audio-level');
}

async function startVisualization() {
  visualization.hidden = false;
  if (voiceBars.length === 0) createVoiceBars();
  resetVoiceBars();

  try {
    if (!audioContext) {
      audioContext = new AudioContext();
    }
    if (audioContext.state === 'suspended') {
      await audioContext.resume();
    }

    const stream = await navigator.mediaDevices.getUserMedia({ audio: true, video: false });
    micStream = stream;

    const source = audioContext.createMediaStreamSource(stream);
    analyserNode = audioContext.createAnalyser();
    analyserNode.fftSize = 256;
    analyserNode.smoothingTimeConstant = 0.75;
    source.connect(analyserNode);

    vizActive = true;
    drawVisualization();
  } catch (err) {
    console.warn('Web Audio unavailable, visualization disabled:', err);
  }
}

function stopVisualization() {
  vizActive = false;

  if (animationFrameHandle !== null) {
    cancelAnimationFrame(animationFrameHandle);
    animationFrameHandle = null;
  }

  if (micStream) {
    for (const track of micStream.getTracks()) track.stop();
    micStream = null;
  }

  analyserNode = null;
  visualization.hidden = true;
  levelFill.style.width = '0%';
  micQuality.hidden = true;
  resetVoiceBars();
}

function drawVisualization() {
  if (!vizActive || !analyserNode) return;

  animationFrameHandle = requestAnimationFrame(drawVisualization);

  const freqData = new Uint8Array(analyserNode.frequencyBinCount);
  analyserNode.getByteFrequencyData(freqData);

  // Map frequency bins to voice bars
  const binCount = analyserNode.frequencyBinCount;
  const step = Math.floor(binCount / NUM_BARS);
  for (let i = 0; i < NUM_BARS; i++) {
    const val = freqData[i * step] / 255;
    const h = Math.max(3, val * BAR_HEIGHT_MAX);
    voiceBars[i].style.height = `${h}px`;
  }

  // RMS for level bar + mic glow
  const timeData = new Uint8Array(analyserNode.fftSize);
  analyserNode.getByteTimeDomainData(timeData);
  let sumSq = 0;
  for (let i = 0; i < timeData.length; i++) {
    const v = (timeData[i] - 128) / 128;
    sumSq += v * v;
  }
  const rms = Math.sqrt(sumSq / timeData.length);
  const level = Math.min(1, rms * 3);
  levelFill.style.width = `${(level * 100).toFixed(1)}%`;
  micWrapper.style.setProperty('--audio-level', level.toFixed(3));
}

// ─── Mic Button Click ────────────────────────────────────────────────────────
micBtn.addEventListener('click', async () => {
  if (uiState === 'processing') return; // ignore while processing

  clearError();
  try {
    await invoke('toggle_recording');
  } catch (err) {
    showError(String(err));
  }
});

// ─── Backend Transcription Event ─────────────────────────────────────────────
listen('transcription', (event) => {
  if (transcriptionHandled) return;
  transcriptionHandled = true;
  const data = event.payload;
  displayTranscription(data.text, data.processing_time_ms);
});

// ─── Analytics ──────────────────────────────────────────────────────────────

const ANALYTICS_KEY = 'murmur_analytics';

function formatNumber(n) {
  if (n >= 1000) return (n / 1000).toFixed(1) + 'k';
  return String(n);
}

function formatDuration(ms) {
  const totalSecs = Math.floor(ms / 1000);
  const m = Math.floor(totalSecs / 60);
  const s = totalSecs % 60;
  return `${m}:${s.toString().padStart(2, '0')}`;
}

function loadAnalytics() {
  const raw = localStorage.getItem(ANALYTICS_KEY);
  const defaults = {
    totalWords: 0,
    totalSessions: 0,
    totalDurationMs: 0,
    totalWpmSum: 0,
    todayWords: 0,
    todaySessions: 0,
    todayDate: new Date().toDateString(),
    hourlyWords: new Array(24).fill(0),
    lastSession: null,
  };
  if (!raw) return defaults;
  try {
    const data = JSON.parse(raw);
    // Day rollover check
    const today = new Date().toDateString();
    if (data.todayDate !== today) {
      data.todayWords = 0;
      data.todaySessions = 0;
      data.todayDate = today;
    }
    if (!data.hourlyWords) data.hourlyWords = new Array(24).fill(0);
    if (!data.lastSession) data.lastSession = null;
    if (!data.totalWpmSum) data.totalWpmSum = 0;
    return data;
  } catch {
    return defaults;
  }
}

function saveAnalytics(data) {
  localStorage.setItem(ANALYTICS_KEY, JSON.stringify(data));
}

function finalizeSessionAnalytics(session) {
  const durationMs = session.endTime - session.startTime;
  const wpm = durationMs > 10000
    ? Math.round((session.wordCount / durationMs) * 60000)
    : null;
  const avgLatency = session.processingTimes.length > 0
    ? Math.round(session.processingTimes.reduce((a, b) => a + b, 0) / session.processingTimes.length)
    : null;

  const lastSession = {
    wpm,
    phraseCount: session.phraseCount,
    wordCount: session.wordCount,
    durationMs,
    avgLatency,
  };

  const analytics = loadAnalytics();
  analytics.lastSession = lastSession;
  analytics.totalWords += session.wordCount;
  analytics.totalSessions += 1;
  analytics.totalDurationMs += durationMs;
  if (wpm != null) analytics.totalWpmSum += wpm;
  analytics.todayWords += session.wordCount;
  analytics.todaySessions += 1;

  // Track hourly words
  const hour = new Date().getHours();
  analytics.hourlyWords[hour] += session.wordCount;

  saveAnalytics(analytics);
  renderAnalytics();
}

function renderAnalytics() {
  const analytics = loadAnalytics();
  const ls = analytics.lastSession;

  // Last session
  document.getElementById('stat-wpm').textContent = ls && ls.wpm != null ? String(ls.wpm) : '--';
  document.getElementById('stat-phrases').textContent = ls ? String(ls.phraseCount) : '0';
  document.getElementById('stat-duration').textContent = ls ? formatDuration(ls.durationMs) : '0:00';
  document.getElementById('stat-latency').textContent = ls && ls.avgLatency != null ? `${ls.avgLatency}ms` : '--';

  // Today
  document.getElementById('stat-today-words').textContent = formatNumber(analytics.todayWords);
  document.getElementById('stat-today-sessions').textContent = String(analytics.todaySessions);

  // All time
  document.getElementById('stat-total-words').textContent = formatNumber(analytics.totalWords);
  document.getElementById('stat-total-sessions').textContent = String(analytics.totalSessions);

  const avgWpm = analytics.totalSessions > 0
    ? Math.round(analytics.totalWpmSum / analytics.totalSessions)
    : null;
  document.getElementById('stat-avg-wpm').textContent = avgWpm != null ? String(avgWpm) : '--';

  const avgDur = analytics.totalSessions > 0
    ? Math.round(analytics.totalDurationMs / analytics.totalSessions)
    : 0;
  document.getElementById('stat-avg-duration').textContent = formatDuration(avgDur);

  // Peak hour
  const peakEl = document.getElementById('stat-peak-hour');
  const maxWords = Math.max(...analytics.hourlyWords);
  if (maxWords > 0) {
    const peakHour = analytics.hourlyWords.indexOf(maxWords);
    const startH = peakHour % 12 || 12;
    const startP = peakHour < 12 ? 'AM' : 'PM';
    const endHour = (peakHour + 1) % 24;
    const endH = endHour % 12 || 12;
    const endP = endHour < 12 ? 'AM' : 'PM';
    peakEl.textContent = `${startH}${startP} - ${endH}${endP} (${formatNumber(maxWords)} words)`;
  } else {
    peakEl.textContent = 'No data yet';
  }
}

// ─── Initialize ──────────────────────────────────────────────────────────────
async function init() {
  createVoiceBars();
  renderAnalytics();
  try {
    const status = await invoke('get_status');
    updateModelBanner(status);
    if (status.hotkey) {
      hotkeyDisplay.textContent = status.hotkey;
    }
    if (status.output_mode) {
      outputModeDisplay.textContent = status.output_mode;
    }
    if (status.developer_mode) {
      developerModeToggle.checked = true;
      devModeBadge.hidden = false;
    }
  } catch (err) {
    console.error('Failed to get status:', err);
    updateModelBanner({ model_ready: false, model: 'small.en', recording: false, mode: 'idle' });
  }
}

init();
