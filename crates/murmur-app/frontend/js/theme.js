// Apply the saved appearance before paint; it is independent of speech settings.
(() => {
  const storageKey = 'murmur_theme';
  let theme = 'dark';
  try {
    if (localStorage.getItem(storageKey) === 'light') theme = 'light';
  } catch {
    // Appearance still works when webview storage is unavailable.
  }
  document.documentElement.dataset.theme = theme;

  function bindControls() {
    const select = document.getElementById('theme-select');
    const toggle = document.getElementById('theme-toggle');
    const label = document.getElementById('theme-toggle-label');
    const sun = document.getElementById('theme-icon-sun');
    const moon = document.getElementById('theme-icon-moon');

    function render() {
      document.documentElement.dataset.theme = theme;
      if (select) select.value = theme;
      const next = theme === 'dark' ? 'light' : 'dark';
      if (label) label.textContent = next === 'light' ? 'Light mode' : 'Dark mode';
      if (toggle) toggle.setAttribute('aria-label', `Switch to ${next} mode`);
      if (sun) sun.toggleAttribute('hidden', next !== 'light');
      if (moon) moon.toggleAttribute('hidden', next !== 'dark');
    }

    function setTheme(value) {
      theme = value === 'light' ? 'light' : 'dark';
      render();
      try {
        localStorage.setItem(storageKey, theme);
      } catch {
        // Keep the selected appearance for this window if persistence fails.
      }
    }

    const onSelect = () => setTheme(select.value);
    const onToggle = () => setTheme(theme === 'dark' ? 'light' : 'dark');
    const onStorage = (event) => {
      if (event.key !== storageKey && event.key !== null) return;
      theme = event.newValue === 'light' ? 'light' : 'dark';
      render();
    };
    if (select) select.addEventListener('change', onSelect);
    if (toggle) toggle.addEventListener('click', onToggle);
    window.addEventListener('storage', onStorage);
    render();

    window.addEventListener('beforeunload', () => {
      if (select) select.removeEventListener('change', onSelect);
      if (toggle) toggle.removeEventListener('click', onToggle);
      window.removeEventListener('storage', onStorage);
    }, { once: true });
  }
  document.addEventListener('DOMContentLoaded', bindControls, { once: true });
})();
