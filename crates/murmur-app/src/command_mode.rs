//! Command-mode wiring for the desktop app (Phase 0 of
//! docs/command-mode-design.md): the mode toggle behind its own global
//! hotkey, the transcript-to-action Tauri commands, and the pending-action
//! store behind the physical-confirm gate.
//!
//! Safety spine, restated: [`run_command`] can produce a `Pending` action
//! but never runs it. Only [`confirm_pending`], invoked by a real click or
//! keypress in the confirm dialog, executes it. There is deliberately no
//! voice path to confirmation (design Section 5).

use std::sync::atomic::Ordering;

use anyhow::Context;
use murmur_core::command::{Grammar, PermissionStore, RouteOutcome};
use murmur_mcp::ActionBackend;
use serde_json::Value;
use tauri::{Emitter, Manager, State};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};

use crate::command_exec::{
    ExecOutcome, Executor, PendingAction, ToolBackend, confirm_and_execute, starter_grammar,
};
use crate::native_actions::{NativeActions, SystemActions};
use crate::state::AppState;

/// Default command-mode hotkey: a separate activation channel from the
/// dictation hotkey (design Section 5), configurable later.
pub(crate) const COMMAND_MODE_HOTKEY: &str = "ctrl+shift+period";

/// Everything `run_command` needs, kept behind one async lock in app state
/// (execution awaits the tool backend while holding it).
pub(crate) struct CommandState {
    grammar: Grammar,
    executor: Executor<SystemActions>,
    backend: ActionBackend,
    /// The single gated action awaiting physical confirmation, if any.
    pending: Option<PendingAction>,
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
            pending: None,
        })
    }
}

/// Serializable mirror of [`ExecOutcome`] for the frontend. `Pending`
/// carries the tool name and parsed arguments so the confirm dialog can
/// echo exactly what the ASR produced.
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

/// Route one transcript and run it through the guarded executor. Returns
/// the DTO plus any pending action for the caller to stash. Free function
/// so tests can drive it with mock actions and backends.
async fn route_and_execute<A: NativeActions, B: ToolBackend>(
    grammar: &Grammar,
    executor: &Executor<A>,
    backend: &B,
    transcript: &str,
) -> anyhow::Result<(ExecOutcomeDto, Option<PendingAction>)> {
    let outcome = route_transcript(grammar, transcript);
    let executed = executor.execute(outcome, backend).await?;
    Ok(split_outcome(executed))
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
    let (dto, new_pending) = route_and_execute(grammar, executor, &*backend, &transcript)
        .await
        .map_err(|e| format!("{e:#}"))?;
    // Each utterance supersedes any stale pending action, so the confirm
    // dialog can never run something other than what it currently shows.
    *pending = new_pending;
    Ok(dto)
}

/// Execute the stored pending action. Only the confirm dialog's physical
/// click or keypress invokes this; voice can never reach it because no
/// routed outcome maps here.
#[tauri::command]
pub(crate) async fn confirm_pending(state: State<'_, AppState>) -> Result<ExecOutcomeDto, String> {
    let mut command = state.command.lock().await;
    let Some(pending) = command.pending.take() else {
        return Err("no action is awaiting confirmation".to_string());
    };
    let outcome = confirm_and_execute(pending, &command.backend)
        .await
        .map_err(|e| format!("{e:#}"))?;
    Ok(split_outcome(outcome).0)
}

/// Drop the stored pending action without running it.
#[tauri::command]
pub(crate) async fn cancel_pending(state: State<'_, AppState>) -> Result<(), String> {
    let dropped = state.command.lock().await.pending.take().is_some();
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

        let (dto, pending) = route_and_execute(&grammar, &executor, &backend, "open firefox")
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

        let (dto, pending) = route_and_execute(&grammar, &executor, &backend, "make me a sandwich")
            .await
            .expect("execute");

        assert_eq!(dto, ExecOutcomeDto::NoAction);
        assert!(pending.is_none());
        assert!(actions.calls().is_empty());
        assert!(backend.calls().is_empty());
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
        })
        .expect("serialize");
        assert_eq!(pending["kind"], "pending");
        assert_eq!(pending["tool"], "git/push");
        assert_eq!(pending["args"]["remote"], "origin");
        assert_eq!(pending["reversible"], false);

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
