use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::error::CommandError;
use super::tool::{RiskTier, Tool};

/// The user's saved policy for a tool. A tool with no saved policy defaults
/// to [`Permission::Ask`], so a newly discovered tool can never run without
/// the user seeing it first.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    Deny,
    #[default]
    Ask,
    Allow,
}

/// What the executor should do for a given permission and risk tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Never run; the user denied this tool.
    Blocked,
    /// Require explicit confirmation (keyboard or mouse, never voice) that
    /// echoes the parsed arguments back to the user.
    Confirm,
    /// Run without asking, then offer undo.
    AutoRunReversible,
    /// Run without asking.
    AutoRun,
}

/// Map a saved permission and a tool's risk tier to an execution decision.
///
/// Safety invariant: a Destructive tool always yields [`Decision::Confirm`],
/// even under [`Permission::Allow`]. Audio is an untrusted input channel, so
/// voice must never be able to auto-run a destructive action.
pub fn decide(perm: Permission, risk: RiskTier) -> Decision {
    match perm {
        Permission::Deny => Decision::Blocked,
        Permission::Ask => Decision::Confirm,
        Permission::Allow => match risk {
            RiskTier::ReadOnly => Decision::AutoRun,
            RiskTier::Mutating => Decision::AutoRunReversible,
            RiskTier::Destructive => Decision::Confirm,
        },
    }
}

const PERMISSIONS_FILE: &str = "command_permissions.toml";

/// Per-tool permission policy, persisted as TOML in the murmur config dir.
/// This is where "always allow" and "always deny" choices live.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PermissionStore {
    /// Saved policy keyed by tool name; absent tools default to `Ask`.
    #[serde(default)]
    permissions: HashMap<String, Permission>,
}

impl PermissionStore {
    /// The default store file path: `<config_dir>/murmur/command_permissions.toml`.
    pub fn path() -> Result<PathBuf, CommandError> {
        let dir = dirs::config_dir().ok_or(CommandError::ConfigDirUnavailable)?;
        Ok(dir.join("murmur").join(PERMISSIONS_FILE))
    }

    /// The saved permission for a tool name, defaulting to `Ask` when unset.
    pub fn get(&self, name: &str) -> Permission {
        self.permissions.get(name).copied().unwrap_or_default()
    }

    /// Save a policy for a tool name (how "always allow" is persisted).
    pub fn set(&mut self, name: impl Into<String>, permission: Permission) {
        self.permissions.insert(name.into(), permission);
    }

    /// The execution decision for a tool: its saved permission combined with
    /// its intrinsic risk tier via [`decide`].
    pub fn decision_for(&self, tool: &Tool) -> Decision {
        decide(self.get(&tool.name), tool.risk)
    }

    /// Load the store from the default path. Any failure (missing config
    /// dir, unreadable or corrupt file) recovers to defaults: the default
    /// policy is `Ask` for everything, which is the safe fallback.
    pub fn load() -> Self {
        match Self::path() {
            Ok(path) => Self::load_from(&path),
            Err(e) => {
                tracing::warn!(error = %e, "cannot resolve permission store path, using defaults");
                Self::default()
            }
        }
    }

    /// Load the store from `path`, recovering to defaults instead of erroring.
    /// A corrupt file is backed up so a hand-edited policy is not lost.
    pub fn load_from(path: &Path) -> Self {
        if !path.exists() {
            tracing::debug!(path = %path.display(), "no permission store file, using defaults");
            return Self::default();
        }
        match Self::read(path) {
            Ok(store) => store,
            Err(e) => {
                let backup = path.with_extension("toml.bak");
                tracing::warn!(
                    path = %path.display(),
                    backup = %backup.display(),
                    error = %e,
                    "permission store is unreadable or invalid, backing it up and using defaults"
                );
                // Safe to drop: recovery to defaults proceeds whether or not
                // the backup rename succeeds.
                let _ = std::fs::rename(path, &backup);
                Self::default()
            }
        }
    }

    fn read(path: &Path) -> Result<Self, CommandError> {
        let content = std::fs::read_to_string(path)?;
        Ok(toml::from_str(&content)?)
    }

    /// Save the store to the default path.
    pub fn save(&self) -> Result<(), CommandError> {
        self.save_to(&Self::path()?)
    }

    /// Save the store to `path` atomically: write a sibling tempfile, then
    /// rename, so a crash mid-write cannot corrupt the saved policy.
    pub fn save_to(&self, path: &Path) -> Result<(), CommandError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(self)?;
        let tmp_path = path.with_extension("toml.tmp");
        std::fs::write(&tmp_path, &content)?;
        std::fs::rename(&tmp_path, path)?;
        tracing::debug!(path = %path.display(), "saved command permissions");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tool(name: &str, risk: RiskTier) -> Tool {
        Tool {
            name: name.to_string(),
            description: format!("{name} test tool"),
            input_schema: json!({"type": "object", "properties": {}}),
            risk,
        }
    }

    #[test]
    fn decide_covers_full_truth_table() {
        use Decision::*;
        use Permission::*;
        use RiskTier::*;

        assert_eq!(decide(Deny, ReadOnly), Blocked);
        assert_eq!(decide(Deny, Mutating), Blocked);
        assert_eq!(decide(Deny, Destructive), Blocked);

        assert_eq!(decide(Ask, ReadOnly), Confirm);
        assert_eq!(decide(Ask, Mutating), Confirm);
        assert_eq!(decide(Ask, Destructive), Confirm);

        assert_eq!(decide(Allow, ReadOnly), AutoRun);
        assert_eq!(decide(Allow, Mutating), AutoRunReversible);
        assert_eq!(decide(Allow, Destructive), Confirm);
    }

    #[test]
    fn allow_destructive_never_auto_runs() {
        // The core safety invariant: voice must never auto-run a destructive
        // action, even when the user saved "always allow" for the tool.
        assert_eq!(
            decide(Permission::Allow, RiskTier::Destructive),
            Decision::Confirm
        );
    }

    #[test]
    fn unknown_tool_defaults_to_ask() {
        let store = PermissionStore::default();
        assert_eq!(store.get("never_seen"), Permission::Ask);
        assert_eq!(
            store.decision_for(&tool("never_seen", RiskTier::ReadOnly)),
            Decision::Confirm
        );
    }

    #[test]
    fn set_then_get_round_trips() {
        let mut store = PermissionStore::default();
        store.set("git_status", Permission::Allow);
        store.set("delete_file", Permission::Deny);
        assert_eq!(store.get("git_status"), Permission::Allow);
        assert_eq!(store.get("delete_file"), Permission::Deny);
        assert_eq!(store.get("something_else"), Permission::Ask);
    }

    #[test]
    fn destructive_tool_confirms_even_with_explicit_allow() {
        let mut store = PermissionStore::default();
        store.set("delete_branch", Permission::Allow);
        assert_eq!(
            store.decision_for(&tool("delete_branch", RiskTier::Destructive)),
            Decision::Confirm
        );
    }

    #[test]
    fn decision_for_combines_saved_permission_and_risk() {
        let mut store = PermissionStore::default();
        store.set("list_files", Permission::Allow);
        store.set("rename_file", Permission::Allow);
        store.set("read_status", Permission::Deny);
        assert_eq!(
            store.decision_for(&tool("list_files", RiskTier::ReadOnly)),
            Decision::AutoRun
        );
        assert_eq!(
            store.decision_for(&tool("rename_file", RiskTier::Mutating)),
            Decision::AutoRunReversible
        );
        assert_eq!(
            store.decision_for(&tool("read_status", RiskTier::ReadOnly)),
            Decision::Blocked
        );
    }

    #[test]
    fn save_then_load_round_trips_through_toml() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("command_permissions.toml");

        let mut store = PermissionStore::default();
        store.set("git_status", Permission::Allow);
        store.set("delete_branch", Permission::Deny);
        store.set("open_file", Permission::Ask);
        store.save_to(&path).expect("save");

        let reloaded = PermissionStore::load_from(&path);
        assert_eq!(reloaded, store);
    }

    #[test]
    fn missing_file_loads_defaults() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("command_permissions.toml");
        assert_eq!(
            PermissionStore::load_from(&path),
            PermissionStore::default()
        );
    }

    #[test]
    fn corrupt_toml_recovers_to_defaults_and_backs_up() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("command_permissions.toml");
        std::fs::write(&path, "this is { not [[[ toml").expect("write garbage");

        let store = PermissionStore::load_from(&path);
        assert_eq!(store, PermissionStore::default());
        assert_eq!(store.get("anything"), Permission::Ask);
        assert!(path.with_extension("toml.bak").exists());
    }

    #[test]
    fn invalid_utf8_recovers_to_defaults() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("command_permissions.toml");
        std::fs::write(&path, [0x00u8, 0xff, 0xfe, 0x9c]).expect("write bytes");

        let store = PermissionStore::load_from(&path);
        assert_eq!(store, PermissionStore::default());
    }

    #[test]
    fn unknown_keys_still_load() {
        // Forward compat: a store written by a newer version with extra keys
        // must still load (no deny_unknown_fields).
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("command_permissions.toml");
        let content = concat!(
            "future_top_level = \"ignored\"\n",
            "\n",
            "[permissions]\n",
            "git_status = \"allow\"\n",
            "\n",
            "[future_section]\n",
            "nested = 1\n",
        );
        std::fs::write(&path, content).expect("write");

        let store = PermissionStore::load_from(&path);
        assert_eq!(store.get("git_status"), Permission::Allow);
    }
}
