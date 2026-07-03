use serde_json::Value;

/// Intrinsic danger of a tool, independent of the user's saved policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RiskTier {
    /// Observes state only (list files, read status).
    ReadOnly,
    /// Changes state but is reversible (rename, move, edit).
    Mutating,
    /// Irreversible or high impact (delete, send, deploy, push).
    Destructive,
}

/// A voice-invokable action: name, human description, JSON Schema for its
/// arguments, and intrinsic risk tier. Mirrors the MCP tool shape so MCP
/// server tools map onto it directly in later phases.
#[derive(Debug, Clone, PartialEq)]
pub struct Tool {
    pub name: String,
    pub description: String,
    /// JSON Schema describing the tool's arguments.
    pub input_schema: Value,
    pub risk: RiskTier,
}
