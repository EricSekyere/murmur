//! Mixed static and parameterized snippets loaded through `Settings`
//! deserialization and the snippet compiler: old configs keep working
//! unchanged, and a corrupt snippet degrades to a warning, never an error.

use murmur_core::config::Settings;
use murmur_core::snippets;

const MIXED_CONFIG: &str = r#"
[[snippets]]
trigger = "my email"
expansion = "user@example.com"

[[snippets]]
trigger = "create react component {name}"
expansion = "export function {name:pascal}() {{\\n  return null;\\n}}"

[[snippets]]
trigger = "broken {"
expansion = "never fires"

[[snippets]]
trigger = "sign off"
expansion = "Best regards,"
"#;

fn load(toml: &str) -> Settings {
    toml::from_str(toml).expect("config should deserialize")
}

#[test]
fn old_static_config_round_trips_unchanged() {
    let settings = load(MIXED_CONFIG);
    assert_eq!(settings.snippets.len(), 4);
    // The Snippet TOML shape is untouched: triggers and expansions survive
    // deserialization byte for byte.
    assert_eq!(settings.snippets[0].trigger, "my email");
    assert_eq!(settings.snippets[0].expansion, "user@example.com");

    let serialized = toml::to_string(&settings).expect("settings should serialize");
    let reloaded: Settings = toml::from_str(&serialized).expect("round trip");
    assert_eq!(settings.snippets, reloaded.snippets);
}

#[test]
fn mixed_config_compiles_with_statics_intact() {
    let settings = load(MIXED_CONFIG);
    let (compiled, _warnings) = snippets::compile(&settings.snippets);

    // Statics behave exactly as before.
    assert_eq!(
        compiled.expand("my email"),
        Some("user@example.com".to_string())
    );
    assert_eq!(
        compiled.expand("sign off"),
        Some("Best regards,".to_string())
    );

    // The parameterized snippet captures and recases (the TOML literal
    // "\\n" reaches the compiler as backslash-n and becomes a newline).
    assert_eq!(
        compiled.expand("create react component user profile"),
        Some("export function UserProfile() {\n  return null;\n}".to_string())
    );
}

#[test]
fn corrupt_snippet_warns_instead_of_erroring() {
    let settings = load(MIXED_CONFIG);
    let (compiled, warnings) = snippets::compile(&settings.snippets);
    assert!(
        warnings.iter().any(|w| w.contains("broken {")),
        "expected a warning for the malformed trigger, got {warnings:?}"
    );
    // The corrupt entry is inert; everything around it still works.
    assert_eq!(compiled.expand("broken"), None);
    assert_eq!(
        compiled.expand("my email"),
        Some("user@example.com".to_string())
    );
}
