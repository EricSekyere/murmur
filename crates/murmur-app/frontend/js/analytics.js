// Session analytics (localStorage-backed) and transcription diagnostics.

const ANALYTICS_KEY = 'murmur_analytics';

function formatNumber(n) {
  if (n >= 1000) return (n / 1000).toFixed(1) + 'k';
  return String(n);
}

function formatDuration(ms) {
  const totalSecs = Math.floor(ms / 1000);
  const m = Math.floor(totalSecs / 60);
  const s = totalSecs % 60;
  return `${m}:${s.toString().padStart(2, '0')}`;
}

function loadAnalytics() {
  const raw = localStorage.getItem(ANALYTICS_KEY);
  const defaults = {
    totalWords: 0,
    totalSessions: 0,
    totalDurationMs: 0,
    totalWpmSum: 0,
    todayWords: 0,
    todaySessions: 0,
    todayDate: new Date().toDateString(),
    hourlyWords: new Array(24).fill(0),
    lastSession: null,
  };
  if (!raw) return defaults;
  try {
    const data = JSON.parse(raw);
    const today = new Date().toDateString();
    if (data.todayDate !== today) {
      data.todayWords = 0;
      data.todaySessions = 0;
      data.todayDate = today;
    }
    if (!data.hourlyWords) data.hourlyWords = new Array(24).fill(0);
    if (!data.lastSession) data.lastSession = null;
    if (!data.totalWpmSum) data.totalWpmSum = 0;
    return data;
  } catch {
    return defaults;
  }
}

function saveAnalytics(data) {
  localStorage.setItem(ANALYTICS_KEY, JSON.stringify(data));
}

function finalizeSessionAnalytics(session) {
  const durationMs = session.endTime - session.startTime;
  const wpm = durationMs > 10000
    ? Math.round((session.wordCount / durationMs) * 60000)
    : null;
  const avgLatency = session.processingTimes.length > 0
    ? Math.round(session.processingTimes.reduce((a, b) => a + b, 0) / session.processingTimes.length)
    : null;

  const analytics = loadAnalytics();
  analytics.lastSession = {
    wpm,
    phraseCount: session.phraseCount,
    wordCount: session.wordCount,
    durationMs,
    avgLatency,
  };
  analytics.totalWords += session.wordCount;
  analytics.totalSessions += 1;
  analytics.totalDurationMs += durationMs;
  if (wpm != null) analytics.totalWpmSum += wpm;
  analytics.todayWords += session.wordCount;
  analytics.todaySessions += 1;
  analytics.hourlyWords[new Date().getHours()] += session.wordCount;

  saveAnalytics(analytics);
  renderAnalytics();
}

function renderAnalytics() {
  const analytics = loadAnalytics();
  const ls = analytics.lastSession;
  const setText = (id, value) => {
    const el = document.getElementById(id);
    if (el) el.textContent = value;
  };

  setText('stat-wpm', ls && ls.wpm != null ? String(ls.wpm) : '--');
  setText('stat-phrases', ls ? String(ls.phraseCount) : '0');
  setText('stat-duration', ls ? formatDuration(ls.durationMs) : '0:00');
  setText('stat-latency', ls && ls.avgLatency != null ? `${ls.avgLatency}ms` : '--');

  setText('stat-today-words', formatNumber(analytics.todayWords));
  setText('stat-today-sessions', String(analytics.todaySessions));

  setText('stat-total-words', formatNumber(analytics.totalWords));
  setText('stat-total-sessions', String(analytics.totalSessions));

  const avgWpm = analytics.totalSessions > 0
    ? Math.round(analytics.totalWpmSum / analytics.totalSessions)
    : null;
  setText('stat-avg-wpm', avgWpm != null ? String(avgWpm) : '--');

  const avgDur = analytics.totalSessions > 0
    ? Math.round(analytics.totalDurationMs / analytics.totalSessions)
    : 0;
  setText('stat-avg-duration', formatDuration(avgDur));

  const maxWords = Math.max(...analytics.hourlyWords);
  if (maxWords > 0) {
    const peakHour = analytics.hourlyWords.indexOf(maxWords);
    const startH = peakHour % 12 || 12;
    const startP = peakHour < 12 ? 'AM' : 'PM';
    const endHour = (peakHour + 1) % 24;
    const endH = endHour % 12 || 12;
    const endP = endHour < 12 ? 'AM' : 'PM';
    setText('stat-peak-hour', `${startH}${startP} - ${endH}${endP} (${formatNumber(maxWords)} words)`);
  } else {
    setText('stat-peak-hour', 'No data yet');
  }
}

// ─── Diagnostics ─────────────────────────────────────────────────────────

function classifyDiagnosticReason(reason) {
  if (!reason) return 'other';
  if (reason.startsWith('too_short')) return 'too_short';
  if (reason === 'too_quiet') return 'too_quiet';
  if (reason.startsWith('hallucination')) return 'hallucination';
  if (reason.startsWith('engine')) return 'engine';
  if (reason === 'no_signal') return 'no_signal';
  return 'other';
}

function renderDiagnostics() {
  const setText = (id, value) => {
    const el = document.getElementById(id);
    if (el) el.textContent = value;
  };

  setText('diag-live-rms', diagnostics.liveRms.toFixed(4));
  setText('diag-peak-rms', diagnostics.peakRms.toFixed(4));
  setText('diag-accepted', String(diagnostics.accepted));
  setText('diag-rejected', String(diagnostics.rejected));
  setText('diag-reason-too-short', String(diagnostics.reasons.too_short));
  setText('diag-reason-too-quiet', String(diagnostics.reasons.too_quiet));
  setText('diag-reason-hallucination', String(diagnostics.reasons.hallucination));
  setText('diag-reason-engine', String(diagnostics.reasons.engine));
  setText('diag-reason-no-signal', String(diagnostics.reasons.no_signal));
  setText('diag-reason-other', String(diagnostics.reasons.other));
}

function resetDiagnostics() {
  diagnostics.liveRms = 0;
  diagnostics.peakRms = 0;
  diagnostics.accepted = 0;
  diagnostics.rejected = 0;
  for (const key of Object.keys(diagnostics.reasons)) {
    diagnostics.reasons[key] = 0;
  }
  renderDiagnostics();
}

listen('transcription-diagnostic', (event) => {
  const payload = event?.payload || {};
  if (payload.kind === 'accepted') {
    diagnostics.accepted += 1;
  } else if (payload.kind === 'rejected') {
    diagnostics.rejected += 1;
    diagnostics.reasons[classifyDiagnosticReason(payload.reason || '')] += 1;
  }

  if (typeof payload.rms === 'number') {
    diagnostics.liveRms = payload.rms;
    if (payload.rms > diagnostics.peakRms) diagnostics.peakRms = payload.rms;
  }

  if (!diagnosticsPanel.hidden) {
    renderDiagnostics();
  }
});

analyticsToggle.addEventListener('click', () => {
  const expanded = analyticsToggle.getAttribute('aria-expanded') === 'true';
  analyticsToggle.setAttribute('aria-expanded', String(!expanded));
  analyticsPanel.hidden = expanded;
  if (!expanded) renderAnalytics();
});

diagnosticsToggle.addEventListener('click', () => {
  const expanded = diagnosticsToggle.getAttribute('aria-expanded') === 'true';
  diagnosticsToggle.setAttribute('aria-expanded', String(!expanded));
  diagnosticsPanel.hidden = expanded;
  if (!expanded) renderDiagnostics();
});

diagnosticsReset.addEventListener('click', () => {
  resetDiagnostics();
  showToast('Diagnostics reset', 'success', 1800);
});
