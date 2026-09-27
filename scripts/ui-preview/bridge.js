(() => {
  const params = new URLSearchParams(location.search);
  if (params.has('onboarding')) localStorage.removeItem('murmur_onboarded');
  else localStorage.setItem('murmur_onboarded', '1');
  const listeners = new Map();
  const emit = (name, payload) => (listeners.get(name) || []).forEach(fn => fn({ payload }));
  const status = {
    model_ready: true, model: 'Parakeet TDT 0.6B v3', model_multilingual: true,
    recording: false, mode: 'idle', hotkey: 'ctrl+shift+space', output_mode: 'keyboard',
    transcription_profile: 'relaxed', language: 'en', developer_mode: false,
    activation_mode: 'toggle', double_tap_key: 'rctrl', phrase_pause_secs: 0.6,
    vad_threshold: 0.5, session_timeout_secs: 60, model_idle_unload_secs: 0,
    daily_word_goal: 1000, show_widget: true, click_to_stop: true, save_history: true,
    clean_speech: true, smart_punctuation: false, sound_feedback: true,
    live_preview: false, caption_position: 'bottom', mic_warm_start: true,
    wake_available: true, wake_downloaded: true, wake_word_sensitivity: 'medium',
    custom_vocabulary: ['TypeScript', 'PostgreSQL'], snippets: [], path_aliases: [], app_profiles: [],
    codebase_vocab_roots: [], codebase_vocab_count: 0, gpu_backend: 'none',
    app_version: '0.5.0', whats_new_seen: '0.5.0', meeting_diarization_supported: true, meeting_diarization_ready: true,
  };
  const entries = [
    { text: 'Add a loading state while the search results are being fetched.', app: 'Code.exe' },
    { text: 'The release notes are ready for review.', app: 'Notepad.exe' },
    { text: 'Schedule a follow-up to review the accessibility improvements.', app: 'Code.exe' },
  ].map((e, i) => ({ ...e, id: `sample-${i}`, timestamp_ms: Date.now() - (i + 1) * 240000, has_audio: false }));
  const models = [
    { id: 'parakeet-v3', name: 'Parakeet TDT 0.6B v3', backend: 'parakeet', description: 'Fast multilingual dictation. Runs on your CPU.', size_mb: 661, downloaded: true, active: true },
    { id: 'parakeet-v2', name: 'Parakeet TDT 0.6B v2', backend: 'parakeet', description: 'English dictation with a compact local model.', size_mb: 661, downloaded: true, active: false },
    { id: 'whisper-small', name: 'Whisper Small', backend: 'whisper', description: 'Multilingual recognition and English translation.', size_mb: 466, downloaded: false, active: false },
    { id: 'whisper-large-v3-turbo', name: 'Whisper Large v3 Turbo', backend: 'whisper', description: 'Higher capacity recognition. Requires more memory.', size_mb: 1620, downloaded: false, active: false },
  ];
  const articles = fetch('/preview-articles.json').then(r => r.json());
  const meetings = [{id: 'sample-meeting', started_ms: Date.now() - 86400000, duration_secs: 900, segments: 2}];
  const today = Math.floor(Date.now() / 86400000);
  async function invoke(command, args = {}) {
    switch (command) {
      case 'get_status': return { ...status };
      case 'get_history': return { entries: entries.filter(e => `${e.text} ${e.app}`.toLowerCase().includes((args.query || '').toLowerCase())) };
      case 'clear_history': entries.length = 0; return;
      case 'list_models': return models;
      case 'list_audio_devices': return ['Desktop microphone (sample)', 'Headset microphone (sample)'];
      case 'list_meetings': return meetings;
      case 'get_meeting': return { speakers: [0, 1], blocks: [{speaker: 0, text: 'Sample meeting: review the release plan.'}, {speaker: 1, text: 'The documentation is ready for review.'}] };
      case 'delete_meeting': meetings.length = 0; return;
      case 'get_usage_stats': return {
        total_words: 24860, words_this_week: 3420, day_streak: 7, total_phrases: 1840,
        unique_words: 2184, vocabulary_richness: 0.31, filler_rate: 1.2, filler_count: 28,
        top_apps: [{ app: 'Visual Studio Code', phrases: 942 }, { app: 'Notepad', phrases: 426 }, { app: 'Browser', phrases: 318 }],
        daily_words: [180, 430, 320, 0, 580, 680, 460, 880, 380, 0, 740, 820, 1040, 420, 580, 660, 320, 440, 510, 480, 720],
      };
      case 'get_records': return { tracked_days: 64, best_day_words: 2140, best_day: today - 5, longest_streak: 12, most_active_weekday: 2 };
      case 'get_daily_activity': return Array.from({ length: 160 }, (_, i) => ({ day: today - i, words: i % 5 ? (i * 173 + 390) % 1900 : 0 }));
      case 'about_info': return { version: '0.5.0', debug_build: false, os: 'Windows', arch: 'x86_64', gpu_backend: 'none', tauri_version: '2' };
      case 'help_ready': return true;
      case 'help_articles': return articles;
      case 'help_article': return (await articles).find(a => a.title === args.title);
      case 'help_search': return (await articles).filter(a => a.markdown.toLowerCase().includes(args.query.toLowerCase())).slice(0, 5).map(a => ({ article: a.title, heading: a.headings[0] || a.title, body: 'Open the article to read the bundled documentation.', score: 0.9 }));
      case 'update_settings': Object.assign(status, args); return;
      case 'set_developer_mode': status.developer_mode = args.enabled; return;
      case 'set_widget_visible': status.show_widget = args.visible; return;
      case 'set_codebase_vocabulary': status.codebase_vocab_enabled = args.enabled; status.codebase_vocab_roots = args.project_roots; return;
      case 'toggle_recording':
        status.recording = !status.recording;
        emit('recording-state', { recording: status.recording });
        if (!status.recording) emit('streaming-done', {});
        return;
      case 'locate_widget': document.getElementById('preview-pill')?.classList.toggle('preview-pill--visible'); return;
      case 'cancel_pending': case 'cancel_choice': return;
      case 'run_command': return args.transcript === 'preview paths'
        ? {kind: 'choose', nonce: 'preview', candidates: ['src/main.rs', 'crates/murmur-app/src/main.rs']}
        : {kind: 'pending', nonce: 'preview', tool: 'sample action', args: {text: 'Design preview only'}, reversible: true};
      case 'take_startup_notice': case 'mark_whats_new_seen': case 'set_output_suppressed': return;
      default: throw new Error('This action needs the desktop app. The design preview uses sample data only.');
    }
  }
  window.__TAURI__ = { core: { invoke }, window: { getCurrentWindow: () => ({ setSize: async () => {}, startDragging: async () => {} }), LogicalSize: class { constructor(width, height) { this.width = width; this.height = height; } } }, event: { listen: async (name, fn) => {
    const set = listeners.get(name) || []; set.push(fn); listeners.set(name, set);
    return () => listeners.set(name, set.filter(f => f !== fn));
  } } };
  window.previewEmit = emit;
  document.addEventListener('DOMContentLoaded', () => {
    const banner = document.createElement('div');
    banner.className = 'preview-notice';
    banner.innerHTML = '<span><strong>Design preview</strong> · Sample data · Changes stay in this tab</span><a href="/?onboarding=1">Review setup</a>';
    const style = document.createElement('style');
    style.textContent = '.preview-notice{display:flex;justify-content:space-between;gap:16px;padding:8px 24px;background:#20232c;border-bottom:1px solid #383e4b;color:#b8c1d1;font:12px Segoe UI,sans-serif;flex-shrink:0}.preview-notice strong{color:#e4e8f0;font-weight:600}.preview-notice a{color:#c2b9ed}.preview-pill{display:none;position:fixed;bottom:24px;right:32px;width:210px;height:70px;border:0;z-index:5;background:transparent}.preview-pill--visible{display:block}';
    if (!document.getElementById('app')) {
      style.textContent += 'html,body{background:transparent}';
      const syncScheme = () => { document.documentElement.style.colorScheme = parent.getComputedStyle(parent.document.documentElement).colorScheme; };
      syncScheme();
      const observer = new MutationObserver(syncScheme);
      observer.observe(parent.document.documentElement, { attributes: true, attributeFilter: ['data-theme'] });
      window.addEventListener('beforeunload', () => observer.disconnect(), { once: true });
    }
    document.head.append(style);
    document.getElementById('app')?.prepend(banner);
    if (document.getElementById('app')) {
      const pill = document.createElement('iframe');
      pill.id = 'preview-pill'; pill.className = 'preview-pill'; pill.title = 'Floating pill preview'; pill.src = '/widget.html';
      document.getElementById('app').append(pill);
    }
  });
})();
