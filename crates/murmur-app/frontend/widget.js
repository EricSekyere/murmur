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
const label = document.getElementById('state-label');
const caption = document.getElementById('caption');
const captionText = document.getElementById('caption-text');

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

// ─── State machine ──────────────────────────────────────────────────────────
// States map to an s-<name> class on #widget. "listening" and "recording" are
// both active dictation; the pill arms as `listening` and the backend's
// audio-signal-detected promotes it to `recording` (label → mono timer).
// Always-listening rest is `armed` (static outlined orb) — not dictation.
// Precedence matches the tray: recording > armed > idle.
let currentState = 'idle';
// Monotonic token: every state change bumps it so a delayed revert (error /
// loading flash) can detect it is stale and skip stomping a newer state.
let stateToken = 0;
let timerHandle = null;
let recordStart = 0;
let heardSpeech = false;

const ACTIVE = new Set(['listening', 'recording']);
// Worker-confirmed armed flag (wake-state / get_status.wake_armed), not the
// settings toggle. Recording wins; armed is never treated as active dictation.
let armed = false;
let recording = false;

const unlistens = [];
function onEvent(name, handler) {
  const p = listen(name, handler);
  unlistens.push(p);
  return p;
}

function restName() {
  return armed ? 'armed' : 'idle';
}

function restLabel() {
  return armed ? 'armed' : 'murmur';
}

function applyRestState() {
  applyState(restName(), restLabel());
}

/** Apply recording > armed > idle unless a session or a flash is showing. */
function syncArmedIndicator() {
  if (recording || ACTIVE.has(currentState) || currentState === 'processing') return;
  if (currentState === 'error' || currentState === 'loading') return;
  applyRestState();
}

function setLabel(text) {
  label.textContent = text;
}

function applyState(name, text) {
  stateToken += 1;
  if (name !== currentState) {
    widget.classList.remove(`s-${currentState}`);
    widget.classList.add(`s-${name}`);
    const wasActive = ACTIVE.has(currentState);
    currentState = name;
    if (ACTIVE.has(name)) {
      if (!wasActive) startRecordingUi();
    } else {
      stopRecordingUi();
      // The caption is only meaningful while actively dictating.
      hideCaption();
    }
  }
  // Only the running timer uses the mono face; every other label is prose.
  if (name !== 'recording') label.classList.remove('mono');
  if (text !== undefined) setLabel(text);
  micBtn.setAttribute('aria-label', ACTIVE.has(name) ? 'Stop recording' : 'Start recording');
}

/** Show a transient state, then fall back to armed/idle unless something newer happened. */
function flashState(name, text, ms) {
  applyState(name, text);
  const token = stateToken;
  setTimeout(() => {
    if (stateToken === token) applyRestState();
  }, ms);
}

function startRecordingUi() {
  recordStart = performance.now();
  heardSpeech = false;
  lastSignalMs = performance.now();
  widget.style.setProperty('--amp', '0');
  if (!timerHandle) timerHandle = setInterval(sessionTick, 250);
}

function stopRecordingUi() {
  if (timerHandle) {
    clearInterval(timerHandle);
    timerHandle = null;
  }
  clearWaiting();
  widget.style.setProperty('--amp', '0');
}

function updateTimer() {
  // Keep showing "listening" until the backend confirms it hears speech,
  // and don't stomp the "waiting" label while dormant.
  if (isWaitingNow() || !heardSpeech || currentState !== 'recording') return;
  const total = Math.floor((performance.now() - recordStart) / 1000);
  const mm = Math.floor(total / 60);
  const ss = String(total % 60).padStart(2, '0');
  label.classList.add('mono');
  setLabel(`${mm}:${ss}`);
}

// ─── Waiting (dormant) overlay ──────────────────────────────────────────────
// A few seconds without signal while a session is still active dims the pill
// to a "waiting" look (is-waiting class + label, so it isn't colour alone).
// The next detected signal restores the active look; session end clears it.
const WAITING_AFTER_MS = 3500;
// RMS level that counts as speech activity. Starts at the backend's default
// signal floor and is replaced by the calibrated (and adaptively lowered)
// threshold via the 'speech-threshold' event, so a quiet speaker the engine
// is actively transcribing also keeps the pill awake.
let signalLevel = 0.002;
// performance.now() of the last audible signal; the 250ms session tick
// compares against it instead of re-arming a timeout on every audio-level
// event (rapid events stay debounced, per the UI guidelines).
let lastSignalMs = 0;

onEvent('speech-threshold', (event) => {
  if (typeof event.payload === 'number' && event.payload > 0) {
    signalLevel = event.payload;
  }
});

// Contexts where the session is not paused-and-resumable dictation, so the
// dormancy "waiting" overlay would mislead: command mode (the pill may be
// awaiting a physical confirmation, not speech) and a display-only session
// (the onboarding mic test).
let commandMode = false;
let outputSuppressed = false;

function dormancySuppressed() {
  return commandMode || outputSuppressed;
}

onEvent('command-mode-changed', (event) => {
  commandMode = !!event.payload?.active;
  if (dormancySuppressed() && isWaitingNow()) leaveWaiting();
});

onEvent('output-suppressed', (event) => {
  outputSuppressed = !!event.payload?.suppressed;
  if (dormancySuppressed() && isWaitingNow()) leaveWaiting();
});

// The is-waiting class is the single source of truth for dormancy.
function isWaitingNow() {
  return widget.classList.contains('is-waiting');
}

function enterWaiting() {
  if (!ACTIVE.has(currentState)) return;
  widget.classList.add('is-waiting');
  label.classList.remove('mono');
  setLabel('waiting');
}

function leaveWaiting() {
  widget.classList.remove('is-waiting');
  if (currentState === 'listening') setLabel('listening');
  updateTimer(); // recording: restore the mono timer face immediately
}

function clearWaiting() {
  widget.classList.remove('is-waiting');
}

/** Signal heard: restore the active look and push the dormancy deadline. */
function noteSignalActivity() {
  if (!ACTIVE.has(currentState)) return;
  lastSignalMs = performance.now();
  if (isWaitingNow()) leaveWaiting();
}

/** 250ms tick while a session is active: dim to "waiting" once the dormancy
    deadline passes, then keep the mono timer painted. */
function sessionTick() {
  if (
    !isWaitingNow() &&
    !dormancySuppressed() &&
    ACTIVE.has(currentState) &&
    performance.now() - lastSignalMs >= WAITING_AFTER_MS
  ) {
    enterWaiting();
  }
  updateTimer();
}

micBtn.addEventListener('click', async () => {
  try {
    await invoke('toggle_recording');
  } catch (err) {
    console.error('toggle_recording failed:', err);
    flashState('error', 'error', 2200);
  }
});

onEvent('audio-level', (event) => {
  if (typeof event.payload !== 'number' || !ACTIVE.has(currentState)) return;
  if (event.payload >= signalLevel) noteSignalActivity();
  // Perceptual lift: speech RMS is small, so raise it toward 1 for the bars.
  const amp = Math.min(1, Math.pow(event.payload * 6, 0.75));
  widget.style.setProperty('--amp', amp.toFixed(3));
});

onEvent('audio-signal-detected', () => {
  heardSpeech = true;
  noteSignalActivity();
  if (currentState === 'listening') {
    applyState('recording');
    updateTimer();
  }
});

// When the caption roams to the active window, the pill must not grow its own.
let captionAtWindow = false;
onEvent('caption-mode', (event) => {
  captionAtWindow = !!event.payload?.at_window;
  if (captionAtWindow) hideCaption();
});

onEvent('streaming-partial', (event) => {
  const text = event.payload?.text;
  if (!text || !ACTIVE.has(currentState) || captionAtWindow) return;
  showCaption(text);
});

onEvent('streaming-phrase', (event) => {
  const text = event.payload?.text;
  if (!text || !ACTIVE.has(currentState) || captionAtWindow) return;
  // Keep the confirmed phrase on screen until the next interim replaces it.
  showCaption(text);
});

onEvent('recording-state', (event) => {
  const { recording: isRecording, processing } = event.payload;
  recording = !!isRecording;
  if (recording) {
    // Arm as listening; audio-signal-detected promotes to recording.
    if (!ACTIVE.has(currentState)) applyState('listening', 'listening');
  } else if (processing) {
    applyState('processing', 'thinking');
  } else {
    applyRestState();
  }
});

onEvent('hotkey-error', (event) => {
  const msg = event.payload?.error || '';
  // "No speech" is an expected outcome, not an error, so rest quietly.
  if (msg.includes('No speech')) {
    recording = false;
    applyRestState();
    return;
  }
  if (msg.includes('still loading')) {
    flashState('loading', 'loading model', 2200);
    return;
  }
  flashState('error', 'error', 2200);
});

onEvent('streaming-done', () => {
  recording = false;
  applyRestState();
});

// "Find pill" from the dashboard: flash the locate beacon so the user can spot
// the widget. A transient overlay class on top of the current state.
onEvent('locate-pill', () => {
  widget.classList.remove('is-locating');
  void widget.offsetWidth; // reflow so re-adding restarts the animation
  widget.classList.add('is-locating');
  setTimeout(() => widget.classList.remove('is-locating'), 2600);
});

onEvent('transcription-error', (event) => {
  const msg = event.payload?.error || 'transcription error';
  console.error('Transcription error:', msg);
  // Ignore chunk-level warnings while actively recording.
  if (ACTIVE.has(currentState)) return;
  flashState('error', 'error', 2200);
});

onEvent('wake-state', (event) => {
  armed = !!event.payload?.armed;
  syncArmedIndicator();
});

window.addEventListener('beforeunload', () => {
  for (const p of unlistens) {
    p.then((off) => off()).catch(() => {});
  }
});

// If the widget (re)loads mid-session or while armed, reflect the real
// backend state instead of assuming idle. Listeners are already live, so a
// stale snapshot must not write recording=false over a session that started
// while get_status was in flight.
(async () => {
  try {
    const status = await invoke('get_status');
    captionAtWindow = status?.caption_position === 'window';
    armed = !!status?.wake_armed;
    if (status?.recording) {
      recording = true;
      applyState('recording');
      label.classList.add('mono');
    } else {
      syncArmedIndicator();
    }
  } catch (_) {
    // Backend not ready yet, so stay idle; events will correct us.
  }
})();
