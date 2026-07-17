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
use murmur_core::command::{Grammar, Match, PermissionStore, RouteOutcome, SlotValue};
use murmur_core::indexer::FileMatch;
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

/// The single gated action awaiting physical confirmation, bound to a nonce.
///
/// ASR stays live while the confirm dialog is open, so a newer utterance can
/// supersede the stored action before the dialog re-renders. The nonce binds a
/// confirm/cancel click to the exact action the dialog displayed: a stale
/// click is refused instead of running an action the user never reviewed.
#[derive(Default)]
struct PendingGate {
    last_nonce: u64,
    slot: Option<(u64, PendingAction)>,
}

impl PendingGate {
    /// Stash a new pending action, superseding any previous one, and return
    /// the nonce the confirm dialog must echo back.
    fn stash(&mut self, action: PendingAction) -> u64 {
        self.last_nonce += 1;
        self.slot = Some((self.last_nonce, action));
        self.last_nonce
    }

    /// Drop whatever is stored (a non-pending outcome supersedes it).
    fn clear(&mut self) {
        self.slot = None;
    }

    /// Take the action only if `nonce` matches the stored one; a stale nonce
    /// returns `None` and leaves the current action in place.
    fn take(&mut self, nonce: u64) -> Option<PendingAction> {
        match self.slot.take() {
            Some((stored, action)) if stored == nonce => Some(action),
            other => {
                self.slot = other;
                None
            }
        }
    }
}

/// Everything `run_command` needs, kept behind one async lock in app state
/// (execution awaits the tool backend while holding it).
pub(crate) struct CommandState {
    grammar: Grammar,
    executor: Executor<SystemActions>,
    backend: ActionBackend,
    /// The single gated action awaiting physical confirmation, if any.
    pending: PendingGate,
}

impl CommandState {
    /// The Phase 0 context: the starter grammar over native actions, the
    /// saved permission policy, and an MCP backend with an empty allowlist
    /// (no servers connect until the Phase 2 wiring lands).
    pub(crate) fn new() -> anyhow::Result<Self> {
        Ok(Self {
            grammar: starter_grammar().context("building the starter command grammar")?,
            executor: Executor::new(SystemActions, PermissionStore::load()),
            backend: ActionBackend::new(std::iter::empty::<String>()),
            pending: PendingGate::default(),
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
    grammar
        .match_phrase(transcript)
        .map(RouteOutcome::Command)
        .unwrap_or(RouteOutcome::NoMatch)
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
/// paths and type the best match's relative path. Handled here rather than
/// in the executor because it needs `AppState::project_files`. The query is
/// spoken content and is never logged; the resolved path only at trace.
// TODO: disambiguation overlay for near-tie scores and the Tier-2 embedding
// fallback (viable-features.md §1.1); v1 types the single best match.
fn run_open_file(state: &AppState, matched: &Match) -> anyhow::Result<ExecOutcomeDto> {
    let (query, output_mode) = aliased_query(state, matched)?;
    let best = {
        let files = state
            .project_files
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        murmur_core::indexer::resolve_file(&query, &files)
            .into_iter()
            .next()
    };
    type_best_match(best, output_mode)
}

/// The directory analog of [`run_open_file`]: resolve "go to the … folder"
/// against the ancestor-directory set of the indexed files and type the best
/// match's relative path. Same privacy rules.
fn run_go_to_dir(state: &AppState, matched: &Match) -> anyhow::Result<ExecOutcomeDto> {
    let (query, output_mode) = aliased_query(state, matched)?;
    let dirs = {
        let files = state
            .project_files
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        murmur_core::indexer::directories(&files)
    };
    let best = murmur_core::indexer::resolve_file(&query, &dirs)
        .into_iter()
        .next();
    type_best_match(best, output_mode)
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

/// Type the best match's relative path, or `NoAction` when nothing overlapped.
fn type_best_match(
    best: Option<FileMatch>,
    output_mode: OutputMode,
) -> anyhow::Result<ExecOutcomeDto> {
    let Some(best) = best else {
        tracing::debug!("no indexed path matched the spoken query");
        return Ok(ExecOutcomeDto::NoAction);
    };
    tracing::trace!(path = %best.path, score = best.score, "spoken path query resolved");
    murmur_core::output::dispatch_verbatim(&best.path, output_mode)
        .context("typing the resolved path")?;
    Ok(ExecOutcomeDto::Executed {
        result: Value::Null,
    })
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
        grammar,
        executor,
        backend,
        pending,
    } = &mut *command;
    let outcome = route_transcript(grammar, &transcript);
    // open_file/go_to_dir need the project file index in app state, which the
    // executor (pure native actions) can't reach, so they are resolved here.
    if let RouteOutcome::Command(matched) = &outcome
        && matched.command_id == CMD_OPEN_FILE
    {
        pending.clear();
        return run_open_file(&state, matched).map_err(|e| format!("{e:#}"));
    }
    if let RouteOutcome::Command(matched) = &outcome
        && matched.command_id == CMD_GO_TO_DIR
    {
        pending.clear();
        return run_go_to_dir(&state, matched).map_err(|e| format!("{e:#}"));
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

        let blocked = serde_json::to_value(ExecOutcomeDto::Blocked).expect("serialize");
        assert_eq!(blocked["kind"], "blocked");
        let no_action = serde_json::to_value(ExecOutcomeDto::NoAction).expect("serialize");
        assert_eq!(no_action["kind"], "no_action");
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
