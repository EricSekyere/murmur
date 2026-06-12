// Dashboard shell: sidebar navigation over the existing sections.
// Loaded after main.js. Reuses main.js's collapsible toggle handlers
// (which also lazy-load data) by clicking them programmatically — the
// trigger buttons themselves are hidden by CSS in the dashboard layout.

(function () {
  const navItems = Array.from(document.querySelectorAll('.nav__item'));
  const viewEls = {
    home: document.getElementById('view-home'),
    analytics: document.getElementById('view-analytics'),
    settings: document.getElementById('view-settings'),
    diagnostics: document.getElementById('view-diagnostics'),
  };
  const viewToggles = {
    home: document.getElementById('history-toggle'),
    analytics: document.getElementById('analytics-toggle'),
    settings: document.getElementById('settings-toggle'),
    diagnostics: document.getElementById('diagnostics-toggle'),
  };

  /** Expand a section panel via its (hidden) collapsible trigger so the
   *  existing handler runs its data refresh. If already expanded, cycle it
   *  so revisiting a view always shows fresh data. */
  function ensureExpanded(toggle, refresh) {
    if (!toggle) return;
    const expanded = toggle.getAttribute('aria-expanded') === 'true';
    if (!expanded) {
      toggle.click();
    } else if (refresh) {
      toggle.click();
      toggle.click();
    }
  }

  function activateView(name) {
    for (const item of navItems) {
      item.classList.toggle('nav__item--active', item.dataset.view === name);
      item.setAttribute('aria-current', item.dataset.view === name ? 'page' : 'false');
    }
    for (const [key, el] of Object.entries(viewEls)) {
      if (el) el.classList.toggle('view--active', key === name);
    }
    // Refresh settings/analytics on every visit; history stays live.
    ensureExpanded(viewToggles[name], name !== 'home');
  }

  for (const item of navItems) {
    item.addEventListener('click', () => activateView(item.dataset.view));
  }

  // Initial state: home view with history expanded.
  activateView('home');
})();
