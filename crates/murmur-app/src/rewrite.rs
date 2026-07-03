//! Command Mode selection rewrite: capture the selected
//! text in the target app via a clipboard round-trip, rewrite it with the
//! local LLM, and paste the result over the selection.
//!
//! The clipboard round-trip mirrors murmur-core's paste output: snapshot the
//! user's clipboard first and restore it before returning, on every path.
//! Selection and rewrite text are spoken-adjacent content and are never
//! logged above trace.

use std::time::Duration;

use anyhow::{Context, Result};
use murmur_core::llm::RewriteMode;
use tauri::State;

use crate::state::AppState;

/// Cadence and budget for the copied selection to land on the cleared
/// clipboard: apps write it asynchronously after the Ctrl+C keystroke.
const COPY_POLL_INTERVAL: Duration = Duration::from_millis(20);
const COPY_POLL_ATTEMPTS: u32 = 25;

/// Result of a rewrite request, mirrored to the frontend.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum RewriteOutcomeDto {
    /// The selection was rewritten and pasted over.
    Rewritten { chars: usize },
    /// The copy produced nothing: no text is selected in the target app.
    NoSelection,
    /// The rewrite backend cannot run (built without `llm`, or no model).
    Unavailable { reason: String },
}

/// Clipboard operations selection capture needs, mockable so tests never
/// touch the real OS clipboard.
pub(crate) trait ClipboardPort {
    /// Opaque snapshot of the current contents (text or image), restored
    /// after the capture so the user's clipboard survives.
    type Snapshot;

    fn snapshot(&mut self) -> Self::Snapshot;
    fn restore(&mut self, snapshot: &Self::Snapshot);
    /// Current clipboard text; `None` when empty or non-text.
    fn text(&mut self) -> Option<String>;
    fn clear(&mut self);
}

/// Capture the current selection: snapshot the clipboard, clear it, send the
/// platform copy chord, poll for the copied text, and restore the snapshot.
/// Clearing first is what makes "no selection" detectable: after a copy that
/// produced nothing the clipboard is still empty, whereas comparing before
/// and after would misread a selection identical to the old clipboard.
/// Returns `None` when nothing (or only whitespace) was copied.
fn capture_selection<C: ClipboardPort>(
    clipboard: &mut C,
    send_copy: impl FnOnce(&mut C) -> Result<()>,
    poll_interval: Duration,
    poll_attempts: u32,
) -> Result<Option<String>> {
    let saved = clipboard.snapshot();
    clipboard.clear();
    let copy_sent = send_copy(clipboard);
    let copied = match &copy_sent {
        Ok(()) => poll_copied_text(clipboard, poll_interval, poll_attempts),
        Err(_) => None,
    };
    // Restore before surfacing any error so the user's clipboard survives
    // even a failed copy.
    clipboard.restore(&saved);
    copy_sent?;
    Ok(copied.filter(|text| !text.trim().is_empty()))
}

/// Poll the cleared clipboard until the copied text appears or the budget
/// runs out.
fn poll_copied_text<C: ClipboardPort>(
    clipboard: &mut C,
    interval: Duration,
    attempts: u32,
) -> Option<String> {
    for _ in 0..attempts {
        if let Some(text) = clipboard.text() {
            return Some(text);
        }
        std::thread::sleep(interval);
    }
    None
}

/// Real clipboard backed by arboard, preserving text or image contents (the
/// same pattern as murmur-core's paste output).
struct SystemClipboard {
    inner: arboard::Clipboard,
}

enum SystemSnapshot {
    Text(String),
    Image(arboard::ImageData<'static>),
    Empty,
}

impl SystemClipboard {
    fn new() -> Result<Self> {
        Ok(Self {
            inner: arboard::Clipboard::new().context("opening the clipboard")?,
        })
    }
}

impl ClipboardPort for SystemClipboard {
    type Snapshot = SystemSnapshot;

    fn snapshot(&mut self) -> SystemSnapshot {
        if let Ok(text) = self.inner.get_text() {
            SystemSnapshot::Text(text)
        } else if let Ok(image) = self.inner.get_image() {
            SystemSnapshot::Image(image)
        } else {
            SystemSnapshot::Empty
        }
    }

    fn restore(&mut self, snapshot: &SystemSnapshot) {
        // Best effort, mirroring paste.rs: an unrestorable clipboard must
        // not fail the rewrite.
        let _ = match snapshot {
            SystemSnapshot::Text(text) => self.inner.set_text(text),
            SystemSnapshot::Image(image) => self.inner.set_image(image.clone()),
            SystemSnapshot::Empty => self.inner.clear(),
        };
    }

    fn text(&mut self) -> Option<String> {
        self.inner.get_text().ok().filter(|text| !text.is_empty())
    }

    fn clear(&mut self) {
        // A failed clear only weakens no-selection detection; the poll then
        // sees the old text and the capture may report it, never crash.
        let _ = self.inner.clear();
    }
}

/// What the model layer produced for a captured selection.
// In a build without `llm` the Rewritten arm is matched but never
// constructed; the variant must still exist so the delivery path compiles
// identically in both builds.
#[cfg_attr(not(feature = "llm"), allow(dead_code))]
enum RewriteRun {
    Rewritten(String),
    Unavailable(String),
}

/// Output-token budget for a rewrite: roughly the input's token count
/// (about 4 chars/token) with headroom for formats that expand (bullet
/// lists), bounded to keep a runaway generation inside the 4k context.
#[cfg(any(test, feature = "llm"))]
fn rewrite_token_budget(text: &str) -> usize {
    (text.chars().count() / 2 + 64).clamp(192, 1536)
}

/// Rewrite `text` with the lazily loaded local LLM. The engine loads on
/// first use (the model holds about 1 GB resident) and is cached in app
/// state for the app's lifetime.
#[cfg(feature = "llm")]
fn run_rewrite(
    engine_slot: &std::sync::Mutex<Option<murmur_core::llm::LlmEngine>>,
    text: &str,
    mode: RewriteMode,
) -> Result<RewriteRun> {
    use murmur_core::llm;

    if !llm::is_downloaded() {
        let expected = llm::model_path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "the murmur data directory".to_string());
        return Ok(RewriteRun::Unavailable(format!(
            "The local rewrite model is not downloaded yet (expected at {expected})."
        )));
    }

    let mut slot = engine_slot.lock().unwrap_or_else(|e| e.into_inner());
    if slot.is_none() {
        let path = llm::model_path().context("resolving the rewrite model path")?;
        *slot = Some(llm::LlmEngine::load(&path).context("loading the rewrite model")?);
    }
    let engine = slot
        .as_ref()
        .context("rewrite engine unavailable after load")?;

    let rewritten = llm::rewrite(engine, text, mode, rewrite_token_budget(text))
        .context("rewriting the selection")?;
    Ok(RewriteRun::Rewritten(rewritten))
}

#[cfg(not(feature = "llm"))]
fn run_rewrite(_text: &str, _mode: RewriteMode) -> Result<RewriteRun> {
    Ok(RewriteRun::Unavailable(
        "The local LLM is not available in this build of Murmur (built without the llm feature)."
            .to_string(),
    ))
}

/// The blocking half of [`rewrite_selection`]: focus the target window,
/// capture the selection, rewrite, and paste the result over it. Runs on a
/// blocking thread because every step (keystrokes, clipboard polls, model
/// inference) blocks.
fn rewrite_selection_blocking(
    mode: RewriteMode,
    #[cfg(windows)] fallback_hwnd: usize,
    #[cfg(feature = "llm")] engine_slot: &std::sync::Mutex<Option<murmur_core::llm::LlmEngine>>,
) -> Result<RewriteOutcomeDto> {
    // The request comes from Murmur's own UI, so the selection lives in the
    // window the user was in just before: the live-tracked external target
    // (there is no dictation-session start window to prefer here).
    #[cfg(windows)]
    if !crate::focus::ensure_external_target(0, fallback_hwnd) {
        anyhow::bail!(
            "no target window found; click into the app that has your selection, then try again"
        );
    }
    // Remember exactly which window the selection was copied from, so the
    // paste after a slow inference can refuse if focus has moved on.
    #[cfg(windows)]
    let target_hwnd = crate::focus::foreground_window();

    let selected = {
        let mut clipboard = SystemClipboard::new()?;
        capture_selection(
            &mut clipboard,
            |_| {
                // A hotkey-held modifier would corrupt the chord, same as the
                // paste path.
                #[cfg(windows)]
                murmur_core::output::keyboard::release_all_modifiers_public();
                murmur_core::output::keyboard::copy()
            },
            COPY_POLL_INTERVAL,
            COPY_POLL_ATTEMPTS,
        )?
    };
    let Some(selected) = selected else {
        tracing::info!("rewrite: no selection captured");
        return Ok(RewriteOutcomeDto::NoSelection);
    };
    tracing::debug!(
        chars = selected.chars().count(),
        ?mode,
        "rewriting selection"
    );

    #[cfg(feature = "llm")]
    let run = run_rewrite(engine_slot, &selected, mode)?;
    #[cfg(not(feature = "llm"))]
    let run = run_rewrite(&selected, mode)?;

    match run {
        RewriteRun::Unavailable(reason) => {
            tracing::info!(%reason, "rewrite unavailable");
            Ok(RewriteOutcomeDto::Unavailable { reason })
        }
        RewriteRun::Rewritten(rewritten) => {
            let rewritten = rewritten.trim();
            if rewritten.is_empty() {
                anyhow::bail!("the model produced no text; the selection was left unchanged");
            }
            #[cfg(windows)]
            if !crate::focus::ensure_external_target(target_hwnd, 0) {
                anyhow::bail!("the target window went away during the rewrite; nothing was pasted");
            }
            // The selection is still active in the target, so the paste
            // replaces it. paste_text snapshots and restores the user's
            // clipboard around its own copy, same as dictation delivery.
            murmur_core::output::paste::ClipboardPasteOutput::new()
                .paste_text(rewritten)
                .context("pasting the rewritten text over the selection")?;
            Ok(RewriteOutcomeDto::Rewritten {
                chars: rewritten.chars().count(),
            })
        }
    }
}

/// Rewrite the text currently selected in the target application with the
/// local LLM and paste the result over it. The whole flow runs off the
/// async reactor.
#[tauri::command]
pub(crate) async fn rewrite_selection(
    state: State<'_, AppState>,
    mode: RewriteMode,
) -> Result<RewriteOutcomeDto, String> {
    // Dictation delivery and the rewrite both drive the clipboard and
    // keyboard; running them concurrently would interleave keystrokes.
    if *state.recording.lock().unwrap_or_else(|e| e.into_inner()) {
        return Err("stop dictation before rewriting a selection".to_string());
    }
    #[cfg(windows)]
    let fallback_hwnd = *state
        .last_external_foreground
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    #[cfg(feature = "llm")]
    let engine_slot = std::sync::Arc::clone(&state.llm);

    tauri::async_runtime::spawn_blocking(move || {
        rewrite_selection_blocking(
            mode,
            #[cfg(windows)]
            fallback_hwnd,
            #[cfg(feature = "llm")]
            &engine_slot,
        )
    })
    .await
    .map_err(|e| format!("rewrite task failed: {e}"))?
    .map_err(|e| format!("{e:#}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// In-memory clipboard that records every operation in order, plus an
    /// optional delayed arrival (`incoming`) to exercise the poll loop.
    #[derive(Default)]
    struct MockClipboard {
        content: Option<String>,
        /// Text that lands only after N reads, simulating an app that
        /// writes the clipboard asynchronously after Ctrl+C.
        incoming: Option<(u32, String)>,
        ops: Vec<&'static str>,
    }

    impl MockClipboard {
        fn holding(text: &str) -> Self {
            Self {
                content: Some(text.to_string()),
                ..Self::default()
            }
        }
    }

    impl ClipboardPort for MockClipboard {
        type Snapshot = Option<String>;

        fn snapshot(&mut self) -> Option<String> {
            self.ops.push("snapshot");
            self.content.clone()
        }

        fn restore(&mut self, snapshot: &Option<String>) {
            self.ops.push("restore");
            self.content = snapshot.clone();
        }

        fn text(&mut self) -> Option<String> {
            self.ops.push("read");
            if let Some((reads_left, text)) = self.incoming.take() {
                if reads_left == 0 {
                    self.content = Some(text);
                } else {
                    self.incoming = Some((reads_left - 1, text));
                }
            }
            self.content.clone().filter(|t| !t.is_empty())
        }

        fn clear(&mut self) {
            self.ops.push("clear");
            self.content = None;
        }
    }

    fn capture(
        clipboard: &mut MockClipboard,
        send_copy: impl FnOnce(&mut MockClipboard) -> Result<()>,
    ) -> Result<Option<String>> {
        capture_selection(clipboard, send_copy, Duration::ZERO, 3)
    }

    #[test]
    fn copied_text_is_the_selection_and_the_clipboard_is_restored() {
        let mut clipboard = MockClipboard::holding("previous contents");
        let selected = capture(&mut clipboard, |c| {
            c.ops.push("copy");
            c.content = Some("selected words".to_string());
            Ok(())
        })
        .expect("capture");

        assert_eq!(selected.as_deref(), Some("selected words"));
        assert_eq!(clipboard.content.as_deref(), Some("previous contents"));
        // Ordering: snapshot before the clear that enables detection, the
        // copy after the clear, and the restore strictly last.
        assert_eq!(&clipboard.ops[..3], &["snapshot", "clear", "copy"]);
        assert_eq!(clipboard.ops.last(), Some(&"restore"));
    }

    #[test]
    fn empty_clipboard_after_copy_means_no_selection() {
        let mut clipboard = MockClipboard::holding("previous contents");
        let selected = capture(&mut clipboard, |c| {
            // The app had no selection, so Ctrl+C wrote nothing.
            c.ops.push("copy");
            Ok(())
        })
        .expect("capture");

        assert_eq!(selected, None);
        assert_eq!(clipboard.content.as_deref(), Some("previous contents"));
        assert_eq!(clipboard.ops.last(), Some(&"restore"));
    }

    #[test]
    fn whitespace_only_copy_is_no_selection() {
        let mut clipboard = MockClipboard::holding("previous contents");
        let selected = capture(&mut clipboard, |c| {
            c.content = Some("  \n\t ".to_string());
            Ok(())
        })
        .expect("capture");

        assert_eq!(selected, None);
        assert_eq!(clipboard.content.as_deref(), Some("previous contents"));
    }

    #[test]
    fn slow_copy_is_caught_by_polling() {
        let mut clipboard = MockClipboard::holding("previous contents");
        let selected = capture(&mut clipboard, |c| {
            c.incoming = Some((2, "late arrival".to_string()));
            Ok(())
        })
        .expect("capture");

        assert_eq!(selected.as_deref(), Some("late arrival"));
        assert_eq!(clipboard.content.as_deref(), Some("previous contents"));
    }

    #[test]
    fn clipboard_is_restored_even_when_the_copy_chord_fails() {
        let mut clipboard = MockClipboard::holding("previous contents");
        let result = capture(&mut clipboard, |_| anyhow::bail!("SendInput failed"));

        assert!(result.is_err());
        assert_eq!(clipboard.content.as_deref(), Some("previous contents"));
        assert_eq!(clipboard.ops.last(), Some(&"restore"));
    }

    #[test]
    fn token_budget_scales_with_input_within_bounds() {
        assert_eq!(rewrite_token_budget(""), 192);
        assert_eq!(rewrite_token_budget(&"x".repeat(40)), 192);
        assert_eq!(rewrite_token_budget(&"x".repeat(1000)), 564);
        assert_eq!(rewrite_token_budget(&"x".repeat(100_000)), 1536);
    }

    #[test]
    fn dto_serializes_with_kind_tags() {
        let rewritten =
            serde_json::to_value(RewriteOutcomeDto::Rewritten { chars: 42 }).expect("serialize");
        assert_eq!(rewritten["kind"], "rewritten");
        assert_eq!(rewritten["chars"], 42);

        let none = serde_json::to_value(RewriteOutcomeDto::NoSelection).expect("serialize");
        assert_eq!(none["kind"], "no_selection");

        let unavailable = serde_json::to_value(RewriteOutcomeDto::Unavailable {
            reason: "no model".to_string(),
        })
        .expect("serialize");
        assert_eq!(unavailable["kind"], "unavailable");
        assert_eq!(unavailable["reason"], "no model");
    }
}
