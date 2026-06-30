// What's New: highlights of recent features. Auto-opens once per app version,
// and can be reopened from the Settings "What's New" button.

const WHATS_NEW = [
  ['Ask for help by voice',
    "A new Help tab answers questions about Murmur. Type or dictate what you need and it finds the most relevant section instantly, entirely on-device and offline. Open it from the Help tab."],
  ['A fresh new look',
    "The app and the floating pill share a redesigned interface, and the dashboard now draws live charts of your words per day, your day streak, and your top apps."],
  ['Lower memory use',
    "Murmur now hands inference memory back to your system between phrases instead of holding it for the whole session, so it sits much lighter in the background."],
  ['Reliable echo cancellation',
    "On audio setups where echo cancellation could cut the microphone to silence, Murmur detects it and falls back to the raw mic, so dictation always works. Toggle it in Settings."],
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
