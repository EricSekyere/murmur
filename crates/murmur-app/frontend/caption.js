// Roaming caption window: shows interim text pushed from the backend. The
// backend owns position and visibility; this only renders the text.
const { listen } = window.__TAURI__.event;

const box = document.getElementById('caption');
const textEl = document.getElementById('caption-text');

listen('caption-text', (event) => {
  const text = event.payload || '';
  textEl.textContent = text;
  box.hidden = text.length === 0;
});
