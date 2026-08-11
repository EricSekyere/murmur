//! Integration tests for the MCP client-config installer, exercised through
//! [`murmur_mcp::install_at`] against fixture files so the user's real
//! editor configs are never touched.

use serde_json::{Value, json};

fn read_value(path: &std::path::Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).expect("read config")).expect("parse")
}

#[test]
fn install_creates_a_fresh_config_with_the_murmur_entry() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("mcp.json");
    murmur_mcp::install_at(&path, "/bin/murmur").expect("install");
    let value = read_value(&path);
    assert_eq!(value["mcpServers"]["murmur"]["command"], "/bin/murmur");
    assert_eq!(value["mcpServers"]["murmur"]["args"], json!(["mcp"]));
}

#[test]
fn install_preserves_foreign_servers_and_unknown_keys() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("mcp.json");
    std::fs::write(
        &path,
        json!({
            "theme": "dark",
            "mcpServers": {
                "github": {"command": "gh-mcp", "args": ["--stdio"], "env": {"TOKEN": "x"}}
            }
        })
        .to_string(),
    )
    .expect("seed");

    murmur_mcp::install_at(&path, "/bin/murmur").expect("install");
    let value = read_value(&path);
    assert_eq!(value["theme"], "dark");
    assert_eq!(value["mcpServers"]["github"]["command"], "gh-mcp");
    assert_eq!(value["mcpServers"]["github"]["env"]["TOKEN"], "x");
    assert_eq!(value["mcpServers"]["murmur"]["command"], "/bin/murmur");
}

#[test]
fn install_is_idempotent() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("mcp.json");
    murmur_mcp::install_at(&path, "/bin/murmur").expect("first install");
    let first = std::fs::read(&path).expect("read");
    murmur_mcp::install_at(&path, "/bin/murmur").expect("second install");
    let second = std::fs::read(&path).expect("read");
    assert_eq!(first, second, "re-running install must be byte-identical");
}

#[test]
fn install_updates_a_stale_murmur_entry_in_place() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("mcp.json");
    murmur_mcp::install_at(&path, "/old/location/murmur").expect("install");
    murmur_mcp::install_at(&path, "/new/location/murmur").expect("reinstall");
    let value = read_value(&path);
    assert_eq!(
        value["mcpServers"]["murmur"]["command"],
        "/new/location/murmur"
    );
}

#[test]
fn install_refuses_malformed_json_without_modifying_the_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("mcp.json");
    let original = "{ this is someone's broken config";
    std::fs::write(&path, original).expect("seed");

    let result = murmur_mcp::install_at(&path, "/bin/murmur");
    assert!(result.is_err(), "unparseable config must be an error");
    assert_eq!(
        std::fs::read_to_string(&path).expect("read"),
        original,
        "a config we cannot merge must be left byte-for-byte untouched"
    );
}

#[test]
fn install_refuses_a_config_with_the_wrong_shape() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("mcp.json");
    let original = json!({"mcpServers": ["not", "an", "object"]}).to_string();
    std::fs::write(&path, &original).expect("seed");

    assert!(murmur_mcp::install_at(&path, "/bin/murmur").is_err());
    assert_eq!(std::fs::read_to_string(&path).expect("read"), original);
}

#[test]
fn install_treats_an_empty_file_as_a_fresh_config() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("mcp.json");
    std::fs::write(&path, "  \n").expect("seed");
    murmur_mcp::install_at(&path, "/bin/murmur").expect("install");
    assert_eq!(
        read_value(&path)["mcpServers"]["murmur"]["command"],
        "/bin/murmur"
    );
}
