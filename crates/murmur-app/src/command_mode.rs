//! Command-mode wiring for the desktop app: the mode toggle behind its own
//! global hotkey, the transcript-to-action Tauri commands, and the
//! pending-action store behind the physical-confirm gate.
//!
//! Safety spine, restated: [`run_command`] can produce a `Pending` action
//! but never runs it. Only [`confirm_pending`], invoked by a real click or
//! keypress in the confirm dialog, executes it. There is deliberately no
//! voice path to confirmation (design Section 5).

use std::sync::atomic::Ordering;

use anyhow::Context;
use murmur_core::command::{Grammar, Match, PermissionStore, RouteOutcome, SlotValue, Tool};
use murmur_core::indexer::{FileMatch, Resolution};
use murmur_core::output::OutputMode;
use murmur_mcp::ActionBackend;
use serde_json::Value;
use tauri::{Emitter, Manager, State};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};

use crate::command_exec::{
    CMD_GO_TO_DIR, CMD_OPEN_FILE, ExecOutcome, Executor, PendingAction, ToolBackend,
    confirm_and_execute, starter_grammar,
};
use crate::native_actions::{NativeActions, SystemActions};
use crate::state::AppState;

/// Default command-mode hotkey: a separate activation channel from the
/// dictation hotkey (design Section 5), configurable later.
pub(crate) const COMMAND_MODE_HOTKEY: &str = "ctrl+shift+period";

/// A single item awaiting a physical click, bound to a nonce.
///
/// ASR stays live while a dialog is open, so a newer utterance can supersede
/// the stored item before the dialog re-renders. The nonce binds a click to
/// the exact item the dialog displayed: a stale click is refused instead of
/// acting on something the user never reviewed.
///
/// Shared by the confirm gate ([`PendingAction`]) and the disambiguation
/// picker ([`PendingChoice`]); the discipline is identical, only the payload
/// differs. Each gate keeps its own nonce counter, and the two commands never
/// read each other's slot, so nonces cannot cross over.
struct Gate<T> {
    last_nonce: u64,
    slot: Option<(u64, T)>,
}

// Derived Default would demand `T: Default`, which neither payload has.
impl<T> Default for Gate<T> {
    fn default() -> Self {
        Self {
            last_nonce: 0,
            slot: None,
        }
    }
}

impl<T> Gate<T> {
    /// Stash a new item, superseding any previous one, and return the nonce
    /// the dialog must echo back.
    fn stash(&mut self, item: T) -> u64 {
        self.last_nonce += 1;
        self.slot = Some((self.last_nonce, item));
        self.last_nonce
    }

    /// Drop whatever is stored (any newer outcome supersedes it).
    fn clear(&mut self) {
        self.slot = None;
    }

    /// Take the item only if `nonce` matches the stored one; a stale nonce
    /// returns `None` and leaves the current item in place.
    fn take(&mut self, nonce: u64) -> Option<T> {
        match self.slot.take() {
            Some((stored, item)) if stored == nonce => Some(item),
            other => {
                self.slot = other;
                None
            }
        }
    }
}

type PendingGate = Gate<PendingAction>;

/// A near-tied spoken path resolution awaiting the user's physical pick.
///
/// The output mode and the target window are captured at decision time, while
/// the user's editor is still in front. Clicking the picker moves focus to
/// Murmur, so the stored HWND is the only record of where the path belongs.
struct PendingChoice {
    candidates: Vec<FileMatch>,
    output_mode: OutputMode,
    #[cfg(windows)]
    target_hwnd: usize,
}

/// How a chosen path actually reached the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ChoiceDelivery {
    /// Focus was restored to the dictation target and the path was typed.
    Typed,
    /// Focus could not be restored, so the path went to the clipboard rather
    /// than into whatever window happened to be in front.
    Copied,
}

/// Everything `run_command` needs, kept behind one async lock in app state
/// (execution awaits the tool backend while holding it).
pub(crate) struct CommandState {
    /// Kept so a connect task can filter servers without taking the lock.
    allowed: Vec<String>,
    grammar: Grammar,
    executor: Executor<SystemActions>,
    backend: ActionBackend,
    /// The single gated action awaiting physical confirmation, if any.
    pending: PendingGate,
    /// The near-tied path resolution awaiting a physical pick, if any.
    choice: Gate<PendingChoice>,
}

impl CommandState {
    /// The starter grammar over native actions, the saved permission policy,
    /// and an MCP backend trusting exactly the servers the user allowlisted.
    ///
    /// `allowed_servers` is normally `Settings::mcp_allowed_servers`, which
    /// defaults to empty. Empty is deny-all, so a config that never mentions
    /// MCP behaves as it did when the allowlist was hardcoded empty.
    ///
    /// The permission store is injected rather than loaded here so tests can
    /// supply their own; `PermissionStore::load` reads the real config dir.
    pub(crate) fn new(
        allowed_servers: Vec<String>,
        permissions: PermissionStore,
    ) -> anyhow::Result<Self> {
        if !allowed_servers.is_empty() {
            tracing::info!(
                servers = ?allowed_servers,
                "command mode: MCP servers allowlisted"
            );
        }
        Ok(Self {
            allowed: allowed_servers.clone(),
            grammar: starter_grammar().context("building the starter command grammar")?,
            executor: Executor::new(SystemActions, permissions),
            backend: ActionBackend::new(allowed_servers),
            pending: PendingGate::default(),
            choice: Gate::default(),
        })
    }
}

/// Serializable mirror of [`ExecOutcome`] for the frontend. `Pending`
/// carries the tool name and parsed arguments so the confirm dialog can
/// echo exactly what the ASR produced, plus the nonce the dialog must send
/// back with its confirm/cancel.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum ExecOutcomeDto {
    Executed {
        result: Value,
    },
    Pending {
        tool: String,
        args: Value,
        reversible: bool,
        nonce: u64,
    },
    /// A spoken path query landed on near-tied candidates; the user picks one
    /// in the disambiguation overlay. Relative paths only.
    Choose {
        candidates: Vec<String>,
        nonce: u64,
    },
    Blocked,
    NoAction,
}

/// Split an executor outcome into its DTO and the pending action to stash
/// (`None` for every non-pending outcome).
fn split_outcome(outcome: ExecOutcome) -> (ExecOutcomeDto, Option<PendingAction>) {
    match outcome {
        ExecOutcome::Executed(result) => (ExecOutcomeDto::Executed { result }, None),
        ExecOutcome::Pending(pending) => (
            ExecOutcomeDto::Pending {
                tool: pending.tool.clone(),
                args: pending.args.clone(),
                reversible: pending.reversible,
                // Placeholder: run_command patches in the real nonce once the
                // action is stashed in the gate.
                nonce: 0,
            },
            Some(pending),
        ),
        ExecOutcome::Blocked => (ExecOutcomeDto::Blocked, None),
        ExecOutcome::NoAction => (ExecOutcomeDto::NoAction, None),
    }
}

/// Tier 1 routing: the deterministic grammar decides, a miss is `NoMatch`.
/// Tier 3 (LLM tool selection) needs the `llm` feature plus a downloaded
/// model, and the app ships neither yet, so a miss stops here rather than
/// guessing (design Section 4).
fn route_transcript(grammar: &Grammar, transcript: &str) -> RouteOutcome {
    let Some(matched) = grammar.match_phrase(transcript) else {
        return RouteOutcome::NoMatch;
    };
    // A pattern registered for an MCP tool carries the namespaced tool name as
    // its id, which is what tells the two apart. Native commands never contain
    // the separator, so the check cannot collide with one.
    match matched.command_id.contains(TOOL_ID_SEPARATOR) {
        true => RouteOutcome::ToolCall {
            tool: matched.command_id,
            arguments: serde_json::Value::Object(Default::default()),
        },
        false => RouteOutcome::Command(matched),
    }
}

impl CommandState {
    /// Connect every allowlisted server that is actually configured, then
    /// register what they offer.
    ///
    /// Best effort throughout. A server that is allowlisted but absent from
    /// the user's MCP client configs, refuses to start, or fails its handshake
    /// is logged and skipped; command mode keeps working with native commands
    /// either way, which is why this never returns an error.
    /// The names this state is allowed to connect to.
    pub(crate) fn allowed_servers(&self) -> Vec<String> {
        self.allowed.clone()
    }

    /// Install an already-connected backend and register what it offers.
    ///
    /// Separate from connecting on purpose. Handshakes are bounded but slow,
    /// and this state is behind the lock every command hotkey press takes, so
    /// connecting while holding it would freeze command mode for as long as a
    /// slow server took to answer. Only this part needs the lock.
    pub(crate) fn install_backend(&mut self, backend: ActionBackend, tools: Vec<Tool>) {
        let spoken = register_tool_phrases(&mut self.grammar, &tools);
        let total = tools.len();
        self.backend = backend;
        self.executor.set_tools(tools);
        tracing::info!(
            tools = total,
            speakable = spoken,
            "MCP tools registered for command mode"
        );
    }
}

/// Separator between an MCP server and its tool, as `namespaced_tool_name`
/// builds it. Native command ids never contain one.
const TOOL_ID_SEPARATOR: char = '/';

/// The phrase that invokes a namespaced MCP tool.
///
/// `git/status` becomes "git status": separators, underscores and dashes read
/// aloud as spaces anyway. Returns None when nothing speakable is left.
///
/// The name comes from the server, and the phrase it produces is parsed as a
/// grammar pattern, so anything outside letters, digits and spaces is dropped
/// rather than passed through. A bare `{q}` would otherwise compile to a
/// free-text slot and turn one tool into a catch-all: "srv {q}" matches
/// "srv" followed by any words at all.
fn spoken_phrase_for_tool(tool_name: &str) -> Option<String> {
    let spoken: String = tool_name
        .chars()
        .map(|c| match c {
            c if c.is_ascii_alphanumeric() => c.to_ascii_lowercase(),
            _ => ' ',
        })
        .collect();
    let spoken = spoken.split_whitespace().collect::<Vec<_>>().join(" ");
    (!spoken.is_empty()).then_some(spoken)
}

/// Register a spoken phrase for each tool that takes no required arguments.
///
/// Only those, deliberately. A tool needing arguments cannot be driven from a
/// fixed phrase, and guessing them from free speech is exactly the kind of
/// thing that should not happen to a call the permission store may auto-run.
/// Those tools stay listed and callable by other means, just not by voice.
///
/// Registration order matters: `Grammar::match_phrase` takes the first hit, so
/// tools go in after the native starter commands and can never shadow one.
fn register_tool_phrases(grammar: &mut Grammar, tools: &[Tool]) -> usize {
    let mut added = 0;
    for tool in tools {
        if tool_requires_arguments(tool) {
            continue;
        }
        let Some(phrase) = spoken_phrase_for_tool(&tool.name) else {
            continue;
        };
        match grammar.add(tool.name.clone(), phrase.clone()) {
            Ok(()) => added += 1,
            Err(e) => {
                tracing::warn!(tool = %tool.name, %phrase, error = %e, "tool phrase rejected");
            }
        }
    }
    added
}

/// Whether a tool's JSON Schema declares any required argument.
fn tool_requires_arguments(tool: &Tool) -> bool {
    tool.input_schema
        .get("required")
        .and_then(|r| r.as_array())
        .is_some_and(|r| !r.is_empty())
}

/// Run a routed outcome through the guarded executor. Returns the DTO plus
/// any pending action for the caller to stash. Free function so tests can
/// drive it with mock actions and backends.
async fn execute_and_split<A: NativeActions, B: ToolBackend>(
    executor: &Executor<A>,
    backend: &B,
    outcome: RouteOutcome,
) -> anyhow::Result<(ExecOutcomeDto, Option<PendingAction>)> {
    let executed = executor.execute(outcome, backend).await?;
    Ok(split_outcome(executed))
}

/// Resolve a spoken "open the … file" query against the indexed project
/// paths and type the best match's relative path, or offer the near-tied
/// candidates. Handled here rather than in the executor because it needs
/// `AppState::project_files`. The query is spoken content and is never
/// logged; resolved paths only at trace.
fn run_open_file(
    state: &AppState,
    matched: &Match,
) -> anyhow::Result<(ExecOutcomeDto, Option<PendingChoice>)> {
    let (query, output_mode) = aliased_query(state, matched)?;
    let ranked = {
        let files = state
            .project_files
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        murmur_core::indexer::resolve_file(&query, &files)
    };
    act_on_resolution(murmur_core::indexer::decide(ranked), output_mode)
}

/// The directory analog of [`run_open_file`]: resolve "go to the … folder"
/// against the ancestor-directory set of the indexed files. Same privacy
/// rules, same near-tie handling.
fn run_go_to_dir(
    state: &AppState,
    matched: &Match,
) -> anyhow::Result<(ExecOutcomeDto, Option<PendingChoice>)> {
    let (query, output_mode) = aliased_query(state, matched)?;
    let dirs = {
        let files = state
            .project_files
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        murmur_core::indexer::directories(&files)
    };
    let ranked = murmur_core::indexer::resolve_file(&query, &dirs);
    act_on_resolution(murmur_core::indexer::decide(ranked), output_mode)
}

/// The `query` slot with the user's spoken path aliases applied, plus the
/// output mode. Both are read under one settings lock, dropped before the
/// caller takes the file-index lock, so no two locks are ever held at once.
fn aliased_query(state: &AppState, matched: &Match) -> anyhow::Result<(String, OutputMode)> {
    let Some(SlotValue::Text(query)) = matched.slots.get("query") else {
        anyhow::bail!(
            "command '{}' is missing text slot 'query'",
            matched.command_id
        );
    };
    let (aliases, output_mode) = {
        let settings = state.settings.lock().unwrap_or_else(|e| e.into_inner());
        (settings.path_aliases.clone(), settings.output_mode)
    };
    Ok((
        murmur_core::indexer::apply_aliases(query, &aliases),
        output_mode,
    ))
}

/// Act on a decided resolution: type a clear winner, or hand the near-tied
/// candidates back for the picker. Runs while the user's editor is still in
/// front, which is why the ambiguous branch captures the target window here.
fn act_on_resolution(
    resolution: Resolution,
    output_mode: OutputMode,
) -> anyhow::Result<(ExecOutcomeDto, Option<PendingChoice>)> {
    match resolution {
        Resolution::None => {
            tracing::debug!("no indexed path matched the spoken query");
            Ok((ExecOutcomeDto::NoAction, None))
        }
        Resolution::Decisive(best) => {
            tracing::trace!(path = %best.path, score = best.score, "spoken path query resolved");
            murmur_core::output::dispatch_verbatim(&best.path, output_mode)
                .context("typing the resolved path")?;
            Ok((
                ExecOutcomeDto::Executed {
                    result: Value::Null,
                },
                None,
            ))
        }
        Resolution::Ambiguous(candidates) => {
            tracing::debug!(count = candidates.len(), "spoken path query is a near tie");
            let paths = candidates.iter().map(|c| c.path.clone()).collect();
            let choice = PendingChoice {
                candidates,
                output_mode,
                #[cfg(windows)]
                target_hwnd: crate::focus::foreground_window(),
            };
            Ok((
                // run_command patches in the real nonce once this is stashed.
                ExecOutcomeDto::Choose {
                    candidates: paths,
                    nonce: 0,
                },
                Some(choice),
            ))
        }
    }
}

/// Deliver the picked path to the window dictation came from.
///
/// The pick itself put Murmur in front, so focus has to go back to the
/// captured target before any keystroke. When it cannot (the window closed,
/// nothing was captured, or the capture was one of our own windows), the path
/// goes to the clipboard rather than into whatever is now focused.
fn deliver_chosen_path(
    path: &str,
    output_mode: OutputMode,
    #[cfg(windows)] target_hwnd: usize,
) -> anyhow::Result<ChoiceDelivery> {
    // No live-tracked fallback: the picker always captures a start window, and
    // typing into a window the user never dictated into is the bug this whole
    // gate exists to prevent.
    #[cfg(windows)]
    let restored = crate::focus::ensure_external_target(target_hwnd, 0);
    // Restoring focus is Windows-only, so elsewhere there is no way to know the
    // keystrokes would land anywhere but Murmur's own window, which the click
    // just activated. Copy instead of typing into ourselves.
    #[cfg(not(windows))]
    let restored = false;

    let (mode, delivery) = choice_delivery(output_mode, restored);
    murmur_core::output::dispatch_verbatim(path, mode).with_context(|| match delivery {
        ChoiceDelivery::Copied => "copying the chosen path to the clipboard",
        ChoiceDelivery::Typed => "typing the chosen path",
    })?;
    Ok(delivery)
}

/// How to deliver a picked path given whether focus was restored: the output
/// mode to dispatch with, and what to tell the user happened.
///
/// Split out from the dispatch so the decision is testable without a
/// clipboard, which needs a display server the CI runners do not have.
fn choice_delivery(output_mode: OutputMode, restored: bool) -> (OutputMode, ChoiceDelivery) {
    // Clipboard and stdout put nothing in a window, so they need no target.
    let needs_focused_target = !matches!(output_mode, OutputMode::Clipboard | OutputMode::Stdout);
    if needs_focused_target && !restored {
        return (OutputMode::Clipboard, ChoiceDelivery::Copied);
    }
    (output_mode, ChoiceDelivery::Typed)
}

/// Run a command-mode transcript through Tier 1 routing and the guarded
/// executor. A gated (`Pending`) result is stored for [`confirm_pending`];
/// it is never executed here. The transcript is spoken content and is never
/// logged above trace.
#[tauri::command]
pub(crate) async fn run_command(
    state: State<'_, AppState>,
    transcript: String,
) -> Result<ExecOutcomeDto, String> {
    let mut command = state.command.lock().await;
    let CommandState {
        allowed: _,
        grammar,
        executor,
        backend,
        pending,
        choice,
    } = &mut *command;
    let outcome = route_transcript(grammar, &transcript);
    // Every utterance supersedes both gates: whatever dialog is on screen is
    // now stale, and its click must not act on the older resolution.
    pending.clear();
    choice.clear();
    // open_file/go_to_dir need the project file index in app state, which the
    // executor (pure native actions) can't reach, so they are resolved here.
    if let RouteOutcome::Command(matched) = &outcome
        && (matched.command_id == CMD_OPEN_FILE || matched.command_id == CMD_GO_TO_DIR)
    {
        let resolved = if matched.command_id == CMD_OPEN_FILE {
            run_open_file(&state, matched)
        } else {
            run_go_to_dir(&state, matched)
        };
        let (mut dto, new_choice) = resolved.map_err(|e| format!("{e:#}"))?;
        if let Some(pending_choice) = new_choice {
            let stashed = choice.stash(pending_choice);
            if let ExecOutcomeDto::Choose { nonce, .. } = &mut dto {
                *nonce = stashed;
            }
        }
        return Ok(dto);
    }
    let (mut dto, new_pending) = execute_and_split(executor, &*backend, outcome)
        .await
        .map_err(|e| format!("{e:#}"))?;
    // Each utterance supersedes any stale pending action, and the nonce binds
    // the confirm dialog to this exact action, so a confirm click can never
    // run something other than what the dialog showed.
    match new_pending {
        Some(action) => {
            let stashed = pending.stash(action);
            if let ExecOutcomeDto::Pending { nonce, .. } = &mut dto {
                *nonce = stashed;
            }
        }
        None => pending.clear(),
    }
    Ok(dto)
}

/// Execute the stored pending action. Only the confirm dialog's physical
/// click or keypress invokes this; voice can never reach it because no
/// routed outcome maps here. `nonce` must match the displayed action's, so a
/// click racing a superseding utterance is refused rather than misfiring.
#[tauri::command]
pub(crate) async fn confirm_pending(
    state: State<'_, AppState>,
    nonce: u64,
) -> Result<ExecOutcomeDto, String> {
    let mut command = state.command.lock().await;
    let Some(pending) = command.pending.take(nonce) else {
        return Err("that action is no longer awaiting confirmation".to_string());
    };
    let outcome = confirm_and_execute(pending, &command.backend)
        .await
        .map_err(|e| format!("{e:#}"))?;
    Ok(split_outcome(outcome).0)
}

/// Drop the stored pending action without running it. A stale nonce is a
/// no-op so cancelling an outdated dialog never discards a newer action.
#[tauri::command]
pub(crate) async fn cancel_pending(state: State<'_, AppState>, nonce: u64) -> Result<(), String> {
    let dropped = state.command.lock().await.pending.take(nonce).is_some();
    tracing::info!(dropped, "pending command action cancelled");
    Ok(())
}

/// Deliver the candidate the user physically picked in the disambiguation
/// overlay. `nonce` must match the displayed set's, so a click racing a
/// superseding utterance is refused rather than inserting a stale path.
#[tauri::command]
pub(crate) async fn choose_candidate(
    state: State<'_, AppState>,
    nonce: u64,
    index: usize,
) -> Result<ChoiceDelivery, String> {
    let mut command = state.command.lock().await;
    let Some(choice) = command.choice.take(nonce) else {
        return Err("those suggestions are no longer available".to_string());
    };
    let Some(chosen) = choice.candidates.get(index) else {
        return Err("that suggestion is out of range".to_string());
    };
    tracing::trace!(path = %chosen.path, "spoken path disambiguation picked");
    deliver_chosen_path(
        &chosen.path,
        choice.output_mode,
        #[cfg(windows)]
        choice.target_hwnd,
    )
    .map_err(|e| format!("{e:#}"))
}

/// Drop the stored candidates without inserting any of them. A stale nonce is
/// a no-op so dismissing an outdated picker never discards a newer one.
#[tauri::command]
pub(crate) async fn cancel_choice(state: State<'_, AppState>, nonce: u64) -> Result<(), String> {
    let dropped = state.command.lock().await.choice.take(nonce).is_some();
    tracing::info!(dropped, "spoken path disambiguation cancelled");
    Ok(())
}

/// The parsed command-mode shortcut; `None` (with a warning) should the
/// constant ever fail to parse.
pub(crate) fn hotkey_shortcut() -> Option<Shortcut> {
    match COMMAND_MODE_HOTKEY.parse() {
        Ok(shortcut) => Some(shortcut),
        Err(e) => {
            tracing::warn!(
                hotkey = COMMAND_MODE_HOTKEY,
                error = ?e,
                "command-mode hotkey failed to parse"
            );
            None
        }
    }
}

/// Register the command-mode hotkey. Failure is non-fatal: dictation is
/// unaffected and command mode stays reachable once UI wiring lands.
pub(crate) fn register_hotkey(app: &tauri::App) {
    let Some(shortcut) = hotkey_shortcut() else {
        return;
    };
    match app.global_shortcut().register(shortcut) {
        Ok(()) => {
            tracing::info!(
                hotkey = COMMAND_MODE_HOTKEY,
                "registered command-mode hotkey"
            );
        }
        Err(e) => {
            tracing::warn!(
                hotkey = COMMAND_MODE_HOTKEY,
                error = ?e,
                "could not register command-mode hotkey"
            );
        }
    }
}

/// Flip the command-mode flag and announce the new state to every window
/// (design Section 5: visible mode state).
pub(crate) fn toggle_mode(app: &tauri::AppHandle) {
    let state = app.state::<AppState>();
    let active = !state.command_mode.fetch_xor(true, Ordering::AcqRel);
    tracing::info!(active, "command mode toggled");
    let _ = app.emit(
        "command-mode-changed",
        serde_json::json!({ "active": active }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    use murmur_core::command::{Permission, RiskTier, Tool};
    use serde_json::json;
    use std::sync::{Arc, Mutex};

    fn tool(name: &str, required: &[&str]) -> Tool {
        Tool {
            name: name.to_string(),
            description: String::new(),
            input_schema: serde_json::json!({ "required": required }),
            risk: murmur_core::command::RiskTier::ReadOnly,
        }
    }

    #[test]
    fn no_native_command_id_contains_the_tool_separator() {
        // Routing tells a native command from a server tool by that separator
        // alone, so a native id gaining one would silently reroute the command
        // as a tool call. Pin it rather than leave it to a comment.
        let grammar = starter_grammar().expect("starter grammar");
        for id in grammar.command_ids() {
            assert!(
                !id.contains(TOOL_ID_SEPARATOR),
                "native command id {id:?} would be routed as a tool call"
            );
        }
    }

    #[test]
    fn a_tool_id_routes_as_a_tool_call_and_a_command_does_not() {
        // The whole bridge: a grammar hit whose id names a tool must become a
        // ToolCall, while a native command must stay a Command.
        let mut g = Grammar::new();
        g.add("open_file", "open file").expect("native");
        g.add("git/status", "git status").expect("tool");
        assert!(matches!(
            route_transcript(&g, "git status"),
            RouteOutcome::ToolCall { ref tool, .. } if tool == "git/status"
        ));
        assert!(matches!(
            route_transcript(&g, "open file"),
            RouteOutcome::Command(_)
        ));
        assert!(matches!(
            route_transcript(&g, "nonsense"),
            RouteOutcome::NoMatch
        ));
    }

    #[test]
    fn a_tool_phrase_never_shadows_a_native_command() {
        // Registration order is priority order, so a server offering a tool
        // named after a built-in must not capture the phrase.
        let mut g = Grammar::new();
        g.add("undo_that", "undo that").expect("native");
        register_tool_phrases(&mut g, &[tool("evil/undo_that", &[])]);
        assert!(matches!(
            route_transcript(&g, "undo that"),
            RouteOutcome::Command(_)
        ));
    }

    #[test]
    fn only_tools_without_required_arguments_become_speakable() {
        // A fixed phrase cannot supply arguments, and guessing them for a call
        // the permission store may auto-run is not acceptable.
        let mut g = Grammar::new();
        let added = register_tool_phrases(
            &mut g,
            &[tool("git/status", &[]), tool("fs/delete", &["path"])],
        );
        assert_eq!(added, 1);
        assert!(matches!(
            route_transcript(&g, "git status"),
            RouteOutcome::ToolCall { .. }
        ));
        assert!(matches!(
            route_transcript(&g, "fs delete"),
            RouteOutcome::NoMatch
        ));
    }

    #[test]
    fn a_tool_name_cannot_inject_grammar_syntax() {
        // A server picks its own tool names, and the derived phrase is parsed
        // as a grammar pattern. A bare `{q}` compiles to a free-text slot, so
        // "srv {q}" would match "srv <anything>" and turn one tool into a
        // catch-all for an unbounded family of phrases.
        let mut g = Grammar::new();
        register_tool_phrases(&mut g, &[tool("srv/{q}", &[])]);
        assert!(
            matches!(
                route_transcript(&g, "srv delete everything"),
                RouteOutcome::NoMatch
            ),
            "a server-chosen name captured an arbitrary phrase"
        );
    }

    #[test]
    fn metacharacters_are_stripped_from_derived_phrases() {
        for name in ["a/{q}", "a/(x|y)", "a/[opt]", "a/{n:0..9}"] {
            let phrase = spoken_phrase_for_tool(name).unwrap_or_default();
            assert!(
                !phrase.contains(['{', '}', '(', ')', '[', ']', '|', ':']),
                "{name} produced grammar syntax: {phrase:?}"
            );
        }
    }

    #[test]
    fn tool_names_become_speakable_phrases() {
        assert_eq!(
            spoken_phrase_for_tool("git/status").as_deref(),
            Some("git status")
        );
        assert_eq!(
            spoken_phrase_for_tool("fs/list_directory").as_deref(),
            Some("fs list directory")
        );
        assert_eq!(
            spoken_phrase_for_tool("A/B-c.d").as_deref(),
            Some("a b c d")
        );
        assert_eq!(spoken_phrase_for_tool("//").as_deref(), None);
    }

    /// The gate the allowlist exists to control: with nothing configured the
    /// backend refuses every server, which is the behaviour that shipped when
    /// the list was hardcoded empty.
    #[test]
    fn an_empty_allowlist_trusts_no_server() {
        let state =
            CommandState::new(Vec::new(), PermissionStore::default()).expect("command state");
        assert!(!state.backend.is_allowed("git"));
        assert!(!state.backend.is_allowed("filesystem"));
    }

    #[test]
    fn configured_servers_reach_the_backend_and_others_still_do_not() {
        let state = CommandState::new(
            vec!["git".to_string(), "filesystem".to_string()],
            PermissionStore::default(),
        )
        .expect("state");
        assert!(state.backend.is_allowed("git"));
        assert!(state.backend.is_allowed("filesystem"));
        assert!(!state.backend.is_allowed("something-else"));
    }

    #[derive(Clone, Default)]
    struct RecordingActions {
        calls: Arc<Mutex<Vec<String>>>,
    }

    impl RecordingActions {
        fn record(&self, entry: String) -> anyhow::Result<()> {
            self.calls
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(entry);
            Ok(())
        }

        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap_or_else(|e| e.into_inner()).clone()
        }
    }

    impl NativeActions for RecordingActions {
        fn launch(&self, target: &str) -> anyhow::Result<()> {
            self.record(format!("launch {target}"))
        }

        fn focus_window(&self, query: &str) -> anyhow::Result<()> {
            self.record(format!("focus {query}"))
        }

        fn send_keys(&self, keys: &str) -> anyhow::Result<()> {
            self.record(format!("keys {keys}"))
        }
    }

    #[derive(Default)]
    struct RecordingBackend {
        calls: Mutex<Vec<(String, Value)>>,
    }

    impl RecordingBackend {
        fn calls(&self) -> Vec<(String, Value)> {
            self.calls.lock().unwrap_or_else(|e| e.into_inner()).clone()
        }
    }

    impl ToolBackend for RecordingBackend {
        async fn invoke(&self, tool: &str, args: Value) -> anyhow::Result<Value> {
            self.calls
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push((tool.to_string(), args));
            Ok(json!({"ok": true}))
        }
    }

    fn destructive_executor() -> Executor<RecordingActions> {
        let mut store = PermissionStore::default();
        store.set("git/push", Permission::Allow);
        let mut executor = Executor::new(RecordingActions::default(), store);
        executor.set_tools([Tool {
            name: "git/push".into(),
            description: "Push commits to the remote".into(),
            input_schema: json!({"type": "object"}),
            risk: RiskTier::Destructive,
        }]);
        executor
    }

    #[tokio::test]
    async fn grammar_transcript_executes_native_command() {
        let actions = RecordingActions::default();
        let executor = Executor::new(actions.clone(), PermissionStore::default());
        let grammar = starter_grammar().expect("starter grammar compiles");
        let backend = RecordingBackend::default();

        let (dto, pending) = execute_and_split(
            &executor,
            &backend,
            route_transcript(&grammar, "open firefox"),
        )
        .await
        .expect("execute");

        assert_eq!(
            dto,
            ExecOutcomeDto::Executed {
                result: Value::Null
            }
        );
        assert!(pending.is_none());
        assert_eq!(actions.calls(), vec!["launch firefox".to_string()]);
        assert!(backend.calls().is_empty());
    }

    #[tokio::test]
    async fn unmatched_transcript_is_no_action() {
        let actions = RecordingActions::default();
        let executor = Executor::new(actions.clone(), PermissionStore::default());
        let grammar = starter_grammar().expect("starter grammar compiles");
        let backend = RecordingBackend::default();

        let (dto, pending) = execute_and_split(
            &executor,
            &backend,
            route_transcript(&grammar, "make me a sandwich"),
        )
        .await
        .expect("execute");

        assert_eq!(dto, ExecOutcomeDto::NoAction);
        assert!(pending.is_none());
        assert!(actions.calls().is_empty());
        assert!(backend.calls().is_empty());
    }

    #[test]
    fn open_file_routes_as_a_command_for_interception() {
        // run_command relies on the routed id to divert open_file to the
        // project file index before the executor sees it.
        let grammar = starter_grammar().expect("starter grammar compiles");
        match route_transcript(&grammar, "open the user controller test file") {
            RouteOutcome::Command(matched) => assert_eq!(matched.command_id, CMD_OPEN_FILE),
            _ => panic!("expected an open_file command match"),
        }
    }

    #[test]
    fn go_to_dir_routes_as_a_command_for_interception() {
        let grammar = starter_grammar().expect("starter grammar compiles");
        match route_transcript(&grammar, "navigate to the source components folder") {
            RouteOutcome::Command(matched) => assert_eq!(matched.command_id, CMD_GO_TO_DIR),
            _ => panic!("expected a go_to_dir command match"),
        }
    }

    fn pending_action(tool: &str) -> PendingAction {
        PendingAction {
            tool: tool.into(),
            args: json!({}),
            reversible: true,
        }
    }

    #[test]
    fn confirm_with_matching_nonce_takes_the_action() {
        let mut gate = PendingGate::default();
        let nonce = gate.stash(pending_action("git/push"));
        let taken = gate.take(nonce).expect("matching nonce takes the action");
        assert_eq!(taken.tool, "git/push");
        assert!(gate.take(nonce).is_none(), "slot is emptied after take");
    }

    #[test]
    fn stale_nonce_cannot_take_a_superseding_action() {
        let mut gate = PendingGate::default();
        let old_nonce = gate.stash(pending_action("git/push"));
        let new_nonce = gate.stash(pending_action("fs/delete"));
        assert_ne!(old_nonce, new_nonce);
        assert!(
            gate.take(old_nonce).is_none(),
            "a confirm for the superseded dialog must not run the new action"
        );
        let taken = gate
            .take(new_nonce)
            .expect("the superseding action survives a stale take");
        assert_eq!(taken.tool, "fs/delete");
    }

    #[test]
    fn clear_drops_the_action_for_any_nonce() {
        let mut gate = PendingGate::default();
        let nonce = gate.stash(pending_action("git/push"));
        gate.clear();
        assert!(gate.take(nonce).is_none());
    }

    #[test]
    fn dto_serializes_with_kind_tags() {
        let executed = serde_json::to_value(ExecOutcomeDto::Executed {
            result: json!({"ok": true}),
        })
        .expect("serialize");
        assert_eq!(executed["kind"], "executed");
        assert_eq!(executed["result"]["ok"], true);

        let pending = serde_json::to_value(ExecOutcomeDto::Pending {
            tool: "git/push".into(),
            args: json!({"remote": "origin"}),
            reversible: false,
            nonce: 7,
        })
        .expect("serialize");
        assert_eq!(pending["kind"], "pending");
        assert_eq!(pending["tool"], "git/push");
        assert_eq!(pending["args"]["remote"], "origin");
        assert_eq!(pending["reversible"], false);
        assert_eq!(pending["nonce"], 7);

        let choose = serde_json::to_value(ExecOutcomeDto::Choose {
            candidates: vec!["src/api/user.ts".into(), "src/web/user.ts".into()],
            nonce: 3,
        })
        .expect("serialize");
        assert_eq!(choose["kind"], "choose");
        assert_eq!(choose["candidates"][1], "src/web/user.ts");
        assert_eq!(choose["nonce"], 3);

        let blocked = serde_json::to_value(ExecOutcomeDto::Blocked).expect("serialize");
        assert_eq!(blocked["kind"], "blocked");
        let no_action = serde_json::to_value(ExecOutcomeDto::NoAction).expect("serialize");
        assert_eq!(no_action["kind"], "no_action");
    }

    fn pending_choice(paths: &[&str]) -> PendingChoice {
        PendingChoice {
            candidates: paths
                .iter()
                .map(|p| FileMatch {
                    path: (*p).to_string(),
                    score: 1.0,
                })
                .collect(),
            output_mode: OutputMode::Auto,
            #[cfg(windows)]
            target_hwnd: 0,
        }
    }

    #[test]
    fn choice_delivery_serializes_snake_case() {
        assert_eq!(
            serde_json::to_value(ChoiceDelivery::Copied).expect("serialize"),
            Value::String("copied".into())
        );
        assert_eq!(
            serde_json::to_value(ChoiceDelivery::Typed).expect("serialize"),
            Value::String("typed".into())
        );
    }

    #[test]
    fn a_pick_never_types_when_focus_was_not_restored() {
        // Clicking the picker activates Murmur's own window. Without focus
        // handed back, typing would insert the path into our own UI, which is
        // the exact failure the choice gate exists to prevent. Non-Windows
        // cannot restore focus at all, so it always takes this branch.
        for mode in [
            OutputMode::Auto,
            OutputMode::Keyboard,
            OutputMode::ClipboardPaste,
        ] {
            assert_eq!(
                choice_delivery(mode, false),
                (OutputMode::Clipboard, ChoiceDelivery::Copied),
                "{mode:?} must not type into an unrestored target"
            );
            assert_eq!(
                choice_delivery(mode, true),
                (mode, ChoiceDelivery::Typed),
                "{mode:?} types once focus is back"
            );
        }
        // These place nothing in a window, so a missing target is irrelevant.
        for mode in [OutputMode::Clipboard, OutputMode::Stdout] {
            assert_eq!(
                choice_delivery(mode, false),
                (mode, ChoiceDelivery::Typed),
                "{mode:?} needs no focused window"
            );
        }
    }

    #[test]
    fn a_superseding_choice_refuses_the_stale_pick() {
        let mut gate: Gate<PendingChoice> = Gate::default();
        let old_nonce = gate.stash(pending_choice(&["a/user.ts", "b/user.ts"]));
        let new_nonce = gate.stash(pending_choice(&["c/order.ts", "d/order.ts"]));
        assert_ne!(old_nonce, new_nonce);
        assert!(
            gate.take(old_nonce).is_none(),
            "a pick for the superseded picker must not insert the new candidates"
        );
        let taken = gate.take(new_nonce).expect("the newer set survives");
        assert_eq!(taken.candidates[0].path, "c/order.ts");
        assert!(gate.take(new_nonce).is_none(), "slot is emptied after take");
    }

    #[test]
    fn clearing_the_choice_gate_drops_the_candidates() {
        // Each new utterance clears both gates, so the open picker's click
        // finds nothing to act on.
        let mut gate: Gate<PendingChoice> = Gate::default();
        let nonce = gate.stash(pending_choice(&["a/user.ts", "b/user.ts"]));
        gate.clear();
        assert!(gate.take(nonce).is_none());
    }

    #[tokio::test]
    async fn destructive_pending_only_runs_via_confirm() {
        let executor = destructive_executor();
        let backend = RecordingBackend::default();

        let outcome = executor
            .execute(
                RouteOutcome::ToolCall {
                    tool: "git/push".into(),
                    arguments: json!({"remote": "origin"}),
                },
                &backend,
            )
            .await
            .expect("execute");
        let (dto, pending) = split_outcome(outcome);

        // The gated result reaches the UI with the full argument echo, and
        // nothing has run yet: run_command has no path that executes it.
        assert!(matches!(
            dto,
            ExecOutcomeDto::Pending {
                reversible: false,
                ..
            }
        ));
        assert!(
            backend.calls().is_empty(),
            "destructive action must not run before physical confirmation"
        );

        // The only execution path is confirm_and_execute, which is what
        // confirm_pending (physical click or keypress) calls.
        let pending = pending.expect("pending action stored");
        let confirmed = confirm_and_execute(pending, &backend)
            .await
            .expect("confirm");
        assert_eq!(confirmed, ExecOutcome::Executed(json!({"ok": true})));
        assert_eq!(
            backend.calls(),
            vec![("git/push".to_string(), json!({"remote": "origin"}))]
        );
    }

    #[tokio::test]
    async fn cancelled_pending_never_touches_the_backend() {
        let executor = destructive_executor();
        let backend = RecordingBackend::default();

        let outcome = executor
            .execute(
                RouteOutcome::ToolCall {
                    tool: "git/push".into(),
                    arguments: json!({}),
                },
                &backend,
            )
            .await
            .expect("execute");
        let (_, pending) = split_outcome(outcome);

        // cancel_pending just drops the stored action.
        drop(pending);
        assert!(backend.calls().is_empty());
    }
}
