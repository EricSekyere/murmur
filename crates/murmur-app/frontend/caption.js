// Roaming caption window: shows interim text pushed from the backend. The
// backend owns position and visibility; this only renders the text and clears
// itself if no update arrives, so a stray late partial can never make it linger.
const { listen } = window.__TAURI__.event;

const box = document.getElementById('caption');
const textEl = document.getElementById('caption-text');

let hideTimer = null;

listen('caption-text', (event) => {
  const text = event.payload || '';
  textEl.textContent = text;
  box.hidden = text.length === 0;

  clearTimeout(hideTimer);
  if (text) {
    hideTimer = setTimeout(() => { box.hidden = true; }, 1500);
  }
});
