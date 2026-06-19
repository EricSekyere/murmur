//! Tree-sitter symbol extraction: parse a source file and collect its real
//! identifiers (function/type/field/variable names), skipping comments and
//! string literals. A precision upgrade over the regex scan in `extract.rs` —
//! no tokens from comments or strings, and no language keywords (those are
//! distinct node kinds, not `identifier` nodes).
//!
//! Languages without a bundled grammar return `None` so the caller falls back
//! to the lexical scan.

use tree_sitter::{Language, Parser};

/// Map a file extension (lowercase, no dot) to its grammar.
fn language_for(ext: &str) -> Option<Language> {
    let lang = match ext {
        "rs" => tree_sitter_rust::LANGUAGE.into(),
        "py" => tree_sitter_python::LANGUAGE.into(),
        "js" | "jsx" => tree_sitter_javascript::LANGUAGE.into(),
        "ts" => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        "tsx" => tree_sitter_typescript::LANGUAGE_TSX.into(),
        "go" => tree_sitter_go::LANGUAGE.into(),
        "java" => tree_sitter_java::LANGUAGE.into(),
        _ => return None,
    };
    Some(lang)
}

/// Whether tree-sitter can parse this extension.
pub(crate) fn supports(ext: &str) -> bool {
    language_for(ext).is_some()
}

/// Extract identifier-like symbol names from `source`, skipping comments and
/// strings. Returns `None` for unsupported extensions or on a parser failure.
pub(crate) fn extract_symbols(source: &str, ext: &str) -> Option<Vec<String>> {
    let language = language_for(ext)?;
    let mut parser = Parser::new();
    parser.set_language(&language).ok()?;
    let tree = parser.parse(source, None)?;

    let bytes = source.as_bytes();
    let mut out = Vec::new();
    let mut stack = vec![tree.root_node()];
    while let Some(node) = stack.pop() {
        let kind = node.kind();
        // Don't descend into comments or strings: their text isn't code.
        if is_skip(kind) {
            continue;
        }
        if node.child_count() == 0 {
            if is_identifier(kind)
                && let Ok(text) = std::str::from_utf8(&bytes[node.byte_range()])
            {
                out.push(text.to_string());
            }
        } else {
            for i in 0..node.child_count() {
                if let Some(child) = node.child(i) {
                    stack.push(child);
                }
            }
        }
    }
    Some(out)
}

/// Comment and string node kinds, whose contents are not code symbols.
fn is_skip(kind: &str) -> bool {
    kind.contains("comment") || kind.contains("string") || kind.starts_with("char")
}

/// Leaf node kinds that name a symbol across the supported grammars
/// (`identifier`, `type_identifier`, `field_identifier`, `constant`, ...).
fn is_identifier(kind: &str) -> bool {
    kind == "identifier" || kind.ends_with("_identifier") || kind == "constant"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_symbols_and_skips_comments_and_strings() {
        let src = r#"
// commentSymbolShouldDrop
fn computeWidgetTotal() {
    let userIdentifier = "stringSymbolShouldDrop";
    callHelper(userIdentifier);
}
"#;
        let syms = extract_symbols(src, "rs").expect("rust supported");
        assert!(syms.iter().any(|s| s == "computeWidgetTotal"));
        assert!(syms.iter().any(|s| s == "userIdentifier"));
        assert!(syms.iter().any(|s| s == "callHelper"));
        assert!(
            !syms.iter().any(|s| s == "commentSymbolShouldDrop"),
            "comment text must be skipped"
        );
        assert!(
            !syms.iter().any(|s| s == "stringSymbolShouldDrop"),
            "string contents must be skipped"
        );
    }

    #[test]
    fn unsupported_extension_returns_none() {
        assert!(extract_symbols("anything", "txt").is_none());
        assert!(!supports("md"));
        assert!(supports("rs"));
        assert!(supports("tsx"));
    }
}
