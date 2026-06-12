// Voice-bar visualization driven by backend audio-level events.

const NUM_BARS = 32;
const BAR_HEIGHT_MAX = 48;
let voiceBars = [];

function createVoiceBars() {
  voiceBarsContainer.innerHTML = '';
  voiceBars = [];
  for (let i = 0; i < NUM_BARS; i++) {
    const bar = document.createElement('div');
    bar.className = 'voice-bar';
    voiceBarsContainer.appendChild(bar);
    voiceBars.push(bar);
  }
}

function resetVoiceBars() {
  for (const bar of voiceBars) {
    bar.style.height = '3px';
  }
  micWrapper.style.removeProperty('--audio-level');
}

function startVisualization() {
  visualization.hidden = false;
  if (voiceBars.length === 0) createVoiceBars();
  resetVoiceBars();
  currentRms = 0;
  targetRms = 0;
  vizActive = true;
  drawVisualization();
}

function stopVisualization() {
  vizActive = false;

  if (animationFrameHandle !== null) {
    cancelAnimationFrame(animationFrameHandle);
    animationFrameHandle = null;
  }

  currentRms = 0;
  targetRms = 0;
  visualization.hidden = true;
  levelFill.style.width = '0%';
  micQuality.hidden = true;
  resetVoiceBars();
}

function drawVisualization() {
  if (!vizActive) return;

  animationFrameHandle = requestAnimationFrame(drawVisualization);

  currentRms += (targetRms - currentRms) * 0.25;
  const level = Math.min(1, currentRms * 5);

  // Per-bar wave + jitter makes a single RMS value read as a live waveform.
  const time = performance.now() * 0.003;
  for (let i = 0; i < NUM_BARS; i++) {
    const wave = Math.sin(time + i * 0.4) * 0.3 + 0.7;
    const jitter = 0.8 + Math.random() * 0.4;
    const val = level * wave * jitter;
    const h = Math.max(3, val * BAR_HEIGHT_MAX);
    voiceBars[i].style.height = `${h}px`;
  }

  levelFill.style.width = `${(level * 100).toFixed(1)}%`;
  micWrapper.style.setProperty('--audio-level', level.toFixed(3));
}

listen('audio-level', (event) => {
  const level = event.payload;
  if (typeof level !== 'number') return;

  targetRms = level;
  diagnostics.liveRms = level;
  if (level > diagnostics.peakRms) diagnostics.peakRms = level;
  if (!diagnosticsPanel.hidden) renderDiagnostics();

  micQuality.hidden = false;
  // Thresholds assume mic-gain-normalized levels from the backend.
  if (level > 0.08) {
    micQualityText.textContent = 'Good signal';
    micQuality.className = 'mic-quality mic-quality--good';
  } else if (level > 0.02) {
    micQualityText.textContent = 'Fair signal';
    micQuality.className = 'mic-quality mic-quality--fair';
  } else {
    micQualityText.textContent = 'Low signal';
    micQuality.className = 'mic-quality mic-quality--low';
  }
});
