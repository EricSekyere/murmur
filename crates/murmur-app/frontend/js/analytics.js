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
    // Validate shape (not just presence): a bad array yields NaN in peak-hour math.
    if (!Array.isArray(data.hourlyWords) || data.hourlyWords.length !== 24) {
      data.hourlyWords = new Array(24).fill(0);
    } else {
      data.hourlyWords = data.hourlyWords.map(n => (Number.isFinite(n) ? n : 0));
    }
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

// All-time usage stats, derived on the backend from the local history log.
async function renderUsageStats() {
  let stats;
  try {
    stats = await invoke('get_usage_stats');
  } catch (err) {
    return; // history may be empty or unreadable; leave the zeros
  }
  const set = (id, val) => {
    const el = document.getElementById(id);
    if (el) el.textContent = val;
  };
  set('usage-total-words', formatNumber(stats.total_words || 0));
  set('usage-week-words', formatNumber(stats.words_this_week || 0));
  set('usage-streak', String(stats.day_streak || 0));
  set('usage-phrases', formatNumber(stats.total_phrases || 0));
  set('usage-unique-words', formatNumber(stats.unique_words || 0));
  set('usage-richness', `${Math.round((stats.vocabulary_richness || 0) * 100)}% richness`);
  set('usage-filler-rate', (stats.filler_rate || 0).toFixed(1));
  set('usage-filler-hint', `${formatNumber(stats.filler_count || 0)} total`);

  // Top apps → horizontal bars scaled to the busiest app (--usage-w drives the
  // fill width in CSS).
  const list = document.getElementById('usage-top-apps');
  if (list) {
    list.innerHTML = '';
    const apps = stats.top_apps || [];
    const maxPhrases = apps.reduce((m, a) => Math.max(m, a.phrases), 0) || 1;
    apps.forEach((a) => {
      const li = document.createElement('li');
      li.className = 'usage-apps__item';
      li.style.setProperty('--usage-w', `${Math.round((a.phrases / maxPhrases) * 100)}%`);
      const name = document.createElement('span');
      name.className = 'usage-apps__name';
      name.textContent = a.app;
      const count = document.createElement('span');
      count.className = 'usage-apps__count';
      count.textContent = `${formatNumber(a.phrases)} phrase${a.phrases === 1 ? '' : 's'}`;
      li.append(name, count);
      list.appendChild(li);
    });
  }

  // Per-day activity (last ~3 weeks) → sparkline bar heights + streak cells.
  const daily = Array.isArray(stats.daily_words) ? stats.daily_words : [];
  const maxDay = daily.reduce((m, w) => Math.max(m, w), 0);

  const spark = document.querySelector('#analytics-panel .usage-spark');
  if (spark) {
    spark.replaceChildren();
    daily.forEach((w) => {
      const bar = document.createElement('i');
      bar.style.height = maxDay > 0 ? `${Math.max(6, Math.round((w / maxDay) * 100))}%` : '3px';
      spark.appendChild(bar);
    });
  }

  const streak = document.querySelector('#analytics-panel .usage-streak');
  if (streak) {
    streak.replaceChildren();
    daily.forEach((w) => {
      const cell = document.createElement('i');
      // Loud days glow, quiet-but-active days fill, silent days stay empty.
      if (w > 0) cell.className = maxDay > 0 && w >= maxDay * 0.66 ? 'hi' : 'on';
      streak.appendChild(cell);
    });
  }

  // Daily word goal: the target lives in settings (get_status); today's words
  // are the last entry of the oldest-first daily series. Hidden when 0 (off).
  const goalWrap = document.getElementById('usage-goal');
  if (goalWrap) {
    let goal = 0;
    try {
      const status = await invoke('get_status');
      goal = status.daily_word_goal || 0;
    } catch {
      // Unreadable settings: treat as no goal and keep the element hidden.
    }
    if (goal > 0) {
      const todayWords = daily.length > 0 ? daily[daily.length - 1] : 0;
      const count = document.getElementById('usage-goal-count');
      if (count) {
        count.textContent = `${todayWords.toLocaleString()} / ${goal.toLocaleString()} words today`;
      }
      const fill = document.getElementById('usage-goal-fill');
      if (fill) fill.style.width = `${Math.min(100, Math.round((todayWords / goal) * 100))}%`;
    }
    goalWrap.hidden = goal <= 0;
  }
}

// Personal records, derived on the backend from the per-day insights
// aggregate (which outlives the capped history log).
async function renderRecords() {
  let records;
  try {
    records = await invoke('get_records');
  } catch (err) {
    return; // aggregate may be empty or unreadable; leave the placeholders
  }
  const set = (id, val) => {
    const el = document.getElementById(id);
    if (el) el.textContent = val;
  };
  const weekdays = ['Sunday', 'Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday'];
  const tracked = records.tracked_days || 0;
  const bestWords = records.best_day_words || 0;

  set('records-best-day', bestWords > 0 ? formatNumber(bestWords) : '--');
  set('records-best-day-date',
    bestWords > 0 ? new Date(records.best_day * 86400000).toLocaleDateString() : '--');
  set('records-streak', String(records.longest_streak || 0));
  set('records-weekday',
    tracked > 0 ? (weekdays[records.most_active_weekday] || '--') : '--');

  const note = document.getElementById('records-note');
  if (note) {
    note.textContent = tracked > 0
      ? `Reflects ${tracked} tracked day${tracked === 1 ? '' : 's'} of dictation`
      : 'Start dictating to build records';
    note.hidden = false;
  }
}

analyticsToggle.addEventListener('click', () => {
  const expanded = analyticsToggle.getAttribute('aria-expanded') === 'true';
  analyticsToggle.setAttribute('aria-expanded', String(!expanded));
  analyticsPanel.hidden = expanded;
  if (!expanded) {
    renderAnalytics();
    renderUsageStats();
    renderRecords();
  }
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
