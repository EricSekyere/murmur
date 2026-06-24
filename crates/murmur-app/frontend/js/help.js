// Local Help search: query the bundled help corpus via the `help_search`
// command and render the best-matching sections. Retrieval runs entirely
// on-device; nothing typed here leaves the machine. Loaded after dom.js, which
// owns the shared `invoke`/`listen` bridge.

(function () {
  const input = document.getElementById('help-search');
  const resultsList = document.getElementById('help-results');
  const emptyState = document.getElementById('help-empty');
  const preparing = document.getElementById('help-preparing');
  if (!input || !resultsList || !emptyState || !preparing) return;

  const DEBOUNCE_MS = 200;
  let debounceHandle = null;
  // Monotonic id so a slow search can't overwrite a newer one's results.
  let queryToken = 0;
  let helpReady = false;

  function setReady(ready) {
    helpReady = ready;
    preparing.hidden = ready;
  }

  // Poll once at startup in case the engine was ready before this view loaded;
  // the `help-ready` event covers the still-preparing case.
  invoke('help_ready')
    .then((ready) => setReady(!!ready))
    .catch(() => setReady(false));

  const unlistenReady = listen('help-ready', () => setReady(true));

  function showEmptyState() {
    resultsList.replaceChildren();
    emptyState.hidden = false;
  }

  /** Build one result card from a hit, using textContent throughout so query
   *  and corpus text are never interpreted as markup. */
  function renderHit(hit) {
    const li = document.createElement('li');
    li.className = 'help-card';

    const article = document.createElement('span');
    article.className = 'help-card__article';
    article.textContent = hit.article;

    const heading = document.createElement('h3');
    heading.className = 'help-card__heading';
    heading.textContent = hit.heading;

    const body = document.createElement('p');
    body.className = 'help-card__body';
    body.textContent = hit.body;

    const score = document.createElement('span');
    score.className = 'help-card__score';
    score.textContent = `${Math.round((hit.score || 0) * 100)}% match`;

    li.append(article, heading, body, score);
    return li;
  }

  function renderResults(hits) {
    resultsList.replaceChildren();
    if (!hits.length) {
      emptyState.hidden = false;
      return;
    }
    emptyState.hidden = true;
    const frag = document.createDocumentFragment();
    for (const hit of hits) frag.appendChild(renderHit(hit));
    resultsList.appendChild(frag);
  }

  async function runSearch(query) {
    const trimmed = query.trim();
    if (!trimmed) {
      showEmptyState();
      return;
    }
    const token = ++queryToken;
    try {
      const hits = await invoke('help_search', { query: trimmed });
      if (token !== queryToken) return; // a newer query superseded this one
      renderResults(Array.isArray(hits) ? hits : []);
    } catch (err) {
      if (token !== queryToken) return;
      console.error('Help search failed:', err);
      showEmptyState();
    }
  }

  input.addEventListener('input', () => {
    if (debounceHandle !== null) clearTimeout(debounceHandle);
    debounceHandle = setTimeout(() => {
      debounceHandle = null;
      runSearch(input.value);
    }, DEBOUNCE_MS);
  });

  input.addEventListener('keydown', (event) => {
    if (event.key !== 'Enter') return;
    event.preventDefault();
    if (debounceHandle !== null) {
      clearTimeout(debounceHandle);
      debounceHandle = null;
    }
    runSearch(input.value);
  });

  // Clean up the event listener if the window is torn down.
  window.addEventListener('beforeunload', () => {
    if (debounceHandle !== null) clearTimeout(debounceHandle);
    unlistenReady.then((off) => off()).catch(() => {});
  });
})();
