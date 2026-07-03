// Roaming caption window: shows interim text pushed from the backend. The
// backend owns position and visibility; this only renders the text and clears
// itself if no update arrives, so a stray late partial can never make it linger.
// Interim payloads are plain strings; a final translated phrase arrives as
// { text, hold_ms } so it stays up long enough to read.
const { listen } = window.__TAURI__.event;

const box = document.getElementById('caption');
const textEl = document.getElementById('caption-text');

const DEFAULT_HOLD_MS = 1500;

let hideTimer = null;

listen('caption-text', (event) => {
  const payload = event.payload;
  const isObject = payload !== null && typeof payload === 'object';
  const text = (isObject ? payload.text : payload) || '';
  const holdMs = isObject && Number.isFinite(payload.hold_ms)
    ? payload.hold_ms
    : DEFAULT_HOLD_MS;

  textEl.textContent = text;
  box.hidden = text.length === 0;

  clearTimeout(hideTimer);
  if (text) {
    hideTimer = setTimeout(() => { box.hidden = true; }, holdMs);
  }
});
