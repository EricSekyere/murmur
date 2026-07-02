// What's New: highlights of the current release. Auto-opens once per app
// version, and can be reopened from the Settings "What's New" button.
//
// The highlights come from js/whatsnew.data.js (window.WHATS_NEW_DATA), which
// the release build regenerates from the release's commits (see
// scripts/release.sh). This file only renders them.

function whatsNewItems() {
  const data = window.WHATS_NEW_DATA;
  if (data && Array.isArray(data.items) && data.items.length) return data.items;
  // Fallback when the generated data file is absent (e.g. a dev build).
  return [{ title: 'Thanks for using Murmur', body: 'See the release notes on GitHub for the full list of changes.' }];
}

function renderWhatsNew() {
  whatsNewBody.innerHTML = '';
  whatsNewItems().forEach((item) => {
    const title = item.title || '';
    const body = item.body || '';
    const row = document.createElement('div');
    row.className = 'whatsnew__item';
    const bullet = document.createElement('span');
    bullet.textContent = '›';
    const text = document.createElement('span');
    const b = document.createElement('b');
    b.textContent = body ? `${title}. ` : title;
    text.append(b);
    if (body) text.append(document.createTextNode(body));
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
