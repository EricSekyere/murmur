(() => {
  const trigger = document.getElementById('window-menu');
  const lifetime = new AbortController();
  let popup;
  let submenu;
  let selected;
  let menuEvents;
  let previousFocus;
  let opening = false;
  let generation = 0;

  function closeSubmenu(restoreFocus = false) {
    submenu?.remove(); submenu = null;
    selected?.setAttribute('aria-expanded', 'false');
    if (restoreFocus) selected?.focus();
    selected = null;
  }

  function close(restoreFocus = false) {
    generation++; opening = false;
    closeSubmenu(); popup?.remove(); popup = null;
    menuEvents?.abort(); menuEvents = null;
    trigger.setAttribute('aria-expanded', 'false');
    if (restoreFocus) trigger.focus();
  }

  function button(label, hint) {
    const item = document.createElement('button');
    item.type = 'button'; item.role = 'menuitem'; item.tabIndex = -1;
    const text = document.createElement('span'); text.textContent = label; item.append(text);
    if (hint) {
      const shortcut = document.createElement('kbd'); shortcut.textContent = hint;
      shortcut.setAttribute('aria-hidden', 'true'); item.append(shortcut);
    }
    return item;
  }

  function panel(id, label, kind) {
    const element = document.createElement('div'); element.id = id;
    element.className = `application-menu application-menu--${kind}`; element.role = 'menu';
    element.setAttribute('aria-label', label);
    document.getElementById('app').append(element);
    return element;
  }

  function position(element, x, y) {
    element.style.left = `${Math.max(8, Math.min(x, innerWidth - element.offsetWidth - 8))}px`;
    element.style.top = `${Math.max(8, Math.min(y, innerHeight - element.offsetHeight - 8))}px`;
  }

  function shortcut(value) {
    return (value || '').replace('CmdOrCtrl', document.documentElement.dataset.platform === 'macos' ? '\u2318' : 'Ctrl').replace('Comma', ',');
  }

  async function nativeMenu(group, anchor = trigger.getBoundingClientRect()) {
    close();
    if (previousFocus?.isConnected) previousFocus.focus();
    try {
      await window.__TAURI__.core.invoke('show_window_menu', { menuId: group?.id || null, x: anchor.left, y: anchor.bottom + 8 });
    } catch (error) { showToast(`Could not open menu: ${error}`, 'error'); }
  }

  function openGroup(group, item, focus = true) {
    if (group.native) { nativeMenu(group, item.getBoundingClientRect()); return; }
    closeSubmenu(); selected = item; item.setAttribute('aria-expanded', 'true');
    submenu = panel('application-submenu', group.label, 'submenu');
    for (const entry of group.items) {
      if (entry.kind === 'separator') {
        const separator = document.createElement('hr'); separator.role = 'separator'; submenu.append(separator); continue;
      }
      const command = button(entry.label, shortcut(entry.accelerator));
      command.dataset.command = entry.id; command.disabled = !entry.enabled;
      command.addEventListener('click', async () => {
        close(true);
        try { await window.__TAURI__.core.invoke('invoke_menu_command', { id: entry.id }); }
        catch (error) { showToast(String(error), 'error'); }
      });
      submenu.append(command);
    }
    const root = popup.getBoundingClientRect(); const anchor = item.getBoundingClientRect();
    const x = root.right + submenu.offsetWidth + 12 <= innerWidth ? root.right + 4 : root.left - submenu.offsetWidth - 4;
    position(submenu, x, anchor.top - 6);
    submenu.addEventListener('keydown', event => handleKeys(event, submenu), { signal: menuEvents.signal });
    if (focus) submenu.querySelector('[role=menuitem]:not(:disabled)')?.focus();
  }

  function render(groups) {
    popup = panel('application-menu', 'Application menu', 'parent');
    const heading = document.createElement('div'); heading.className = 'application-menu__heading'; heading.textContent = 'Murmur'; popup.append(heading);
    for (const group of groups) {
      const item = button(group.label); item.dataset.menuId = group.id;
      item.setAttribute('aria-haspopup', 'menu'); item.setAttribute('aria-expanded', 'false');
      item.addEventListener('click', () => openGroup(group, item));
      item.addEventListener('pointerenter', event => {
        if (event.pointerType !== 'mouse' || selected === item) return;
        group.native ? closeSubmenu() : openGroup(group, item, false);
      });
      popup.append(item);
    }
    const system = button('Open system menu'); system.className = 'application-menu__system';
    system.addEventListener('click', () => nativeMenu()); system.addEventListener('pointerenter', () => closeSubmenu()); popup.append(system);
    const anchor = trigger.getBoundingClientRect(); position(popup, anchor.left, anchor.bottom + 8);
    popup.addEventListener('keydown', event => handleKeys(event, popup), { signal: menuEvents.signal });
    popup.querySelector('[role=menuitem]').focus();
  }

  function handleKeys(event, container) {
    const items = [...container.querySelectorAll('[role=menuitem]:not(:disabled)')];
    const index = items.indexOf(document.activeElement);
    const positions = { ArrowDown: (index + 1) % items.length, ArrowUp: (index + items.length - 1) % items.length, Home: 0, End: items.length - 1 };
    if (event.key in positions) { event.preventDefault(); items[positions[event.key]]?.focus(); }
    if (event.key === 'ArrowRight' && container === popup) { event.preventDefault(); document.activeElement.click(); }
    if (event.key === 'Escape' || event.key === 'ArrowLeft') {
      event.preventDefault(); container === submenu ? closeSubmenu(true) : close(true);
    }
    if (event.key === 'Tab') close(true);
    if (event.key.length === 1 && !event.ctrlKey && !event.metaKey && !event.altKey) {
      const ordered = [...items.slice(index + 1), ...items.slice(0, index + 1)];
      ordered.find(item => item.textContent.toLowerCase().startsWith(event.key.toLowerCase()))?.focus();
    }
  }

  async function toggle() {
    if (popup || opening) { close(true); return; }
    opening = true; const request = generation;
    window.dispatchEvent(new Event('application-menu-opened'));
    menuEvents = new AbortController(); const options = { signal: menuEvents.signal };
    document.addEventListener('pointerdown', event => {
      if (!popup?.contains(event.target) && !submenu?.contains(event.target) && !trigger.contains(event.target)) close();
    }, options);
    window.addEventListener('blur', () => close(), options);
    window.addEventListener('resize', () => close(), options);
    try {
      const groups = await window.__TAURI__.core.invoke('window_menu_groups');
      if (generation !== request) return;
      render(groups); trigger.setAttribute('aria-expanded', 'true');
    } catch {
      if (generation === request) await nativeMenu();
    } finally { if (generation === request) opening = false; }
  }

  trigger.addEventListener('pointerdown', () => { if (!popup) previousFocus = document.activeElement; }, { signal: lifetime.signal });
  document.addEventListener('keydown', event => {
    if (event.key === 'F10' && !event.shiftKey) { event.preventDefault(); previousFocus = document.activeElement; toggle(); }
  }, { signal: lifetime.signal });
  window.murmurApplicationMenu = { toggle, close };
  window.addEventListener('beforeunload', () => { close(); lifetime.abort(); }, { once: true });
})();
