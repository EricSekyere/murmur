// Meeting mode UI: start/stop control, live transcript, and the saved-
// meetings list. Loaded after dom.js/ui.js (uses invoke/listen/showToast).

let meetingActive = false;

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
    li.className = 'history-item';

    const textSpan = document.createElement('span');
    textSpan.className = 'history-item__text';
    textSpan.textContent = meetingDateLabel(meeting.started_ms);

    const meta = document.createElement('span');
    meta.className = 'history-item__time';
    const segs = meeting.segments === 1 ? '1 segment' : `${meeting.segments} segments`;
    meta.textContent = `${meetingMmSs(meeting.duration_secs)} · ${segs}`;

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
    li.appendChild(exportBtn);
    li.appendChild(deleteBtn);
    meetingsList.appendChild(li);
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
