// Settings panel: hotkey, output mode, devices, sliders, and model picker.

let capturedHotkey = '';
let changingModelId = null; // model currently downloading/switching

settingsToggle.addEventListener('click', async () => {
  const expanded = settingsToggle.getAttribute('aria-expanded') === 'true';
  settingsToggle.setAttribute('aria-expanded', String(!expanded));
  settingsPanel.hidden = expanded;
  if (expanded) return;

  loadModelList();
  try {
    const status = await invoke('get_status');
    await loadAudioDevices(status.audio_device || '');
    if (status.hotkey) hotkeyInput.value = status.hotkey;
    if (status.output_mode) {
      outputModeSelect.value = status.output_mode === 'stdout' ? 'auto' : status.output_mode;
    }
    if (status.transcription_profile) {
      transcriptionProfileSelect.value = status.transcription_profile;
    }
    if (status.language) {
      languageSelect.value = status.language;
    }
    translateToggle.checked = !!status.translate_to_english;
    applyMultilingualState(!!status.model_multilingual);
    if (status.phrase_pause_secs != null) {
      phrasePauseRange.value = status.phrase_pause_secs;
      phrasePauseValue.textContent = `${parseFloat(status.phrase_pause_secs).toFixed(1)}s`;
    }
    if (status.vad_threshold != null) {
      const pct = thresholdToSensitivity(status.vad_threshold);
      micSensitivityRange.value = pct;
      micSensitivityValue.textContent = `${pct}%`;
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
    if (status.activation_mode) {
      activationModeSelect.value = status.activation_mode;
    }
    if (status.double_tap_key) {
      doubleTapKeySelect.value = ['rctrl', 'lctrl', 'ctrl'].includes(status.double_tap_key)
        ? status.double_tap_key
        : 'rctrl';
    }
    if (Array.isArray(status.custom_vocabulary)) {
      vocabularyInput.value = status.custom_vocabulary.join('\n');
      vocabularySave.disabled = true;
    }
    if (Array.isArray(status.snippets)) {
      snippetsInput.value = status.snippets
        .map(s => `${s.trigger} = ${s.expansion}`)
        .join('\n');
      snippetsSave.disabled = true;
    }
    if (Array.isArray(status.app_profiles)) {
      appProfilesInput.value = status.app_profiles
        .map(formatAppProfile)
        .join('\n');
      appProfilesSave.disabled = true;
    }
    if (status.sound_feedback != null) {
      soundFeedbackToggle.checked = status.sound_feedback;
    }
    if (status.live_preview != null) {
      livePreviewToggle.checked = status.live_preview;
    }
    if (status.caption_position) {
      captionPositionSelect.value = status.caption_position;
    }
    developerModeToggle.checked = !!status.developer_mode;
    devModeBadge.hidden = !status.developer_mode;
  } catch (err) {
    console.error('Failed to get settings:', err);
  }
});

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

hotkeyInput.addEventListener('keydown', (e) => {
  e.preventDefault();
  const parts = [];
  if (e.ctrlKey)  parts.push('ctrl');
  if (e.altKey)   parts.push('alt');
  if (e.shiftKey) parts.push('shift');
  if (e.metaKey)  parts.push('super');

  const key = e.key.toLowerCase();
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

outputModeSelect.addEventListener('change', async () => {
  const mode = outputModeSelect.value;
  try {
    await invoke('update_settings', { output_mode: mode });
    outputModeDisplay.textContent = mode;
    showToast(`Output: ${mode}`, 'success');
  } catch (err) {
    showToast(`Failed: ${err}`, 'error');
  }
});

transcriptionProfileSelect.addEventListener('change', async () => {
  const profile = transcriptionProfileSelect.value;
  try {
    await invoke('update_settings', { transcription_profile: profile });
    showToast(`Profile: ${profile}`, 'success');
  } catch (err) {
    showToast(`Failed: ${err}`, 'error');
  }
});

// Language and translation only work with a multilingual model; reflect that
// by dimming the controls and noting it in the hint when an English-only
// model is active.
function applyMultilingualState(multilingual) {
  languageSelect.disabled = !multilingual;
  translateToggle.disabled = !multilingual;
  if (languageHint) {
    languageHint.textContent = multilingual
      ? 'Auto-detect, or pick a language. Powered by the multilingual model.'
      : 'Needs the multilingual model (Large v3 Turbo). The English models only do English.';
  }
}

languageSelect.addEventListener('change', async () => {
  const language = languageSelect.value;
  try {
    await invoke('update_settings', { language });
    showToast(`Language: ${languageSelect.options[languageSelect.selectedIndex].text}`, 'success');
  } catch (err) {
    showToast(`Failed: ${err}`, 'error');
  }
});

translateToggle.addEventListener('change', async () => {
  const enabled = translateToggle.checked;
  try {
    await invoke('update_settings', { translate_to_english: enabled });
  } catch (err) {
    translateToggle.checked = !enabled;
    showToast(`Failed: ${err}`, 'error');
  }
});

async function loadAudioDevices(selectedDevice = '') {
  try {
    const devices = await invoke('list_audio_devices');
    audioDeviceSelect.innerHTML = '';

    const defaultOpt = document.createElement('option');
    defaultOpt.value = '';
    defaultOpt.textContent = 'System default';
    audioDeviceSelect.appendChild(defaultOpt);

    for (const d of devices) {
      const opt = document.createElement('option');
      opt.value = d;
      opt.textContent = d;
      audioDeviceSelect.appendChild(opt);
    }

    audioDeviceSelect.value = devices.includes(selectedDevice) ? selectedDevice : '';
  } catch (err) {
    console.warn('Failed to list audio devices:', err);
  }
}

audioDeviceSelect.addEventListener('change', async () => {
  try {
    await invoke('update_settings', { audio_device: audioDeviceSelect.value });
    showToast(
      audioDeviceSelect.value ? `Mic: ${audioDeviceSelect.value}` : 'Mic: system default',
      'success'
    );
  } catch (err) {
    showToast(`Failed: ${err}`, 'error');
  }
});

phrasePauseRange.addEventListener('input', () => {
  phrasePauseValue.textContent = `${parseFloat(phrasePauseRange.value).toFixed(1)}s`;
});

phrasePauseRange.addEventListener('change', async () => {
  try {
    await invoke('update_settings', { phrase_pause_secs: parseFloat(phrasePauseRange.value) });
  } catch (err) {
    showToast(`Failed: ${err}`, 'error');
  }
});

// Mic Sensitivity. The slider is a 0-100 "sensitivity" percent; higher
// sensitivity maps to a lower VAD threshold (picks up quieter speech).
// 0% -> 0.60 (strict), 100% -> 0.10 (very sensitive).
const SENS_MIN_THRESHOLD = 0.10;
const SENS_MAX_THRESHOLD = 0.60;

function sensitivityToThreshold(pct) {
  const t = SENS_MAX_THRESHOLD - (pct / 100) * (SENS_MAX_THRESHOLD - SENS_MIN_THRESHOLD);
  return Math.round(t * 100) / 100;
}

function thresholdToSensitivity(threshold) {
  const clamped = Math.min(SENS_MAX_THRESHOLD, Math.max(SENS_MIN_THRESHOLD, threshold));
  const pct = (SENS_MAX_THRESHOLD - clamped) / (SENS_MAX_THRESHOLD - SENS_MIN_THRESHOLD) * 100;
  return Math.round(pct / 5) * 5; // snap to the slider's step
}

micSensitivityRange.addEventListener('input', () => {
  micSensitivityValue.textContent = `${micSensitivityRange.value}%`;
});

micSensitivityRange.addEventListener('change', async () => {
  const threshold = sensitivityToThreshold(parseInt(micSensitivityRange.value, 10));
  try {
    await invoke('update_settings', { vad_threshold: threshold });
    showToast('Mic sensitivity updated', 'success');
  } catch (err) {
    showToast(`Failed: ${err}`, 'error');
  }
});

sessionTimeoutRange.addEventListener('input', () => {
  sessionTimeoutValue.textContent = `${sessionTimeoutRange.value}s`;
});

sessionTimeoutRange.addEventListener('change', async () => {
  try {
    await invoke('update_settings', {
      session_timeout_secs: parseInt(sessionTimeoutRange.value, 10),
    });
  } catch (err) {
    showToast(`Failed: ${err}`, 'error');
  }
});

clickToStopToggle.addEventListener('change', async () => {
  const enabled = clickToStopToggle.checked;
  try {
    await invoke('update_settings', { click_to_stop: enabled });
  } catch (err) {
    clickToStopToggle.checked = !enabled;
    showToast(`Failed: ${err}`, 'error');
  }
});

showWidgetToggle.addEventListener('change', async () => {
  const visible = showWidgetToggle.checked;
  try {
    await invoke('set_widget_visible', { visible });
  } catch (err) {
    showWidgetToggle.checked = !visible;
    showToast(`Failed: ${err}`, 'error');
  }
});

activationModeSelect.addEventListener('change', async () => {
  const mode = activationModeSelect.value;
  try {
    await invoke('update_settings', { activation_mode: mode });
    showToast(mode === 'hold' ? 'Push-to-talk enabled' : 'Toggle mode enabled', 'success');
  } catch (err) {
    showToast(`Failed: ${err}`, 'error');
  }
});

doubleTapKeySelect.addEventListener('change', async () => {
  try {
    await invoke('update_settings', { double_tap_key: doubleTapKeySelect.value });
    showToast('Activation key updated', 'success');
  } catch (err) {
    showToast(`Failed: ${err}`, 'error');
  }
});

soundFeedbackToggle.addEventListener('change', async () => {
  const enabled = soundFeedbackToggle.checked;
  try {
    await invoke('update_settings', { sound_feedback: enabled });
  } catch (err) {
    soundFeedbackToggle.checked = !enabled;
    showToast(`Failed: ${err}`, 'error');
  }
});

livePreviewToggle.addEventListener('change', async () => {
  const enabled = livePreviewToggle.checked;
  try {
    await invoke('update_settings', { live_preview: enabled });
  } catch (err) {
    livePreviewToggle.checked = !enabled;
    showToast(`Failed: ${err}`, 'error');
  }
});

captionPositionSelect.addEventListener('change', async () => {
  const caption_position = captionPositionSelect.value;
  try {
    await invoke('update_settings', { caption_position });
    showToast(`Live caption: ${captionPositionSelect.options[captionPositionSelect.selectedIndex].text}`, 'success');
  } catch (err) {
    showToast(`Failed: ${err}`, 'error');
  }
});

vocabularyInput.addEventListener('input', () => {
  vocabularySave.disabled = false;
});

vocabularySave.addEventListener('click', async () => {
  const words = vocabularyInput.value
    .split('\n')
    .map(w => w.trim())
    .filter(w => w.length > 0);
  vocabularySave.disabled = true;
  try {
    await invoke('update_settings', { custom_vocabulary: words });
    showToast(`Dictionary saved (${words.length} ${words.length === 1 ? 'term' : 'terms'})`, 'success');
  } catch (err) {
    vocabularySave.disabled = false;
    showToast(`Failed: ${err}`, 'error');
  }
});

// Parse "trigger = expansion" lines into snippet objects. Only the first '='
// splits, so an expansion can itself contain '='.
function parseSnippets(text) {
  return text
    .split('\n')
    .map(line => {
      const eq = line.indexOf('=');
      if (eq === -1) return null;
      const trigger = line.slice(0, eq).trim();
      const expansion = line.slice(eq + 1).trim();
      return trigger && expansion ? { trigger, expansion } : null;
    })
    .filter(Boolean);
}

snippetsInput.addEventListener('input', () => {
  snippetsSave.disabled = false;
});

snippetsSave.addEventListener('click', async () => {
  const snippets = parseSnippets(snippetsInput.value);
  snippetsSave.disabled = true;
  try {
    await invoke('update_settings', { snippets });
    showToast(`Snippets saved (${snippets.length} ${snippets.length === 1 ? 'snippet' : 'snippets'})`, 'success');
  } catch (err) {
    snippetsSave.disabled = false;
    showToast(`Failed: ${err}`, 'error');
  }
});

const OUTPUT_MODES = ['auto', 'keyboard', 'clipboard_paste', 'clipboard'];

// Render a profile object back to its "app = options" line.
function formatAppProfile(p) {
  const opts = [];
  if (p.developer_mode === true) opts.push('dev');
  else if (p.developer_mode === false) opts.push('plain');
  if (p.output_mode) opts.push(p.output_mode);
  return opts.length ? `${p.app} = ${opts.join(', ')}` : p.app;
}

// Parse "app = dev, clipboard_paste" lines into profile objects. Unknown
// tokens are ignored; a line needs an app and at least one valid override.
function parseAppProfiles(text) {
  return text
    .split('\n')
    .map(line => {
      const eq = line.indexOf('=');
      if (eq === -1) return null;
      const app = line.slice(0, eq).trim();
      if (!app) return null;
      const tokens = line.slice(eq + 1).split(',').map(t => t.trim().toLowerCase()).filter(Boolean);
      let output_mode = null;
      let developer_mode = null;
      for (const t of tokens) {
        if (t === 'dev' || t === 'developer') developer_mode = true;
        else if (t === 'plain' || t === 'nodev') developer_mode = false;
        else if (OUTPUT_MODES.includes(t)) output_mode = t;
      }
      if (output_mode === null && developer_mode === null) return null;
      return { app, output_mode, developer_mode };
    })
    .filter(Boolean);
}

appProfilesInput.addEventListener('input', () => {
  appProfilesSave.disabled = false;
});

appProfilesSave.addEventListener('click', async () => {
  const appProfiles = parseAppProfiles(appProfilesInput.value);
  appProfilesSave.disabled = true;
  try {
    await invoke('update_settings', { app_profiles: appProfiles });
    showToast(`Profiles saved (${appProfiles.length} ${appProfiles.length === 1 ? 'profile' : 'profiles'})`, 'success');
  } catch (err) {
    appProfilesSave.disabled = false;
    showToast(`Failed: ${err}`, 'error');
  }
});

// ─── Model picker ────────────────────────────────────────────────────────

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
    modelList.appendChild(buildModelCard(m));
  }
}

function buildModelCard(m) {
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
  for (const label of [m.backend, `${m.size_mb} MB`, m.downloaded ? 'downloaded' : null]) {
    if (!label) continue;
    const badge = document.createElement('span');
    badge.className = 'model-card__badge';
    badge.textContent = label;
    meta.appendChild(badge);
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
  } else {
    btn.textContent = m.downloaded ? 'Switch' : 'Download & Switch';
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
  return card;
}

async function handleChangeModel(modelId) {
  changingModelId = modelId;
  for (const btn of modelList.querySelectorAll('.model-card__btn')) {
    btn.disabled = true;
  }
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
    // The new model may differ in multilingual support; refresh the controls.
    invoke('get_status')
      .then(s => applyMultilingualState(!!s.model_multilingual))
      .catch(() => {});
  } else {
    modelReady = false;
    modelInfo.textContent = `Loading: ${data.model_name}...`;
    micBtn.disabled = true;
  }
});

listen('model-download-progress', (event) => {
  const data = event.payload;
  const inlineProgress = () =>
    changingModelId
      ? modelList.querySelector(`.model-card__progress[data-model-id="${changingModelId}"]`)
      : null;

  if (data.error) {
    modelBanner.hidden = false;
    modelBannerText.textContent = data.message || 'Download failed';
    modelProgressWrap.hidden = true;
    modelProgressPct.hidden = true;
    const progressEl = inlineProgress();
    if (progressEl) progressEl.hidden = true;
    if (changingModelId) {
      changingModelId = null;
      loadModelList();
    }
    return;
  }

  if (data.done) {
    modelReady = true;
    modelBanner.hidden = true;
    micBtn.disabled = uiState === 'processing';
    const progressEl = inlineProgress();
    if (progressEl) progressEl.hidden = true;
    return;
  }

  modelBanner.hidden = false;
  modelBannerText.textContent = data.message || 'Downloading...';
  modelProgressWrap.hidden = false;
  modelProgressPct.hidden = false;
  modelProgressFill.style.width = `${data.percent}%`;
  modelProgressPct.textContent = `${data.percent}%`;

  const progressEl = inlineProgress();
  if (progressEl) {
    progressEl.hidden = false;
    const fill = progressEl.querySelector('.model-card__progress-fill');
    if (fill) fill.style.width = `${data.percent}%`;
  }
});
