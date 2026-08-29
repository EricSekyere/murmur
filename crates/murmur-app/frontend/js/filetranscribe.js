// Transcribing a file the user picks, with progress while it runs.
//
// Wrapped in an IIFE: the frontend loads plain scripts into one shared scope.
(() => {
  const { invoke: ftInvoke } = window.__TAURI__.core;
  const { listen: ftListen } = window.__TAURI__.event;

  const el = (id) => document.getElementById(id);

  let running = false;
  let unlistenProgress = null;

  function setStatus(text) {
    const status = el("file-transcribe-status");
    if (status) status.textContent = text;
  }

  function showProgress(done, total) {
    const bar = el("file-transcribe-progress");
    if (!bar) return;
    bar.hidden = false;
    bar.max = total;
    bar.value = done;
  }

  function resetView() {
    const bar = el("file-transcribe-progress");
    const out = el("file-transcribe-output");
    const actions = el("file-transcribe-actions");
    if (bar) {
      bar.hidden = true;
      bar.value = 0;
    }
    if (out) {
      out.hidden = true;
      out.value = "";
    }
    if (actions) actions.hidden = true;
    setStatus("");
  }

  async function pickAndTranscribe() {
    // A second pick mid-run would interleave two transcripts into one box.
    if (running) return;
    running = true;
    const pick = el("file-transcribe-pick");
    if (pick) pick.disabled = true;
    resetView();
    setStatus("Choosing a file...");

    try {
      const text = await ftInvoke("pick_and_transcribe_file");
      if (text === null || text === undefined) {
        setStatus("");
        return;
      }
      const out = el("file-transcribe-output");
      const actions = el("file-transcribe-actions");
      if (out) {
        out.hidden = false;
        out.value = text;
      }
      if (actions) actions.hidden = false;
      setStatus(`Done, ${text.split(/\s+/).filter(Boolean).length} words.`);
    } catch (err) {
      setStatus(`Could not transcribe that file: ${err}`);
    } finally {
      running = false;
      if (pick) pick.disabled = false;
      const bar = el("file-transcribe-progress");
      if (bar) bar.hidden = true;
    }
  }

  async function copyTranscript() {
    const out = el("file-transcribe-output");
    if (!out || !out.value) return;
    try {
      await navigator.clipboard.writeText(out.value);
      setStatus("Copied.");
    } catch {
      // Leave it selected so it can still be copied by hand.
      out.select();
      setStatus("Press Ctrl+C to copy.");
    }
  }

  function openPanel() {
    const toggle = el("file-transcribe-toggle");
    const panel = el("file-transcribe-panel");
    if (!toggle || !panel) return;
    panel.hidden = false;
    toggle.setAttribute("aria-expanded", "true");
  }

  async function initFileTranscribe() {
    const pick = el("file-transcribe-pick");
    const copy = el("file-transcribe-copy");
    const toggle = el("file-transcribe-toggle");
    const panel = el("file-transcribe-panel");

    if (toggle && panel) {
      toggle.addEventListener("click", () => {
        const open = panel.hidden;
        panel.hidden = !open;
        toggle.setAttribute("aria-expanded", String(open));
      });
    }
    if (pick) pick.addEventListener("click", pickAndTranscribe);
    if (copy) copy.addEventListener("click", copyTranscript);

    unlistenProgress = await ftListen("file-transcribe-progress", (event) => {
      const { done, total, text } = event.payload ?? {};
      if (typeof done !== "number" || typeof total !== "number") return;
      showProgress(done, total);
      setStatus(`Transcribing, part ${done} of ${total}...`);
      const out = el("file-transcribe-output");
      if (out && typeof text === "string") {
        out.hidden = false;
        out.value = text;
        out.scrollTop = out.scrollHeight;
      }
    });

    // Opened first so the result is visible when it arrives.
    await ftListen("menu-transcribe-file", () => {
      openPanel();
      pickAndTranscribe();
    });
  }

  // Unlisten on unload so a reload does not stack listeners.
  window.addEventListener("beforeunload", () => {
    if (unlistenProgress) {
      unlistenProgress();
      unlistenProgress = null;
    }
  });

  document.addEventListener("DOMContentLoaded", () => {
    initFileTranscribe();
  });
})();
