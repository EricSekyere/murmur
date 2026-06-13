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
