// Transcription display, history, and recording/streaming event handling.

// Load (optionally filtered) history from the persistent backend store.
async function loadHistory() {
  try {
    const result = await invoke('get_history', { query: historyQuery, limit: 200 });
    history = result.entries || [];
  } catch (err) {
    console.error('Failed to load history:', err);
    history = [];
  }
  renderHistory();
}

let historySearchTimer = null;
function onHistorySearch(value) {
  historyQuery = value;
  clearTimeout(historySearchTimer);
  historySearchTimer = setTimeout(loadHistory, 150);
}

async function clearHistory() {
  try {
    await invoke('clear_history');
    history = [];
    renderHistory();
    showToast('History cleared', 'success');
  } catch (err) {
    showToast(`Failed: ${err}`, 'error');
  }
}

function relativeTime(timestampMs) {
  const delta = Math.floor((Date.now() - timestampMs) / 1000);
  if (delta < 60)   return 'just now';
  if (delta < 3600) return `${Math.floor(delta / 60)}m ago`;
  if (delta < 86400) return `${Math.floor(delta / 3600)}h ago`;
  return `${Math.floor(delta / 86400)}d ago`;
}

// Strip the ".exe" and path from a process name for a cleaner app label.
function appLabel(app) {
  if (!app) return '';
  return app.replace(/\.exe$/i, '');
}

function renderHistory() {
  historyList.innerHTML = '';

  if (history.length === 0) {
    historyCount.hidden = true;
    const li = document.createElement('li');
    li.className = 'history-empty';
    li.textContent = historyQuery ? 'No matches.' : 'No history yet.';
    historyList.appendChild(li);
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
      ? entry.text.slice(0, 60) + '…'
      : entry.text;
    textSpan.title = entry.text;

    const meta = document.createElement('span');
    meta.className = 'history-item__time';
    const label = appLabel(entry.app);
    meta.textContent = label
      ? `${label} · ${relativeTime(entry.timestamp_ms)}`
      : relativeTime(entry.timestamp_ms);

    const copyBtn = document.createElement('button');
    copyBtn.className = 'history-item__copy';
    copyBtn.textContent = 'Copy';
    copyBtn.setAttribute('aria-label', 'Copy this entry');
    copyBtn.addEventListener('click', () => copyToClipboard(entry.text, copyBtn));

    li.appendChild(textSpan);
    li.appendChild(meta);
    li.appendChild(copyBtn);
    historyList.appendChild(li);
  }
}

copyTranscription.addEventListener('click', () => {
  if (!lastTranscription || !navigator.clipboard) return;
  const svgEl = copyTranscription.querySelector('svg');
  navigator.clipboard.writeText(lastTranscription).then(() => {
    copyTranscription.innerHTML = '✓';
    setTimeout(() => {
      copyTranscription.innerHTML = '';
      if (svgEl) copyTranscription.appendChild(svgEl);
    }, 1200);
  }).catch(err => console.warn('Copy transcription failed:', err));
});

historyToggle.addEventListener('click', () => {
  const expanded = historyToggle.getAttribute('aria-expanded') === 'true';
  const nowExpanded = !expanded;
  historyToggle.setAttribute('aria-expanded', String(nowExpanded));
  historyList.hidden = !nowExpanded;
  if (historyControls) historyControls.hidden = !nowExpanded;
  if (nowExpanded) loadHistory();
});

if (historySearch) {
  historySearch.addEventListener('input', () => onHistorySearch(historySearch.value));
}
if (historyClear) {
  historyClear.addEventListener('click', clearHistory);
}

listen('recording-state', (event) => {
  const { recording, processing } = event.payload;
  if (recording) {
    // Reset the UI only on a fresh start — processing updates during
    // streaming must not clear accumulated transcription text.
    if (uiState !== 'recording' && uiState !== 'processing') {
      transcriptionOutput.innerHTML = '';
      lastTranscription = '';
      sessionPhrases = [];
      interimText = '';
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
    applyState('recording');
  } else if (processing) {
    applyState('processing');
    stopDurationTimer();
    stopVisualization();
  } else if (uiState === 'processing' || uiState === 'recording') {
    stopDurationTimer();
    stopVisualization();
    applyState('idle');
  }
});

listen('hotkey-error', (event) => {
  stopDurationTimer();
  stopVisualization();
  showToast(event.payload.error || 'Error', 'error');
  applyState('error');
});

listen('transcription-error', (event) => {
  const msg = event?.payload?.error || 'Transcription issue';
  const now = Date.now();

  // Avoid spamming the same warning repeatedly during a session.
  if (msg === lastTranscriptionError && now - lastTranscriptionErrorAt < 2000) return;
  lastTranscriptionError = msg;
  lastTranscriptionErrorAt = now;

  showToast(msg, 'error', 3500);

  if (uiState !== 'recording' && uiState !== 'processing') {
    applyState('error');
    setTimeout(() => {
      if (uiState === 'error') applyState('idle');
    }, 1500);
  }
});

// Rebuild the transcript preview from the accumulated phrase segments.
// '\n' segments render as line breaks; text segments join with spaces.
function renderSessionTranscript() {
  let html = '';
  let needsSpace = false;
  for (const seg of sessionPhrases) {
    if (seg === '\n') {
      html += '<br>';
      needsSpace = false;
    } else {
      if (needsSpace) html += ' ';
      html += escapeHtml(seg);
      needsSpace = true;
    }
  }
  // Interim text streams in dimmed, ahead of the confirmed phrases, so the
  // panel reads as a single sentence forming live.
  if (interimText) {
    if (needsSpace) html += ' ';
    html += `<span class="interim">${escapeHtml(interimText)}</span>`;
  }
  transcriptionOutput.innerHTML = html || '<span class="placeholder">Listening…</span>';
  lastTranscription = sessionPhrases.filter(s => s !== '\n').join(' ');
  copyTranscription.disabled = lastTranscription.length === 0;
}

function escapeHtml(s) {
  const div = document.createElement('div');
  div.textContent = s;
  return div.innerHTML;
}

listen('streaming-partial', (event) => {
  const text = event.payload?.text;
  if (!text) return;
  // Only meaningful mid-recording; ignore late partials once we've stopped.
  if (uiState !== 'recording') return;
  interimText = text;
  renderSessionTranscript();
});

listen('streaming-phrase', (event) => {
  const { text, processing_time_ms } = event.payload;
  if (!text) return;

  // The confirmed phrase supersedes whatever interim text was showing.
  interimText = '';
  sessionPhrases.push(text);
  renderSessionTranscript();

  if (currentSession) {
    currentSession.phraseCount++;
    currentSession.wordCount += text.trim().split(/\s+/).length;
    currentSession.phraseTimestamps.push(Date.now());
    if (processing_time_ms != null) {
      currentSession.processingTimes.push(processing_time_ms);
    }
  }

  applyState('recording');
});

listen('voice-command', (event) => {
  const command = event.payload?.command;
  if (command === 'new line') {
    sessionPhrases.push('\n');
  } else if (command === 'new paragraph') {
    sessionPhrases.push('\n', '\n');
  } else if (command === 'scratch that') {
    // Remove trailing line breaks, then the last text segment.
    while (sessionPhrases.length && sessionPhrases[sessionPhrases.length - 1] === '\n') {
      sessionPhrases.pop();
    }
    sessionPhrases.pop();
  }
  renderSessionTranscript();
  applyState('recording');
});

listen('streaming-done', () => {
  stopDurationTimer();
  stopVisualization();
  interimText = '';

  const finalText = lastTranscription.trim();
  if (finalText) {
    const words = finalText.split(/\s+/).length;
    wordCount.textContent = `${words} word${words !== 1 ? 's' : ''}`;
    loadHistory();
    applyState('done');
    setTimeout(() => {
      if (uiState === 'done') applyState('idle');
    }, 2000);
  } else {
    applyState('idle');
  }

  if (currentSession) {
    currentSession.endTime = Date.now();
    finalizeSessionAnalytics(currentSession);
    currentSession = null;
  }
});

