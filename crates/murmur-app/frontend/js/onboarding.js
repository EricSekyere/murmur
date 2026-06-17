// First-run onboarding: welcome, live mic test, and tips. Shown once,
// gated by a localStorage flag.

(function () {
  const SEEN_KEY = 'murmur_onboarded';
  const overlay = document.getElementById('onboarding');
  if (!overlay || localStorage.getItem(SEEN_KEY)) return;

  const panels = Array.from(overlay.querySelectorAll('.onboarding__panel'));
  const dots = Array.from(overlay.querySelectorAll('.onboarding__dot'));
  const micBtn = document.getElementById('onboarding-mic');
  const result = document.getElementById('onboarding-result');
  const hotkeyEl = document.getElementById('onboarding-hotkey');
  let micActive = false;

  // The mic test needs the model loaded; enable it only when ready.
  micBtn.disabled = true;

  function goTo(step) {
    panels.forEach(p => p.classList.toggle('onboarding__panel--active', +p.dataset.step === step));
    dots.forEach(d => d.classList.toggle('onboarding__dot--active', +d.dataset.step <= step));
  }

  function finish() {
    if (micActive) {
      invoke('toggle_recording').catch(() => {});
      micActive = false;
    }
    // Restore normal text delivery now the display-only test is over.
    invoke('set_output_suppressed', { suppressed: false }).catch(() => {});
    localStorage.setItem(SEEN_KEY, '1');
    overlay.hidden = true;
  }

  function enableMicTest(message) {
    micBtn.disabled = false;
    result.textContent = message;
  }

  overlay.querySelectorAll('[data-next]').forEach(btn => {
    btn.addEventListener('click', () => goTo(+btn.dataset.next));
  });
  document.getElementById('onboarding-skip').addEventListener('click', finish);
  document.getElementById('onboarding-done').addEventListener('click', finish);

  // Mic test reuses the real dictation pipeline. The button state is driven
  // by the recording-state event below, not toggled locally, so it stays in
  // sync if the session auto-stops on silence.
  micBtn.addEventListener('click', async () => {
    try {
      await invoke('toggle_recording');
    } catch (err) {
      result.textContent = `Could not start: ${err}`;
    }
  });

  listen('recording-state', (event) => {
    if (overlay.hidden) return;
    micActive = !!event.payload.recording;
    micBtn.className = micActive
      ? 'mic-btn mic-btn--recording onboarding__mic'
      : 'mic-btn mic-btn--idle onboarding__mic';
    if (micActive) result.textContent = 'Listening, say something then pause.';
  });

  // Show transcribed text in the test box while onboarding is open.
  listen('streaming-phrase', (event) => {
    if (overlay.hidden) return;
    const text = event.payload?.text;
    if (text) {
      result.textContent = `"${text}"`;
      result.classList.add('onboarding__result--success');
    }
  });

  // Reflect model readiness on the mic-test step.
  invoke('get_status').then(status => {
    if (status.hotkey) hotkeyEl.textContent = status.hotkey;
    if (status.model_ready) enableMicTest('Ready. Click the mic and speak.');
  }).catch(() => {});

  listen('model-download-progress', (event) => {
    if (overlay.hidden) return;
    if (event.payload?.done) enableMicTest('Ready. Click the mic and speak.');
  });

  // Run onboarding display-only so the test never types into a background app.
  invoke('set_output_suppressed', { suppressed: true }).catch(() => {});
  overlay.hidden = false;
})();
