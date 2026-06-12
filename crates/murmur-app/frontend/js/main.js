// App entry: mic button and initial status sync (loaded last).

micBtn.addEventListener('click', async () => {
  if (uiState === 'processing') return;

  clearError();
  try {
    await invoke('toggle_recording');
  } catch (err) {
    showError(String(err));
  }
});

async function init() {
  createVoiceBars();
  renderAnalytics();
  renderDiagnostics();
  try {
    const status = await invoke('get_status');
    updateModelBanner(status);
    if (status.hotkey) {
      hotkeyDisplay.textContent = status.hotkey;
    }
    if (status.output_mode) {
      outputModeDisplay.textContent = status.output_mode === 'stdout' ? 'auto' : status.output_mode;
    }
    if (status.transcription_profile) {
      transcriptionProfileSelect.value = status.transcription_profile;
    }
    developerModeToggle.checked = !!status.developer_mode;
    devModeBadge.hidden = !status.developer_mode;
  } catch (err) {
    console.error('Failed to get status:', err);
    updateModelBanner({ model_ready: false, model: 'small.en', recording: false, mode: 'idle' });
  }
}

init();
