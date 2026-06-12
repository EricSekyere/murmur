// Transcription display, history, and recording/streaming event handling.

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
  procTime.textContent = processingTimeMs != null
    ? `${(processingTimeMs / 1000).toFixed(1)}s`
    : '';

  addToHistory(lastTranscription, words);
  applyState('done');

  setTimeout(() => {
    if (uiState === 'done') applyState('idle');
  }, 2000);
}

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
      ? entry.text.slice(0, 60) + '…'
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
  historyToggle.setAttribute('aria-expanded', String(!expanded));
  historyList.hidden = expanded;
});

listen('recording-state', (event) => {
  const { recording, processing } = event.payload;
  if (recording) {
    // Reset the UI only on a fresh start — processing updates during
    // streaming must not clear accumulated transcription text.
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

listen('hotkey-transcribed', (event) => {
  const data = event.payload;
  stopDurationTimer();
  stopVisualization();

  if (data.text) {
    transcriptionHandled = true;
    displayTranscription(data.text, data.processing_time_ms);
    const preview = data.text.length > 40 ? data.text.slice(0, 40) + '…' : data.text;
    showToast(`Typed: ${preview}`, 'success');
  } else {
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

listen('streaming-phrase', (event) => {
  const { text, processing_time_ms } = event.payload;
  if (!text) return;

  if (transcriptionOutput.querySelector('.placeholder')) {
    transcriptionOutput.innerHTML = '';
  }
  const existing = transcriptionOutput.textContent;
  transcriptionOutput.textContent = existing ? `${existing} ${text}` : text;
  lastTranscription = transcriptionOutput.textContent;
  copyTranscription.disabled = false;

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

  if (currentSession) {
    currentSession.endTime = Date.now();
    finalizeSessionAnalytics(currentSession);
    currentSession = null;
  }
});

listen('transcription', (event) => {
  if (transcriptionHandled) return;
  transcriptionHandled = true;
  const data = event.payload;
  displayTranscription(data.text, data.processing_time_ms);
});
