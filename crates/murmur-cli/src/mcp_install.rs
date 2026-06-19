//! `murmur mcp install`: write Murmur's MCP server into an MCP client's config
//! so setup is one command instead of a hand-edited JSON file. The server entry
//! points at this binary's own resolved path (via `current_exe`), so it works
//! regardless of `PATH`. The merge is idempotent and non-destructive: existing
//! servers and keys are preserved, and a config that isn't valid JSON is left
//! untouched rather than clobbered.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use serde_json::{Value, json};

/// MCP client applications whose config Murmur can write.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum ClientKind {
    /// Cursor (`~/.cursor/mcp.json`).
    Cursor,
    /// Claude Desktop (`<config>/Claude/claude_desktop_config.json`).
    ClaudeDesktop,
}

const SERVER_NAME: &str = "murmur";
const ALL: &[ClientKind] = &[ClientKind::Cursor, ClientKind::ClaudeDesktop];

impl ClientKind {
    fn label(self) -> &'static str {
        match self {
            ClientKind::Cursor => "Cursor",
            ClientKind::ClaudeDesktop => "Claude Desktop",
        }
    }

    /// `--client` value used to force this one client.
    fn flag_value(self) -> &'static str {
        match self {
            ClientKind::Cursor => "cursor",
            ClientKind::ClaudeDesktop => "claude-desktop",
        }
    }

    fn config_path(self) -> Option<PathBuf> {
        match self {
            ClientKind::Cursor => dirs::home_dir().map(|h| h.join(".cursor").join("mcp.json")),
            // config_dir is %APPDATA% (Windows), ~/Library/Application Support
            // (macOS), and ~/.config (Linux) — the right base on each OS.
            ClientKind::ClaudeDesktop => {
                dirs::config_dir().map(|c| c.join("Claude").join("claude_desktop_config.json"))
            }
        }
    }
}

/// Configure `only` if given, otherwise every detected client. Prints a short,
/// actionable report plus the Claude Code one-liner.
pub fn install(only: Option<ClientKind>) -> Result<()> {
    let exe = std::env::current_exe()
        .context("could not determine the murmur executable path")?
        .to_string_lossy()
        .into_owned();

    let mut configured = 0usize;
    match only {
        // Explicit request: write the config even if the client isn't detected.
        Some(client) => {
            write_client(client, &exe)?;
            configured += 1;
        }
        None => {
            for &client in ALL {
                match client.config_path() {
                    Some(path) if detected(&path) => {
                        write_client(client, &exe)?;
                        configured += 1;
                    }
                    _ => println!(
                        "- {} not detected, skipped (use `--client {}` to configure it anyway).",
                        client.label(),
                        client.flag_value(),
                    ),
                }
            }
        }
    }

    if configured == 0 {
        println!("No MCP clients were configured.");
    } else {
        println!();
        println!("Restart the client (or toggle the server in its MCP settings) to load it.");
        println!(
            "Murmur then exposes your transcription history as the get_recent_transcripts and \
             search_transcripts tools."
        );
    }

    println!();
    println!("For Claude Code, run:");
    println!("  claude mcp add {SERVER_NAME} -- \"{exe}\" mcp");
    Ok(())
}

fn write_client(client: ClientKind, exe: &str) -> Result<()> {
    let path = client
        .config_path()
        .ok_or_else(|| anyhow!("could not resolve a config path for {}", client.label()))?;
    let merged = upsert_server(read_json(&path)?, SERVER_NAME, exe)
        .with_context(|| format!("{} has an unexpected shape", path.display()))?;
    write_atomic(&path, &merged).with_context(|| format!("writing {}", path.display()))?;
    println!("- Configured {}: {}", client.label(), path.display());
    Ok(())
}

/// A client is "detected" if its config file or its parent dir already exists,
/// so we don't create config trees for apps that aren't installed.
fn detected(path: &Path) -> bool {
    path.exists() || path.parent().is_some_and(Path::exists)
}

/// Read an existing JSON config, or `{}` when absent/empty. A present-but-invalid
/// file is an error so we never clobber something we can't safely merge.
fn read_json(path: &Path) -> Result<Value> {
    match std::fs::read_to_string(path) {
        Ok(s) if !s.trim().is_empty() => serde_json::from_str(&s)
            .with_context(|| format!("{} is not valid JSON; leaving it untouched", path.display())),
        Ok(_) => Ok(json!({})),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(json!({})),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// Insert or replace the `murmur` entry under `mcpServers`, preserving every
/// other key and server. Errors if the root or `mcpServers` isn't an object.
fn upsert_server(mut root: Value, name: &str, exe: &str) -> Result<Value> {
    if root.is_null() {
        root = json!({});
    }
    let obj = root
        .as_object_mut()
        .ok_or_else(|| anyhow!("config root is not a JSON object"))?;
    let servers = obj.entry("mcpServers").or_insert_with(|| json!({}));
    let servers = servers
        .as_object_mut()
        .ok_or_else(|| anyhow!("\"mcpServers\" is not a JSON object"))?;
    servers.insert(name.to_string(), json!({ "command": exe, "args": ["mcp"] }));
    Ok(root)
}

fn write_atomic(path: &Path, value: &Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let body = serde_json::to_string_pretty(value)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, body.as_bytes())?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_into_empty_creates_server() {
        let out = upsert_server(json!({}), "murmur", "/bin/murmur").unwrap();
        assert_eq!(out["mcpServers"]["murmur"]["command"], "/bin/murmur");
        assert_eq!(out["mcpServers"]["murmur"]["args"][0], "mcp");
    }

    #[test]
    fn upsert_preserves_other_servers_and_keys() {
        let existing = json!({
            "theme": "dark",
            "mcpServers": { "other": { "command": "x" } }
        });
        let out = upsert_server(existing, "murmur", "/bin/murmur").unwrap();
        assert_eq!(out["theme"], "dark");
        assert_eq!(out["mcpServers"]["other"]["command"], "x");
        assert_eq!(out["mcpServers"]["murmur"]["command"], "/bin/murmur");
    }

    #[test]
    fn upsert_overwrites_existing_murmur_entry() {
        let existing = json!({ "mcpServers": { "murmur": { "command": "old", "args": [] } } });
        let out = upsert_server(existing, "murmur", "/new/murmur").unwrap();
        assert_eq!(out["mcpServers"]["murmur"]["command"], "/new/murmur");
        assert_eq!(out["mcpServers"]["murmur"]["args"][0], "mcp");
    }

    #[test]
    fn upsert_rejects_non_object_root() {
        assert!(upsert_server(json!([1, 2, 3]), "murmur", "x").is_err());
    }

    #[test]
    fn upsert_rejects_non_object_servers() {
        assert!(upsert_server(json!({ "mcpServers": "nope" }), "murmur", "x").is_err());
    }
}
