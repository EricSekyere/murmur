// Auto-update banner: offers a one-click update when the backend finds one.

const updateBanner   = document.getElementById('update-banner');
const updateMessage  = document.getElementById('update-message');
const updateInstall  = document.getElementById('update-install');
const updateDismiss  = document.getElementById('update-dismiss');

listen('update-available', (event) => {
  const version = event.payload?.version;
  updateMessage.textContent = version
    ? `Version ${version} is available.`
    : 'A new version is available.';
  updateBanner.hidden = false;
});

// Update check failed (offline or feed unreachable): note it gently, once.
listen('update-check-failed', () => {
  console.warn('Update check failed; updates may not be reaching this install.');
  showToast("Couldn't check for updates", 'info', 4000);
});

updateDismiss.addEventListener('click', () => {
  updateBanner.hidden = true;
});

updateInstall.addEventListener('click', async () => {
  updateInstall.disabled = true;
  updateInstall.textContent = 'Updating…';
  try {
    // Backend downloads, installs, and relaunches — this call won't return
    // on success.
    await invoke('install_update');
  } catch (err) {
    updateInstall.disabled = false;
    updateInstall.textContent = 'Update & Restart';
    showToast(`Update failed: ${err}`, 'error');
  }
});
