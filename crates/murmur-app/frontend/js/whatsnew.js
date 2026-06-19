// What's New: highlights of recent features. Auto-opens once per app version,
// and can be reopened from the Settings "What's New" button.

const WHATS_NEW = [
  ['Codebase vocabulary',
    "Point Murmur at your project folders and it learns your identifiers, so symbols like calculateTotalRevenue transcribe correctly. Add several folders; it re-indexes automatically when your code changes. Settings → Codebase Vocabulary."],
  ['Echo cancellation',
    "The microphone no longer picks up audio from your own speakers — videos, music, and calls are filtered out. On by default."],
  ['Linux builds',
    "Murmur now ships as a .deb and an AppImage, and types directly into apps on X11."],
];

function renderWhatsNew() {
  whatsNewBody.innerHTML = '';
  WHATS_NEW.forEach(([title, desc]) => {
    const row = document.createElement('div');
    row.className = 'whatsnew__item';
    const bullet = document.createElement('span');
    bullet.textContent = '›';
    const text = document.createElement('span');
    const b = document.createElement('b');
    b.textContent = `${title}. `;
    text.append(b, document.createTextNode(desc));
    row.append(bullet, text);
    whatsNewBody.appendChild(row);
  });
}

function openWhatsNew() {
  renderWhatsNew();
  if (whatsNewModal) whatsNewModal.hidden = false;
}

async function closeWhatsNew() {
  if (whatsNewModal) whatsNewModal.hidden = true;
  try {
    await invoke('mark_whats_new_seen');
  } catch (err) {
    console.error('Failed to mark what\'s new seen:', err);
  }
}

// Auto-open once when the running version differs from the last seen version.
function maybeShowWhatsNew(status) {
  if (whatsNewVersion) whatsNewVersion.textContent = `Version ${status.app_version || ''}`;
  if (status.app_version && status.app_version !== status.whats_new_seen) {
    openWhatsNew();
  }
}

if (whatsNewClose) whatsNewClose.addEventListener('click', closeWhatsNew);
if (whatsNewOk) whatsNewOk.addEventListener('click', closeWhatsNew);
if (whatsNewBtn) whatsNewBtn.addEventListener('click', openWhatsNew);
if (whatsNewModal) {
  whatsNewModal.addEventListener('click', (e) => {
    if (e.target === whatsNewModal) closeWhatsNew();
  });
}
