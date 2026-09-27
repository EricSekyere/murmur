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
      case 'window_menu_groups': return Object.entries(previewMenus()).map(([label, items]) => ({ id: label, label, native: ['Edit', 'Murmur', 'Window'].includes(label), items: items.map(item => item ? { kind: 'command', id: `app:${item[1]}`, label: item[0], accelerator: item[2], enabled: item[1] !== 'copy-last' || entries.length > 0 } : { kind: 'separator' }) }));
      case 'invoke_menu_command': window.previewWindowActions.push(args.id); return previewMenuAction(args.id.replace(/^app:/, ''), args.id.replace(/^app:/, ''));
      case 'initialize_window_chrome': return params.get('platform') || 'windows';
      case 'show_window_menu': window.previewWindowActions.push('menu'); return showPreviewMenu(args);
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
  // Mirror the native menu for browser review; production still uses src/menu.rs.
  function previewMenus() {
    const mac = params.get('platform') === 'macos';
    const menus = {
      File: [['Settings', 'settings', 'Ctrl+,'], null, ['Quit', 'quit', 'Ctrl+Q']],
      Dictation: [[status.recording ? 'Stop dictation' : 'Start dictation', 'dictate', 'Ctrl+D'], ['Transcribe a File...', 'transcribe-file', 'Ctrl+O'], ['Command Mode', 'command-mode'], ['Copy Last Transcript', 'copy-last', 'Ctrl+Shift+C'], null, ['Start Meeting', 'meeting']],
      Edit: [['Undo', 'edit:undo', 'Ctrl+Z'], ['Redo', 'edit:redo', mac ? 'Ctrl+Shift+Z' : 'Ctrl+Y'], null, ['Cut', 'edit:cut', 'Ctrl+X'], ['Copy', 'edit:copy', 'Ctrl+C'], ['Paste', 'edit:paste', 'Ctrl+V'], null, ['Select All', 'edit:selectAll', 'Ctrl+A']],
      View: [['Home', 'view:home', 'Ctrl+1'], ['Analytics', 'view:analytics', 'Ctrl+2'], ['Settings', 'view:settings', 'Ctrl+3'], ['Diagnostics', 'view:diagnostics', 'Ctrl+4'], ['Help', 'view:help', 'Ctrl+5'], null, ['Toggle Floating Pill', 'pill']],
      Help: [['Help Center', 'help-center', 'F1'], null, ['Report a Problem', 'report'], ['Project Page', 'project'], null, ['Check for Updates...', 'check-updates'], ['About Murmur', 'about']],
    };
    if (!mac) return menus;
    return {
      Murmur: [['About Murmur', 'about'], ['Check for Updates...', 'check-updates'], null, ['Settings...', 'settings', 'Ctrl+,'], null, ['Services', 'services'], null, ['Hide Murmur', 'hide', 'Ctrl+H'], ['Hide Others', 'hide-others', 'Ctrl+Alt+H'], ['Show All', 'show-all'], null, ['Quit Murmur', 'quit', 'Ctrl+Q']],
      Dictation: menus.Dictation, Edit: menus.Edit, View: menus.View,
      Window: [['Minimize', 'minimize', 'Ctrl+M'], ['Maximize', 'maximize']],
      Help: menus.Help.slice(0, 4),
    };
  }

  function previewMenuAction(id, label) {
    if (id === 'settings' || id.startsWith('view:') || id === 'help-center') {
      const view = id === 'settings' ? 'settings' : id === 'help-center' ? 'help' : id.slice(5);
      document.querySelector(`.nav__item[data-view="${view}"]`).click();
    } else if (id === 'dictate') document.getElementById('mic-btn').click();
    else if (id === 'pill') document.getElementById('find-pill-btn').click();
    else if (id === 'about') emit('show-about', {});
    else showToast(`${label} is available in the desktop app. This preview uses sample data.`, 'success', 4000);
  }

  function previewMenuButton(label, shortcut) {
    const button = document.createElement('button');
    button.type = 'button'; button.role = 'menuitem';
    const text = document.createElement('span'); text.textContent = label; button.append(text);
    if (shortcut) {
      const key = document.createElement('kbd');
      key.textContent = params.get('platform') === 'macos' ? shortcut.replace('Ctrl', '\u2318') : shortcut;
      button.append(key);
    }
    return button;
  }

  function previewMenuKeys(event, panel) {
    const items = [...panel.querySelectorAll('button')];
    const index = items.indexOf(document.activeElement);
    const positions = { ArrowDown: (index + 1) % items.length, ArrowUp: (index + items.length - 1) % items.length, Home: 0, End: items.length - 1 };
    if (!(event.key in positions)) return;
    event.preventDefault(); items[positions[event.key]].focus();
  }

  function previewSubmenu(panel, items, close) {
    panel.replaceChildren();
    for (const item of items) {
      if (!item) { const separator = document.createElement('hr'); separator.role = 'separator'; panel.append(separator); continue; }
      const [label, id, shortcut] = item;
      const button = previewMenuButton(label, shortcut);
      button.dataset.command = id;
      button.addEventListener('click', () => { close(); previewMenuAction(id, label); });
      panel.append(button);
    }
  }

  function showPreviewMenu({ x, y, menuId }) {
    document.getElementById('preview-app-menu')?.dismiss();
    return new Promise(resolve => {
      const trigger = document.getElementById('window-menu');
      const popup = document.createElement('div'); popup.id = 'preview-app-menu'; popup.role = 'menu';
      popup.setAttribute('aria-label', 'Application menu'); popup.style.left = `${Math.max(8, x)}px`; popup.style.top = `${y}px`;
      const submenu = document.createElement('div'); submenu.id = 'preview-app-submenu'; submenu.role = 'menu'; submenu.hidden = true;
      const abort = new AbortController(); const options = { signal: abort.signal };
      let selected;
      const close = (restoreFocus = true) => { popup.remove(); submenu.remove(); abort.abort(); if (restoreFocus) trigger.focus(); resolve(); };
      popup.dismiss = () => close(false);
      const select = (button, label, items, focus = false) => {
        selected = button; popup.querySelectorAll('button').forEach(b => b.setAttribute('aria-expanded', String(b === button)));
        submenu.setAttribute('aria-label', label); previewSubmenu(submenu, items, close); submenu.hidden = false;
        const bounds = button.getBoundingClientRect();
        submenu.style.left = `${Math.min(popup.getBoundingClientRect().right + 4, innerWidth - 288)}px`;
        submenu.style.top = `${Math.max(8, Math.min(bounds.top, innerHeight - submenu.offsetHeight - 8))}px`;
        if (focus) submenu.querySelector('button').focus();
      };
      for (const [label, items] of Object.entries(previewMenus())) {
        const button = previewMenuButton(label, '\u203a'); button.setAttribute('aria-haspopup', 'menu'); button.setAttribute('aria-expanded', 'false');
        button.addEventListener('click', () => select(button, label, items, true));
        button.addEventListener('pointerenter', () => select(button, label, items));
        popup.append(button);
      }
      document.getElementById('app').append(popup, submenu); popup.querySelector('button').focus();
      if (menuId) { const group = [...popup.querySelectorAll('button')].find(button => button.querySelector('span').textContent === menuId); group?.click(); popup.style.visibility = 'hidden'; submenu.style.left = `${Math.min(x, innerWidth - 288)}px`; submenu.style.top = `${y}px`; }
      popup.addEventListener('keydown', event => {
        previewMenuKeys(event, popup);
        if (event.key === 'ArrowRight') { event.preventDefault(); document.activeElement.click(); }
        if (event.key === 'Escape' || event.key === 'Tab') { close(); if (event.key === 'Escape') event.preventDefault(); }
      });
      submenu.addEventListener('keydown', event => {
        previewMenuKeys(event, submenu);
        if (event.key === 'Escape' || event.key === 'ArrowLeft') { event.preventDefault(); if (menuId) { close(); return; } submenu.hidden = true; selected.setAttribute('aria-expanded', 'false'); selected.focus(); }
        if (event.key === 'Tab') close();
      });
      document.addEventListener('pointerdown', event => { if (!popup.contains(event.target) && !submenu.contains(event.target)) close(false); }, options);
      window.addEventListener('blur', () => close(false), options);
    });
  }

  window.previewWindowActions = [];
  let maximized = false;
  const mockWindow = {
    setSize: async () => {}, startDragging: async () => {},
    minimize: async () => { window.previewWindowActions.push('minimize'); },
    toggleMaximize: async () => { maximized = !maximized; window.previewWindowActions.push('maximize'); },
    isMaximized: async () => maximized,
    close: async () => { window.previewWindowActions.push('close'); },
    setTheme: async theme => { window.previewWindowActions.push(`theme:${theme}`); },
    onResized: async () => () => {}, onFocusChanged: async () => () => {},
  };
  window.__TAURI__ = { core: { invoke }, window: { getCurrentWindow: () => mockWindow, LogicalSize: class { constructor(width, height) { this.width = width; this.height = height; } } }, event: { listen: async (name, fn) => {
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

    style.textContent += '#preview-app-menu,#preview-app-submenu{position:fixed;z-index:50;width:184px;padding:6px;border:1px solid var(--color-border-hover);border-radius:8px;background:var(--color-surface-solid);box-shadow:0 12px 32px #0003;color:var(--color-text)}#preview-app-submenu{width:280px;max-height:calc(100vh - 16px);overflow-y:auto}#preview-app-menu button,#preview-app-submenu button{display:flex;align-items:center;gap:16px;width:100%;border:0;border-radius:4px;background:transparent;color:var(--color-text);padding:10px 8px;text-align:left;font:12px var(--font-sans);cursor:pointer}#preview-app-menu button:hover,#preview-app-menu button:focus,#preview-app-submenu button:hover,#preview-app-submenu button:focus,#preview-app-menu button[aria-expanded=true]{background:var(--color-nav-active);outline:none}#preview-app-menu button:focus-visible,#preview-app-submenu button:focus-visible{outline:2px solid var(--color-accent);outline-offset:-2px}#preview-app-menu kbd,#preview-app-submenu kbd{margin-left:auto;white-space:nowrap;color:var(--color-text-muted);font:10px var(--font-sans)}#preview-app-submenu hr{border:0;border-top:1px solid var(--color-border);margin:4px}';
    document.head.append(style);
    document.getElementById('window-titlebar')?.after(banner);
    if (document.getElementById('app')) {
      const pill = document.createElement('iframe');
      pill.id = 'preview-pill'; pill.className = 'preview-pill'; pill.title = 'Floating pill preview'; pill.src = '/widget.html';
      document.getElementById('app').append(pill);
    }
  });
})();
