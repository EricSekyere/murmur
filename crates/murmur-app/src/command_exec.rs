//! Turns a routed voice-command outcome into a guarded action: native
//! grammar matches dispatch straight to
//! [`NativeActions`]; tool calls pass through the [`PermissionStore`] gate
//! and only auto-run when policy and risk tier both allow it.
//!
//! Safety invariant, restated because this is the enforcement point: a
//! Destructive tool never auto-runs, even under `Permission::Allow`. Voice
//! is an untrusted input channel, so destructive actions always come back as
//! [`ExecOutcome::Pending`], and only [`confirm_and_execute`], called by the
//! UI after a physical keyboard or mouse confirmation (never a spoken one),
//! actually runs them.

use std::collections::HashMap;

use anyhow::{Context, Result, bail};
use murmur_core::command::{
    Decision, Grammar, Match, PermissionStore, RiskTier, RouteOutcome, SlotValue, Tool,
};
use murmur_mcp::ActionBackend;
use serde_json::Value;

use crate::native_actions::NativeActions;

/// Async tool execution, abstracted so the gating logic is unit-testable
/// without spawning MCP server processes.
pub trait ToolBackend {
    /// Execute a tool by its registry name with JSON-object arguments.
    fn invoke(&self, tool: &str, args: Value) -> impl Future<Output = Result<Value>> + Send;
}

impl ToolBackend for ActionBackend {
    /// Split the registry name (`server/tool`) and dispatch over MCP. The
    /// permission decision has already been made by [`Executor::execute`].
    async fn invoke(&self, tool: &str, args: Value) -> Result<Value> {
        let (server, name) = tool
            .split_once('/')
            .with_context(|| format!("tool name '{tool}' is not namespaced as server/tool"))?;
        let result = self.call_tool(server, name, args).await?;
        serde_json::to_value(&result).context("serializing MCP tool result")
    }
}

/// A gated action waiting for the user's physical confirmation.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingAction {
    /// Registry tool name (namespaced `server/tool` for MCP tools).
    pub tool: String,
    /// Parsed arguments; the confirmation UI echoes these back verbatim,
    /// because the user cannot see what the ASR heard.
    pub args: Value,
    /// False for Destructive tools: the UI must show the full argument echo,
    /// not a lightweight undo-style confirm.
    pub reversible: bool,
}

/// What the executor did with a routed outcome.
#[derive(Debug, Clone, PartialEq)]
pub enum ExecOutcome {
    /// Ran; carries the tool's JSON result (Null for native actions).
    Executed(Value),
    /// Needs a physical (keyboard or mouse, never voice) confirmation first.
    Pending(PendingAction),
    /// Refused by the permission policy or unknown to the registry.
    Blocked,
    /// Nothing to run: no match, or no action mapped yet.
    NoAction,
}

/// Command ids for the native starter set; [`starter_grammar`] and
/// [`Executor::run_native`] stay in sync through these.
const CMD_LAUNCH: &str = "native.launch";
const CMD_FOCUS: &str = "native.focus";
const CMD_PRESS: &str = "native.press";
const CMD_PASTE: &str = "native.paste";
/// Spoken file resolution. Not a native action: it needs the app's project
/// file index, so `command_mode::run_command` intercepts it before the
/// executor.
pub(crate) const CMD_OPEN_FILE: &str = "open_file";

/// The paste chord "paste" sends, in [`NativeActions::send_keys`] syntax.
#[cfg(target_os = "macos")]
const PASTE_KEYS: &str = "command v";
#[cfg(not(target_os = "macos"))]
const PASTE_KEYS: &str = "control v";

/// The Phase 1 spoken command set backed by native actions.
///
/// # Errors
/// Only if a pattern above is edited into an invalid form; surfacing that
/// beats a startup panic.
pub fn starter_grammar() -> Result<Grammar> {
    let mut grammar = Grammar::new();
    for (id, pattern) in [
        // Before the launch/focus catch-alls: first added pattern wins, so
        // "open the user controller file" resolves a file rather than
        // launching an app named "the user controller file".
        (CMD_OPEN_FILE, "(open|go to) [the] {query} file"),
        (CMD_LAUNCH, "(open|launch|start) {target}"),
        (CMD_FOCUS, "(switch|go) to [the] {query}"),
        (CMD_FOCUS, "focus [on] [the] {query}"),
        (CMD_PRESS, "(press|hit) {keys}"),
        (CMD_PASTE, "paste [from] [my|the] [clipboard]"),
    ] {
        grammar
            .add(id, pattern)
            .with_context(|| format!("registering command pattern '{id}'"))?;
    }
    Ok(grammar)
}

/// Executes routed outcomes against the saved permission policy, the known
/// tool registry, and a set of native actions.
pub struct Executor<A: NativeActions> {
    actions: A,
    permissions: PermissionStore,
    tools: HashMap<String, Tool>,
}

impl<A: NativeActions> Executor<A> {
    pub fn new(actions: A, permissions: PermissionStore) -> Self {
        Self {
            actions,
            permissions,
            tools: HashMap::new(),
        }
    }

    /// Replace the known-tool registry (after MCP tool discovery).
    pub fn set_tools(&mut self, tools: impl IntoIterator<Item = Tool>) {
        self.tools = tools
            .into_iter()
            .map(|tool| (tool.name.clone(), tool))
            .collect();
    }

    /// Replace the permission policy (after the user saves a new choice).
    pub fn set_permissions(&mut self, permissions: PermissionStore) {
        self.permissions = permissions;
    }

    /// Turn a routed outcome into an executed, pending, or refused action.
    pub async fn execute<B: ToolBackend>(
        &self,
        outcome: RouteOutcome,
        backend: &B,
    ) -> Result<ExecOutcome> {
        match outcome {
            RouteOutcome::Command(matched) => self.run_native(&matched),
            RouteOutcome::ToolCall { tool, arguments } => {
                self.gate_tool_call(tool, arguments, backend).await
            }
            RouteOutcome::Intent(intent) => {
                // Tier 2 intents get an action mapping when that tier ships.
                tracing::debug!(intent_id = %intent.intent_id, "no action mapped for intent");
                Ok(ExecOutcome::NoAction)
            }
            RouteOutcome::NoMatch => Ok(ExecOutcome::NoAction),
        }
    }

    /// Apply `deny > ask > allow` plus the risk tier to one tool call.
    async fn gate_tool_call<B: ToolBackend>(
        &self,
        name: String,
        args: Value,
        backend: &B,
    ) -> Result<ExecOutcome> {
        let Some(tool) = self.tools.get(&name) else {
            // An unregistered tool has no risk tier to gate on, so refuse.
            tracing::warn!(tool = %name, "tool call for unregistered tool refused");
            return Ok(ExecOutcome::Blocked);
        };
        match self.permissions.decision_for(tool) {
            Decision::Blocked => {
                tracing::info!(tool = %name, "tool blocked by permission policy");
                Ok(ExecOutcome::Blocked)
            }
            // The guard restates the core invariant locally: even if the
            // decision table ever said auto-run, Destructive still confirms.
            Decision::AutoRun | Decision::AutoRunReversible
                if tool.risk != RiskTier::Destructive =>
            {
                tracing::info!(tool = %name, "auto-running tool");
                let result = backend.invoke(&name, args).await?;
                Ok(ExecOutcome::Executed(result))
            }
            Decision::Confirm | Decision::AutoRun | Decision::AutoRunReversible => {
                Ok(ExecOutcome::Pending(PendingAction {
                    reversible: tool.risk != RiskTier::Destructive,
                    tool: name,
                    args,
                }))
            }
        }
    }

    /// Map a Tier 1 grammar match onto a native action. Slot contents come
    /// from the utterance, so they are never logged here.
    fn run_native(&self, matched: &Match) -> Result<ExecOutcome> {
        match matched.command_id.as_str() {
            CMD_LAUNCH => self.actions.launch(required_slot(matched, "target")?)?,
            CMD_FOCUS => self
                .actions
                .focus_window(required_slot(matched, "query")?)?,
            CMD_PRESS => self.actions.send_keys(required_slot(matched, "keys")?)?,
            CMD_PASTE => self.actions.send_keys(PASTE_KEYS)?,
            // Resolved in command_mode::run_command against the app's file
            // index; reaching the executor means that interception broke.
            CMD_OPEN_FILE => {
                tracing::warn!("open_file reached the executor instead of command_mode");
                return Ok(ExecOutcome::NoAction);
            }
            other => {
                tracing::warn!(command_id = %other, "no native action mapped for command");
                return Ok(ExecOutcome::NoAction);
            }
        }
        tracing::info!(command_id = %matched.command_id, "native command executed");
        Ok(ExecOutcome::Executed(Value::Null))
    }
}

/// Run a pending action after the user confirmed it physically (a keyboard
/// press or mouse click in the confirmation UI, never voice). This is the
/// only path that executes a Destructive tool.
pub async fn confirm_and_execute<B: ToolBackend>(
    pending: PendingAction,
    backend: &B,
) -> Result<ExecOutcome> {
    tracing::info!(tool = %pending.tool, "running confirmed action");
    let result = backend.invoke(&pending.tool, pending.args).await?;
    Ok(ExecOutcome::Executed(result))
}

/// A slot's spoken text. A missing slot is a wiring bug between the grammar
/// and the dispatch table, reported as an error rather than a panic.
fn required_slot<'m>(matched: &'m Match, name: &str) -> Result<&'m str> {
    match matched.slots.get(name) {
        Some(SlotValue::Text(text) | SlotValue::Choice(text)) => Ok(text),
        Some(SlotValue::Number(_)) | None => bail!(
            "command '{}' is missing text slot '{name}'",
            matched.command_id
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use murmur_core::command::{IntentMatch, Permission};
    use serde_json::json;
    use std::sync::Mutex;

    #[derive(Default)]
    struct MockActions {
        calls: Mutex<Vec<(&'static str, String)>>,
    }

    impl MockActions {
        fn record(&self, method: &'static str, arg: &str) -> Result<()> {
            self.calls
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push((method, arg.to_string()));
            Ok(())
        }

        fn calls(&self) -> Vec<(&'static str, String)> {
            self.calls.lock().unwrap_or_else(|e| e.into_inner()).clone()
        }
    }

    impl NativeActions for MockActions {
        fn launch(&self, target: &str) -> Result<()> {
            self.record("launch", target)
        }

        fn focus_window(&self, query: &str) -> Result<()> {
            self.record("focus_window", query)
        }

        fn send_keys(&self, keys: &str) -> Result<()> {
            self.record("send_keys", keys)
        }
    }

    #[derive(Default)]
    struct FakeBackend {
        calls: Mutex<Vec<(String, Value)>>,
    }

    impl FakeBackend {
        fn calls(&self) -> Vec<(String, Value)> {
            self.calls.lock().unwrap_or_else(|e| e.into_inner()).clone()
        }
    }

    impl ToolBackend for FakeBackend {
        async fn invoke(&self, tool: &str, args: Value) -> Result<Value> {
            self.calls
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push((tool.to_string(), args));
            Ok(json!({"ok": true}))
        }
    }

    fn tool(name: &str, risk: RiskTier) -> Tool {
        Tool {
            name: name.into(),
            description: format!("{name} test tool"),
            input_schema: json!({"type": "object"}),
            risk,
        }
    }

    fn executor_with(tools: Vec<Tool>, perms: &[(&str, Permission)]) -> Executor<MockActions> {
        let mut store = PermissionStore::default();
        for (name, permission) in perms {
            store.set(*name, *permission);
        }
        let mut exec = Executor::new(MockActions::default(), store);
        exec.set_tools(tools);
        exec
    }

    fn tool_call(name: &str) -> RouteOutcome {
        RouteOutcome::ToolCall {
            tool: name.into(),
            arguments: json!({"path": "README.md"}),
        }
    }

    #[tokio::test]
    async fn allowed_read_only_tool_auto_runs() {
        let exec = executor_with(
            vec![tool("fs/list", RiskTier::ReadOnly)],
            &[("fs/list", Permission::Allow)],
        );
        let backend = FakeBackend::default();
        let outcome = exec
            .execute(tool_call("fs/list"), &backend)
            .await
            .expect("execute");
        assert_eq!(outcome, ExecOutcome::Executed(json!({"ok": true})));
        assert_eq!(
            backend.calls(),
            vec![("fs/list".to_string(), json!({"path": "README.md"}))]
        );
    }

    #[tokio::test]
    async fn allowed_mutating_tool_auto_runs() {
        let exec = executor_with(
            vec![tool("fs/rename", RiskTier::Mutating)],
            &[("fs/rename", Permission::Allow)],
        );
        let backend = FakeBackend::default();
        let outcome = exec
            .execute(tool_call("fs/rename"), &backend)
            .await
            .expect("execute");
        assert_eq!(outcome, ExecOutcome::Executed(json!({"ok": true})));
        assert_eq!(backend.calls().len(), 1);
    }

    #[tokio::test]
    async fn allowed_destructive_tool_is_pending_never_executed() {
        // The core safety property: "always allow" still cannot voice-run a
        // destructive tool.
        let exec = executor_with(
            vec![tool("git/push", RiskTier::Destructive)],
            &[("git/push", Permission::Allow)],
        );
        let backend = FakeBackend::default();
        let outcome = exec
            .execute(tool_call("git/push"), &backend)
            .await
            .expect("execute");
        match outcome {
            ExecOutcome::Pending(pending) => {
                assert_eq!(pending.tool, "git/push");
                assert_eq!(pending.args, json!({"path": "README.md"}));
                assert!(!pending.reversible);
            }
            other => panic!("expected Pending, got {other:?}"),
        }
        assert!(backend.calls().is_empty(), "destructive tool must not run");
    }

    #[tokio::test]
    async fn denied_tool_is_blocked() {
        let exec = executor_with(
            vec![tool("shell/run", RiskTier::ReadOnly)],
            &[("shell/run", Permission::Deny)],
        );
        let backend = FakeBackend::default();
        let outcome = exec
            .execute(tool_call("shell/run"), &backend)
            .await
            .expect("execute");
        assert_eq!(outcome, ExecOutcome::Blocked);
        assert!(backend.calls().is_empty());
    }

    #[tokio::test]
    async fn unknown_permission_defaults_to_ask_and_pends() {
        let exec = executor_with(vec![tool("fs/read", RiskTier::ReadOnly)], &[]);
        let backend = FakeBackend::default();
        let outcome = exec
            .execute(tool_call("fs/read"), &backend)
            .await
            .expect("execute");
        match outcome {
            ExecOutcome::Pending(pending) => {
                assert_eq!(pending.tool, "fs/read");
                assert!(pending.reversible);
            }
            other => panic!("expected Pending, got {other:?}"),
        }
        assert!(backend.calls().is_empty());
    }

    #[tokio::test]
    async fn unregistered_tool_is_blocked() {
        let exec = executor_with(vec![], &[]);
        let backend = FakeBackend::default();
        let outcome = exec
            .execute(tool_call("ghost/anything"), &backend)
            .await
            .expect("execute");
        assert_eq!(outcome, ExecOutcome::Blocked);
        assert!(backend.calls().is_empty());
    }

    #[tokio::test]
    async fn confirm_and_execute_runs_the_pending_action() {
        let exec = executor_with(
            vec![tool("git/push", RiskTier::Destructive)],
            &[("git/push", Permission::Allow)],
        );
        let backend = FakeBackend::default();
        let ExecOutcome::Pending(pending) = exec
            .execute(tool_call("git/push"), &backend)
            .await
            .expect("execute")
        else {
            panic!("expected Pending");
        };

        let outcome = confirm_and_execute(pending, &backend)
            .await
            .expect("confirm");
        assert_eq!(outcome, ExecOutcome::Executed(json!({"ok": true})));
        assert_eq!(
            backend.calls(),
            vec![("git/push".to_string(), json!({"path": "README.md"}))]
        );
    }

    async fn run_phrase(exec: &Executor<MockActions>, phrase: &str) -> ExecOutcome {
        let grammar = starter_grammar().expect("starter grammar compiles");
        let matched = grammar
            .match_phrase(phrase)
            .unwrap_or_else(|| panic!("phrase {phrase:?} should match"));
        exec.execute(RouteOutcome::Command(matched), &FakeBackend::default())
            .await
            .expect("execute")
    }

    #[tokio::test]
    async fn launch_command_invokes_launch_with_target() {
        let exec = executor_with(vec![], &[]);
        let outcome = run_phrase(&exec, "open firefox").await;
        assert_eq!(outcome, ExecOutcome::Executed(Value::Null));
        assert_eq!(
            exec.actions.calls(),
            vec![("launch", "firefox".to_string())]
        );
    }

    #[tokio::test]
    async fn focus_command_invokes_focus_with_query() {
        let exec = executor_with(vec![], &[]);
        let outcome = run_phrase(&exec, "switch to the browser").await;
        assert_eq!(outcome, ExecOutcome::Executed(Value::Null));
        assert_eq!(
            exec.actions.calls(),
            vec![("focus_window", "browser".to_string())]
        );
    }

    #[tokio::test]
    async fn press_command_invokes_send_keys() {
        let exec = executor_with(vec![], &[]);
        let outcome = run_phrase(&exec, "press control shift p").await;
        assert_eq!(outcome, ExecOutcome::Executed(Value::Null));
        assert_eq!(
            exec.actions.calls(),
            vec![("send_keys", "control shift p".to_string())]
        );
    }

    #[tokio::test]
    async fn paste_command_sends_paste_chord() {
        for phrase in ["paste", "paste clipboard", "paste from my clipboard"] {
            let exec = executor_with(vec![], &[]);
            let outcome = run_phrase(&exec, phrase).await;
            assert_eq!(outcome, ExecOutcome::Executed(Value::Null));
            assert_eq!(
                exec.actions.calls(),
                vec![("send_keys", PASTE_KEYS.to_string())],
                "phrase {phrase:?}"
            );
        }
    }

    #[test]
    fn open_file_pattern_captures_the_spoken_query() {
        let grammar = starter_grammar().expect("starter grammar compiles");
        for phrase in [
            "open the user controller test file",
            "go to the user controller test file",
            "open user controller test file",
        ] {
            let matched = grammar
                .match_phrase(phrase)
                .unwrap_or_else(|| panic!("phrase {phrase:?} should match"));
            assert_eq!(matched.command_id, CMD_OPEN_FILE, "phrase {phrase:?}");
            assert_eq!(
                matched.slots.get("query"),
                Some(&SlotValue::Text("user controller test".into()))
            );
        }
    }

    #[test]
    fn open_file_pattern_does_not_shadow_launch_or_focus() {
        let grammar = starter_grammar().expect("starter grammar compiles");
        // No trailing "file": these stay app launch / window focus.
        assert_eq!(
            grammar.match_phrase("open firefox").unwrap().command_id,
            CMD_LAUNCH
        );
        assert_eq!(
            grammar
                .match_phrase("go to the browser")
                .unwrap()
                .command_id,
            CMD_FOCUS
        );
    }

    #[tokio::test]
    async fn open_file_reaching_the_executor_is_no_action() {
        // run_command resolves open_file before the executor; the executor
        // deliberately maps it to nothing rather than guessing an action.
        let exec = executor_with(vec![], &[]);
        let outcome = run_phrase(&exec, "open the readme file").await;
        assert_eq!(outcome, ExecOutcome::NoAction);
        assert!(exec.actions.calls().is_empty());
    }

    #[tokio::test]
    async fn unmapped_command_id_is_no_action() {
        let exec = executor_with(vec![], &[]);
        let matched = Match {
            command_id: "native.unknown".into(),
            slots: HashMap::new(),
        };
        let outcome = exec
            .execute(RouteOutcome::Command(matched), &FakeBackend::default())
            .await
            .expect("execute");
        assert_eq!(outcome, ExecOutcome::NoAction);
        assert!(exec.actions.calls().is_empty());
    }

    #[tokio::test]
    async fn intent_and_no_match_yield_no_action() {
        let exec = executor_with(vec![], &[]);
        let backend = FakeBackend::default();
        let intent = RouteOutcome::Intent(IntentMatch {
            intent_id: "volume_down".into(),
            similarity: 0.94,
        });
        assert_eq!(
            exec.execute(intent, &backend).await.expect("execute"),
            ExecOutcome::NoAction
        );
        assert_eq!(
            exec.execute(RouteOutcome::NoMatch, &backend)
                .await
                .expect("execute"),
            ExecOutcome::NoAction
        );
        assert!(backend.calls().is_empty());
        assert!(exec.actions.calls().is_empty());
    }
}
