// Local Help: browse the bundled article index by default, search it via the
// `help_search` command, and read a full article in place. Browsing uses the
// static `help_articles`/`help_article` commands, so it works even while the
// search engine is still preparing. Retrieval runs entirely on-device;
// nothing typed here leaves the machine. Loaded after dom.js, which owns the
// shared `invoke`/`listen` bridge.

(function () {
  const input = document.getElementById('help-search');
  const indexNav = document.getElementById('help-index');
  const resultsList = document.getElementById('help-results');
  const emptyState = document.getElementById('help-empty');
  const preparing = document.getElementById('help-preparing');
  const articleView = document.getElementById('help-article');
  const articleTitle = document.getElementById('help-article-title');
  const articleBody = document.getElementById('help-article-body');
  const backButton = document.getElementById('help-back');
  if (
    !input || !indexNav || !resultsList || !emptyState || !preparing ||
    !articleView || !articleTitle || !articleBody || !backButton
  ) return;

  const DEBOUNCE_MS = 200;
  let debounceHandle = null;
  // Monotonic id so a slow search can't overwrite a newer one's results.
  let queryToken = 0;

  function setReady(ready) {
    preparing.hidden = ready;
  }

  // Poll once at startup in case the engine was ready before this view loaded;
  // the `help-ready` event covers the still-preparing case.
  invoke('help_ready')
    .then((ready) => setReady(!!ready))
    .catch(() => setReady(false));

  const unlistenReady = listen('help-ready', () => setReady(true));

  // ── View states: exactly one of index / results / empty / article shows ──

  function show(view) {
    indexNav.hidden = view !== 'index';
    resultsList.hidden = view !== 'results';
    emptyState.hidden = view !== 'empty';
    articleView.hidden = view !== 'article';
  }

  function showIndex() {
    resultsList.replaceChildren();
    articleBody.replaceChildren();
    show('index');
  }

  // ── Article index ──

  function renderIndexEntry(summary) {
    const button = document.createElement('button');
    button.type = 'button';
    button.className = 'help-index__item';
    button.dataset.title = summary.title;

    const title = document.createElement('span');
    title.className = 'help-index__title';
    title.textContent = summary.title;

    const headings = document.createElement('span');
    headings.className = 'help-index__headings';
    headings.textContent = (summary.headings || []).join(' · ');

    button.append(title, headings);
    return button;
  }

  invoke('help_articles')
    .then((summaries) => {
      if (!Array.isArray(summaries)) return;
      const frag = document.createDocumentFragment();
      for (const summary of summaries) frag.appendChild(renderIndexEntry(summary));
      indexNav.replaceChildren(frag);
    })
    .catch((err) => console.error('Help article list failed:', err));

  // ── Minimal markdown renderer ──
  // Covers exactly what the bundled articles use: #/##/### headings,
  // paragraphs, "- " lists, ``` fences, and inline `code` / **bold**. Builds
  // DOM nodes with textContent only, so article text is never parsed as HTML.

  function appendInline(target, text) {
    const parts = text.split(/(\*\*[^*]+\*\*|`[^`]+`)/);
    for (const part of parts) {
      if (!part) continue;
      if (part.startsWith('**') && part.endsWith('**') && part.length > 4) {
        const strong = document.createElement('strong');
        strong.textContent = part.slice(2, -2);
        target.appendChild(strong);
      } else if (part.startsWith('`') && part.endsWith('`') && part.length > 2) {
        const code = document.createElement('code');
        code.textContent = part.slice(1, -1);
        target.appendChild(code);
      } else {
        target.appendChild(document.createTextNode(part));
      }
    }
  }

  function renderMarkdown(markdown) {
    const frag = document.createDocumentFragment();
    let paragraph = [];
    let list = null;
    let codeLines = null;

    function flushParagraph() {
      if (!paragraph.length) return;
      const p = document.createElement('p');
      appendInline(p, paragraph.join(' '));
      frag.appendChild(p);
      paragraph = [];
    }
    function flushList() {
      if (list) frag.appendChild(list);
      list = null;
    }

    for (const line of markdown.split('\n')) {
      if (codeLines !== null) {
        if (line.trim().startsWith('```')) {
          const pre = document.createElement('pre');
          const code = document.createElement('code');
          code.textContent = codeLines.join('\n');
          pre.appendChild(code);
          frag.appendChild(pre);
          codeLines = null;
        } else {
          codeLines.push(line);
        }
        continue;
      }
      const trimmed = line.trim();
      if (trimmed.startsWith('```')) {
        flushParagraph();
        flushList();
        codeLines = [];
        continue;
      }
      const headingMatch = /^(#{1,6})\s+(.*)$/.exec(trimmed);
      if (headingMatch) {
        flushParagraph();
        flushList();
        // The page's view header is an h2; article headings nest below it.
        const level = Math.min(headingMatch[1].length + 2, 6);
        const heading = document.createElement('h' + level);
        appendInline(heading, headingMatch[2]);
        frag.appendChild(heading);
        continue;
      }
      if (trimmed.startsWith('- ')) {
        flushParagraph();
        if (!list) list = document.createElement('ul');
        const li = document.createElement('li');
        appendInline(li, trimmed.slice(2));
        list.appendChild(li);
        continue;
      }
      if (!trimmed) {
        flushParagraph();
        flushList();
        continue;
      }
      flushList();
      paragraph.push(trimmed);
    }
    flushParagraph();
    flushList();
    if (codeLines !== null) {
      // Unclosed fence: render what we have rather than dropping it.
      const pre = document.createElement('pre');
      const code = document.createElement('code');
      code.textContent = codeLines.join('\n');
      pre.appendChild(code);
      frag.appendChild(pre);
    }
    return frag;
  }

  // ── Reading view ──

  async function openArticle(title) {
    try {
      const article = await invoke('help_article', { title });
      articleTitle.textContent = article.title;
      articleBody.replaceChildren(renderMarkdown(article.markdown || ''));
      show('article');
      backButton.focus();
    } catch (err) {
      console.error('Help article load failed:', err);
    }
  }

  // ── Search ──

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

    const open = document.createElement('button');
    open.type = 'button';
    open.className = 'help-card__open';
    open.dataset.title = hit.article;
    open.textContent = 'Read the full article';

    li.append(article, heading, body, score, open);
    return li;
  }

  function renderResults(hits) {
    resultsList.replaceChildren();
    if (!hits.length) {
      show('empty');
      return;
    }
    const frag = document.createDocumentFragment();
    for (const hit of hits) frag.appendChild(renderHit(hit));
    resultsList.appendChild(frag);
    show('results');
  }

  async function runSearch(query) {
    const trimmed = query.trim();
    if (!trimmed) {
      showIndex();
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
      showIndex();
    }
  }

  // ── Event wiring (removed again in the teardown path below) ──

  function onInput() {
    if (debounceHandle !== null) clearTimeout(debounceHandle);
    debounceHandle = setTimeout(() => {
      debounceHandle = null;
      runSearch(input.value);
    }, DEBOUNCE_MS);
  }

  function onKeydown(event) {
    if (event.key !== 'Enter') return;
    event.preventDefault();
    if (debounceHandle !== null) {
      clearTimeout(debounceHandle);
      debounceHandle = null;
    }
    runSearch(input.value);
  }

  function onIndexClick(event) {
    const button = event.target.closest('.help-index__item');
    if (button) openArticle(button.dataset.title);
  }

  function onResultsClick(event) {
    const button = event.target.closest('.help-card__open');
    if (button) openArticle(button.dataset.title);
  }

  function onBack() {
    // Back returns to wherever the user came from: results if a query is
    // active, the index otherwise.
    if (input.value.trim()) {
      runSearch(input.value);
    } else {
      showIndex();
    }
    input.focus();
  }

  input.addEventListener('input', onInput);
  input.addEventListener('keydown', onKeydown);
  indexNav.addEventListener('click', onIndexClick);
  resultsList.addEventListener('click', onResultsClick);
  backButton.addEventListener('click', onBack);

  function teardown() {
    if (debounceHandle !== null) clearTimeout(debounceHandle);
    input.removeEventListener('input', onInput);
    input.removeEventListener('keydown', onKeydown);
    indexNav.removeEventListener('click', onIndexClick);
    resultsList.removeEventListener('click', onResultsClick);
    backButton.removeEventListener('click', onBack);
    unlistenReady.then((off) => off()).catch(() => {});
  }
  window.addEventListener('beforeunload', teardown);
})();
