(() => {
  const root = document.documentElement;
  const toggle = document.getElementById('workspace-toggle');
  const menu = document.getElementById('workspace-menu');
  const nativeMenu = document.getElementById('window-menu');
  const navItems = [...document.querySelectorAll('.nav__item')];
  const abort = new AbortController();
  const options = { signal: abort.signal };
  const nativeWindow = window.__TAURI__.window.getCurrentWindow();
  const unlisteners = [];
  let platform = '';
  let initialized = false;
  let initialization = Promise.resolve();

  function closeMenu(restoreFocus = false) {
    menu.hidden = true;
    toggle.setAttribute('aria-expanded', 'false');
    if (restoreFocus) toggle.focus();
  }

  function openMenu(last = false) {
    window.murmurApplicationMenu.close();
    menu.hidden = false;
    toggle.setAttribute('aria-expanded', 'true');
    const items = [...menu.children];
    const selected = items.find(item => item.getAttribute('aria-current') === 'page');
    (last ? items.at(-1) : selected || items[0]).focus();
  }

  function syncView() {
    const active = navItems.find(item => item.classList.contains('nav__item--active'));
    document.getElementById('workspace-current').textContent = active.querySelector('span').textContent;
    for (const item of menu.children) {
      item.setAttribute('aria-current', item.dataset.view === active.dataset.view ? 'page' : 'false');
    }
    closeMenu();
  }

  navItems.forEach((navItem, index) => {
    const item = document.createElement('button');
    item.type = 'button';
    item.role = 'menuitem';
    item.tabIndex = -1;
    item.dataset.view = navItem.dataset.view;
    item.append(navItem.querySelector('svg').cloneNode(true), navItem.querySelector('span').cloneNode(true));
    const shortcut = document.createElement('kbd');
    shortcut.textContent = `Ctrl ${index + 1}`;
    shortcut.setAttribute('aria-hidden', 'true');
    item.append(shortcut);
    item.addEventListener('click', () => { navItem.click(); closeMenu(true); }, options);
    menu.append(item);
  });

  toggle.addEventListener('click', () => menu.hidden ? openMenu() : closeMenu(true), options);
  toggle.addEventListener('keydown', event => {
    if (!['ArrowDown', 'ArrowUp'].includes(event.key)) return;
    event.preventDefault();
    openMenu(event.key === 'ArrowUp');
  }, options);
  menu.addEventListener('keydown', event => {
    const items = [...menu.children];
    const index = items.indexOf(document.activeElement);
    const positions = { ArrowDown: (index + 1) % items.length, ArrowUp: (index + items.length - 1) % items.length, Home: 0, End: items.length - 1 };
    if (event.key in positions) { event.preventDefault(); items[positions[event.key]].focus(); }
    if (event.key === 'Escape') { event.preventDefault(); closeMenu(true); }
    if (event.key === 'Tab') { closeMenu(true); }
  }, options);
  document.addEventListener('pointerdown', event => {
    if (!event.target.closest('.workspace-switcher')) closeMenu();
  }, options);
  window.addEventListener('workspace-view-changed', syncView, options);
  window.addEventListener('application-menu-opened', () => closeMenu(), options);
  window.addEventListener('blur', () => closeMenu(), options);
  syncView();

  async function perform(action) {
    try { await action(); } catch (error) { showToast(`Window action failed: ${error}`, 'error'); }
  }

  nativeMenu.addEventListener('click', () => { closeMenu(); window.murmurApplicationMenu.toggle(); }, options);
  document.getElementById('window-minimize').addEventListener('click', () => perform(() => nativeWindow.minimize()), options);
  document.getElementById('window-maximize').addEventListener('click', () => perform(async () => {
    await nativeWindow.toggleMaximize();
    await syncMaximized();
  }), options);
  document.getElementById('window-close').addEventListener('click', () => perform(() => nativeWindow.close()), options);

  async function syncMaximized() {
    const maximized = await nativeWindow.isMaximized();
    const button = document.getElementById('window-maximize');
    button.title = maximized ? 'Restore' : 'Maximize';
    button.setAttribute('aria-label', `${button.title} window`);
    button.querySelector('.maximize-icon').toggleAttribute('hidden', maximized);
    button.querySelector('.restore-icon').toggleAttribute('hidden', !maximized);
  }

  const themeObserver = new MutationObserver(() => perform(() => nativeWindow.setTheme(root.dataset.theme)));
  async function initialize() {
    if (initialized || abort.signal.aborted) return;
    // Native decorations remain available if initialization fails.
    try {
      const groups = await window.__TAURI__.core.invoke('window_menu_groups');
      nativeMenu.title = `Application menu: ${groups.map(group => group.label).join(', ')}`;
      platform = await window.__TAURI__.core.invoke('initialize_window_chrome');
    } catch (error) {
      if (String(error) !== 'Application menu is not ready') showToast(`Using native window controls: ${error}`, 'error');
      return;
    }
    initialized = true;
    root.dataset.platform = platform;
    document.getElementById('window-controls').hidden = platform === 'macos';
    if (platform === 'macos') {
      menu.querySelectorAll('kbd').forEach((key, index) => { key.textContent = `\u2318 ${index + 1}`; });
    }
    await syncMaximized();
    unlisteners.push(await nativeWindow.onResized(() => perform(syncMaximized)));
    unlisteners.push(await nativeWindow.onFocusChanged(event => { root.dataset.windowFocused = String(event.payload); }));
    await nativeWindow.setTheme(root.dataset.theme);
    themeObserver.observe(root, { attributes: true, attributeFilter: ['data-theme'] });
  }
  function scheduleInitialize() {
    initialization = initialization.then(initialize).catch(error => showToast(`Window integration failed: ${error}`, 'error'));
  }
  // The webview may load before setup has attached the native menu.
  window.__TAURI__.event.listen('application-menu-ready', scheduleInitialize)
    .then(off => { unlisteners.push(off); scheduleInitialize(); })
    .catch(scheduleInitialize);
  window.addEventListener('beforeunload', () => {
    abort.abort();
    themeObserver.disconnect();
    unlisteners.forEach(off => off());
  }, { once: true });
})();
