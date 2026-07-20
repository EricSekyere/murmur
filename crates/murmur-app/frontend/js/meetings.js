// Meeting mode UI: start/stop control, live transcript, the saved-meetings
// list with per-meeting transcript view (speaker-labeled when diarization
// ran) and on-demand summaries, plus the speaker-label model download.
// Loaded after dom.js/ui.js (uses invoke/listen/showToast).

let meetingActive = false;

// Build capabilities from get_status: which meeting affordances this build
// can honor. Buttons for missing features are simply not rendered.
const meetingCaps = { summarize: false, diarSupported: false, diarReady: false };

async function loadMeetingCaps() {
  try {
    const status = await invoke('get_status');
    meetingCaps.summarize = !!status.meeting_summarize_available;
    meetingCaps.diarSupported = !!status.meeting_diarization_supported;
    meetingCaps.diarReady = !!status.meeting_diarization_ready;
  } catch (err) {
    console.error('Failed to load meeting capabilities:', err);
  }
  updateDiarizationUi();
  // The list may have rendered before the capabilities arrived; re-render so
  // per-meeting buttons (Summarize) match what the build supports.
  loadMeetings();
}

function updateDiarizationUi() {
  meetingDiarRow.hidden = !meetingCaps.diarSupported;
  meetingDiarBtn.hidden = meetingCaps.diarReady;
  meetingDiarReady.hidden = !meetingCaps.diarReady;
}

meetingDiarBtn.addEventListener('click', async () => {
  meetingDiarBtn.disabled = true;
  meetingDiarBtn.classList.add('is-downloading');
  const label = meetingDiarBtn.textContent;
  meetingDiarBtn.textContent = 'Downloading…';
  try {
    await invoke('download_diarization_model');
    meetingCaps.diarReady = true;
    showToast('Speaker labels ready', 'success');
  } catch (err) {
    showToast(`Speaker-label download failed: ${err}`, 'error');
  } finally {
    meetingDiarBtn.disabled = false;
    meetingDiarBtn.classList.remove('is-downloading');
    meetingDiarBtn.textContent = label;
    updateDiarizationUi();
  }
});

function meetingMmSs(totalSecs) {
  const s = Math.max(0, Math.floor(totalSecs));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;
}

function setMeetingUiActive(active, systemAudio) {
  meetingActive = active;
  meetingStartBtn.textContent = active ? 'Stop Meeting' : 'Start Meeting';
  meetingIndicator.hidden = !active;
  meetingLive.hidden = !active && meetingLive.childElementCount === 0;
  if (active) {
    meetingAudioBadge.textContent = systemAudio ? 'mic + system audio' : 'mic only';
  }
}

meetingStartBtn.addEventListener('click', async () => {
  meetingStartBtn.disabled = true;
  try {
    if (meetingActive) {
      await invoke('stop_meeting');
      showToast('Meeting saved', 'success');
      loadMeetings();
    } else {
      meetingLive.innerHTML = '';
      meetingElapsed.textContent = '0:00';
      await invoke('start_meeting');
    }
  } catch (err) {
    showToast(String(err), 'error');
  } finally {
    meetingStartBtn.disabled = false;
  }
});

listen('meeting-state', (event) => {
  const { active, system_audio, duration_secs } = event.payload;
  setMeetingUiActive(active, system_audio);
  if (active) {
    meetingElapsed.textContent = meetingMmSs(duration_secs);
  } else {
    loadMeetings();
  }
});

listen('meeting-segment', (event) => {
  const { start_secs, text } = event.payload;
  if (!text) return;
  const line = document.createElement('div');
  line.className = 'meeting-live__line';
  line.textContent = `[${meetingMmSs(start_secs)}] ${text}`;
  meetingLive.hidden = false;
  meetingLive.appendChild(line);
  meetingLive.scrollTop = meetingLive.scrollHeight;
});

async function loadMeetings() {
  let meetings = [];
  try {
    meetings = await invoke('list_meetings');
  } catch (err) {
    console.error('Failed to list meetings:', err);
  }
  renderMeetings(meetings);
}

function meetingDateLabel(startedMs) {
  return new Date(startedMs).toLocaleString([], {
    year: 'numeric', month: 'short', day: 'numeric',
    hour: '2-digit', minute: '2-digit',
  });
}

function renderMeetings(meetings) {
  meetingsList.innerHTML = '';
  meetingsCount.hidden = meetings.length === 0;
  meetingsCount.textContent = meetings.length;

  if (meetings.length === 0) {
    const li = document.createElement('li');
    li.className = 'history-empty';
    li.textContent = 'No meetings recorded yet.';
    meetingsList.appendChild(li);
    return;
  }

  for (const meeting of meetings) {
    const li = document.createElement('li');
    li.className = 'history-item meeting-item';

    const textSpan = document.createElement('span');
    textSpan.className = 'history-item__text';
    textSpan.textContent = meetingDateLabel(meeting.started_ms);

    const meta = document.createElement('span');
    meta.className = 'history-item__time';
    const segs = meeting.segments === 1 ? '1 segment' : `${meeting.segments} segments`;
    meta.textContent = `${meetingMmSs(meeting.duration_secs)} · ${segs}`;

    // Full-width detail area under the row: summary + transcript blocks,
    // filled lazily from get_meeting on first view.
    const detail = document.createElement('div');
    detail.className = 'meeting-detail transcription-output';
    detail.hidden = true;

    const viewBtn = document.createElement('button');
    viewBtn.className = 'history-item__copy';
    viewBtn.textContent = 'View';
    viewBtn.setAttribute('aria-label', 'View this meeting transcript');
    viewBtn.setAttribute('aria-expanded', 'false');
    viewBtn.addEventListener('click', async () => {
      if (!detail.hidden) {
        detail.hidden = true;
        viewBtn.setAttribute('aria-expanded', 'false');
        return;
      }
      try {
        const data = await invoke('get_meeting', { id: meeting.id });
        renderMeetingDetail(detail, data);
        detail.hidden = false;
        viewBtn.setAttribute('aria-expanded', 'true');
      } catch (err) {
        showToast(`Could not load meeting: ${err}`, 'error');
      }
    });

    const exportBtn = document.createElement('button');
    exportBtn.className = 'history-item__copy';
    exportBtn.textContent = 'Export';
    exportBtn.setAttribute('aria-label', 'Export this meeting as Markdown');
    exportBtn.addEventListener('click', async () => {
      try {
        const path = await invoke('export_meeting', { id: meeting.id });
        showToast(`Exported to ${path}`, 'success', 5000);
      } catch (err) {
        showToast(`Export failed: ${err}`, 'error');
      }
    });

    const deleteBtn = document.createElement('button');
    deleteBtn.className = 'history-item__copy';
    deleteBtn.textContent = 'Delete';
    deleteBtn.setAttribute('aria-label', 'Delete this meeting');
    deleteBtn.addEventListener('click', async () => {
      if (!confirm('Delete this meeting transcript? This cannot be undone.')) return;
      try {
        await invoke('delete_meeting', { id: meeting.id });
        loadMeetings();
      } catch (err) {
        showToast(`Delete failed: ${err}`, 'error');
      }
    });

    li.appendChild(textSpan);
    li.appendChild(meta);
    li.appendChild(viewBtn);
    if (meetingCaps.summarize) {
      li.appendChild(buildSummarizeButton(meeting.id, detail, viewBtn));
    }
    li.appendChild(exportBtn);
    li.appendChild(deleteBtn);
    li.appendChild(detail);
    meetingsList.appendChild(li);
  }
}

// Hidden entirely when the build lacks the llm feature (meetingCaps).
function buildSummarizeButton(id, detail, viewBtn) {
  const btn = document.createElement('button');
  btn.className = 'history-item__copy';
  btn.textContent = 'Summarize';
  btn.setAttribute('aria-label', 'Summarize this meeting with the local model');
  btn.addEventListener('click', async () => {
    btn.disabled = true;
    btn.textContent = 'Summarizing…';
    try {
      await invoke('summarize_saved_meeting', { id });
      // Re-fetch so the detail view shows exactly what was persisted.
      const data = await invoke('get_meeting', { id });
      renderMeetingDetail(detail, data);
      detail.hidden = false;
      viewBtn.setAttribute('aria-expanded', 'true');
      showToast('Summary saved', 'success');
    } catch (err) {
      showToast(`Summary failed: ${err}`, 'error');
    } finally {
      btn.disabled = false;
      btn.textContent = 'Summarize';
    }
  });
  return btn;
}

// Render one meeting's detail: the persisted summary (if any) on top, then
// the backend's precomputed blocks — "Speaker N:" prefixes when diarization
// ran, plain timestamped lines otherwise. Assignment logic lives entirely in
// the backend (get_meeting.blocks); this only formats.
function renderMeetingDetail(container, data) {
  container.innerHTML = '';
  if (data.summary) {
    const heading = document.createElement('div');
    heading.className = 'meeting-detail__heading';
    heading.textContent = 'Summary';
    const body = document.createElement('div');
    body.className = 'meeting-detail__summary';
    body.textContent = data.summary;
    container.appendChild(heading);
    container.appendChild(body);
  }
  const hasSpeakers = Array.isArray(data.speakers) && data.speakers.length > 0;
  const blocks = Array.isArray(data.blocks) ? data.blocks : [];
  if (blocks.length === 0 && !data.summary) {
    const empty = document.createElement('div');
    empty.className = 'meeting-live__line';
    empty.textContent = 'No transcript captured for this meeting.';
    container.appendChild(empty);
    return;
  }
  for (const block of blocks) {
    const line = document.createElement('div');
    line.className = 'meeting-live__line';
    if (hasSpeakers) {
      const who = block.speaker == null ? 'Unknown' : `Speaker ${block.speaker + 1}`;
      line.textContent = `${who}: ${block.text}`;
    } else {
      line.textContent = `[${meetingMmSs(block.start_secs)}] ${block.text}`;
    }
    container.appendChild(line);
  }
}

meetingsToggle.addEventListener('click', () => {
  const expanded = meetingsToggle.getAttribute('aria-expanded') === 'true';
  const nowExpanded = !expanded;
  meetingsToggle.setAttribute('aria-expanded', String(nowExpanded));
  meetingsPanel.hidden = !nowExpanded;
  if (nowExpanded) loadMeetings();
});

// The dashboard layout hides collapsible triggers and expands sections
// programmatically (see dashboard.js); expand Meetings once at startup so
// the section is reachable there too.
meetingsToggle.click();
loadMeetingCaps();
