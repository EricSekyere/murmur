//! Voice-to-action router (docs/command-mode-design.md, Section 4): Tier 1
//! deterministic grammar first, then Tier 2 embedding intent classification
//! (optional), then Tier 3 grammar-constrained LLM tool selection.

use serde_json::Value;

use super::grammar::Match;
use super::intent::IntentMatch;

#[cfg(any(test, feature = "llm"))]
use super::{grammar::Grammar, intent::IntentClassifier, tool::Tool};
#[cfg(feature = "llm")]
use crate::llm::{LlmEngine, LlmError};

/// Where an utterance landed after routing.
#[derive(Debug, Clone, PartialEq)]
pub enum RouteOutcome {
    /// A Tier 1 grammar pattern matched; run the mapped command.
    Command(Match),
    /// The Tier 2 embedding classifier matched a registered intent.
    Intent(IntentMatch),
    /// The Tier 3 LLM picked an allowlisted tool with JSON arguments.
    ToolCall { tool: String, arguments: Value },
    /// No tier claimed the utterance.
    NoMatch,
}

/// Fixed rules of the tool-call grammar; `tool-name` is generated per call.
/// The JSON rules follow llama.cpp's json.gbnf, with whitespace capped at one
/// character so a stalling model cannot pad the token budget. GBNF rule names
/// only allow `[a-zA-Z0-9-]`, hence the dashed names.
const TOOL_CALL_RULES: &str = r#"root ::= "{" ws "\"tool\"" ws ":" ws tool-name ws "," ws "\"arguments\"" ws ":" ws object ws "}"
object ::= "{" ws (pair (ws "," ws pair)*)? ws "}"
pair ::= string ws ":" ws value
value ::= object | array | string | number | "true" | "false" | "null"
array ::= "[" ws (value (ws "," ws value)*)? ws "]"
string ::= "\"" char* "\""
char ::= [^"\\\x7F\x00-\x1F] | "\\" (["\\bfnrt] | "u" [0-9a-fA-F] [0-9a-fA-F] [0-9a-fA-F] [0-9a-fA-F])
number ::= "-"? ("0" | [1-9] [0-9]*) ("." [0-9]+)? ([eE] [-+]? [0-9]+)?
ws ::= [ \t\n]?
"#;

/// Build a GBNF grammar constraining output to exactly
/// `{"tool": <name>, "arguments": <object>}` where `<name>` is one of
/// `tool_names` and `<object>` is any syntactically valid JSON object.
///
/// The constraint guarantees syntax, never semantics: tool allowlisting and
/// risk-tier confirmation stay the backstop for the model choosing badly.
/// An empty `tool_names` yields a grammar the sampler rejects at build time,
/// so guard the call as [`route`] does.
pub fn tool_call_grammar(tool_names: &[&str]) -> String {
    let names = tool_names
        .iter()
        .map(|name| gbnf_json_string(name))
        .collect::<Vec<_>>()
        .join(" | ");
    format!("tool-name ::= {names}\n{TOOL_CALL_RULES}")
}

/// Render `name` as a GBNF literal matching the JSON string `"name"`: first
/// JSON-encode the name, then GBNF-escape that text (two escape layers).
fn gbnf_json_string(name: &str) -> String {
    let json_text = format!("\"{}\"", name.replace('\\', "\\\\").replace('"', "\\\""));
    format!(
        "\"{}\"",
        json_text.replace('\\', "\\\\").replace('"', "\\\"")
    )
}

/// Output budget for one tool call: a name plus a small argument object.
#[cfg(feature = "llm")]
const TOOL_CALL_MAX_TOKENS: usize = 128;

/// Route `utterance` through the tiers: Tier 1 grammar match first; on a
/// miss, the optional Tier 2 embedding classifier; on a miss there too,
/// Tier 3 grammar-constrained tool selection over `tools`. With `tier2`
/// absent the utterance falls straight from Tier 1 to Tier 3, and with no
/// tools registered the LLM is never invoked.
#[cfg(feature = "llm")]
pub fn route(
    grammar: &Grammar,
    tier2: Option<&IntentClassifier>,
    tools: &[Tool],
    engine: &LlmEngine,
    utterance: &str,
) -> Result<RouteOutcome, LlmError> {
    route_with(grammar, tier2, tools, utterance, |system, user, gbnf| {
        engine.generate_constrained(system, user, gbnf, TOOL_CALL_MAX_TOKENS)
    })
}

/// Core of [`route`] with generation injected (generic over its error), so
/// tier ordering is testable without a loaded model or the `llm` feature.
#[cfg(any(test, feature = "llm"))]
fn route_with<F, E>(
    grammar: &Grammar,
    tier2: Option<&IntentClassifier>,
    tools: &[Tool],
    utterance: &str,
    generate: F,
) -> Result<RouteOutcome, E>
where
    F: FnOnce(&str, &str, &str) -> Result<String, E>,
{
    if let Some(matched) = grammar.match_phrase(utterance) {
        return Ok(RouteOutcome::Command(matched));
    }
    if let Some(matched) = tier2.and_then(|classifier| classifier.classify(utterance)) {
        return Ok(RouteOutcome::Intent(matched));
    }
    if tools.is_empty() {
        return Ok(RouteOutcome::NoMatch);
    }
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    let gbnf = tool_call_grammar(&names);
    let raw = generate(&selection_prompt(tools), utterance, &gbnf)?;
    Ok(parse_tool_call(&raw, &names))
}

/// System prompt naming the allowlisted tools. The transcript is untrusted
/// content (design Section 5), so it rides in the user turn, never here.
#[cfg(any(test, feature = "llm"))]
fn selection_prompt(tools: &[Tool]) -> String {
    use std::fmt::Write;

    let mut prompt = String::from(
        "Select the tool that fulfils the user's spoken request and reply with \
         a single JSON object {\"tool\": ..., \"arguments\": ...}. Tools:",
    );
    for tool in tools {
        // Writing to a String cannot fail.
        let _ = write!(prompt, "\n- {}: {}", tool.name, tool.description);
    }
    prompt
}

#[cfg(any(test, feature = "llm"))]
#[derive(serde::Deserialize)]
struct RawToolCall {
    tool: String,
    arguments: Value,
}

/// Parse constrained output into an outcome. The grammar makes malformed
/// JSON unlikely, but truncation at the token budget can still leave an
/// incomplete object; that and an off-allowlist name degrade to `NoMatch`.
#[cfg(any(test, feature = "llm"))]
fn parse_tool_call(raw: &str, allowed: &[&str]) -> RouteOutcome {
    match serde_json::from_str::<RawToolCall>(raw) {
        Ok(call) if allowed.contains(&call.tool.as_str()) => {
            tracing::debug!(tool = %call.tool, "tier 3 selected tool");
            RouteOutcome::ToolCall {
                tool: call.tool,
                arguments: call.arguments,
            }
        }
        Ok(call) => {
            tracing::warn!(tool = %call.tool, "tier 3 chose a tool outside the allowlist");
            RouteOutcome::NoMatch
        }
        Err(error) => {
            tracing::warn!(%error, "tier 3 output failed to parse");
            RouteOutcome::NoMatch
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_call_grammar_names_every_tool_and_core_rule() {
        let gbnf = tool_call_grammar(&["open_file", "set_volume"]);
        assert!(!gbnf.trim().is_empty());
        assert!(gbnf.contains(r#""\"open_file\"""#));
        assert!(gbnf.contains(r#""\"set_volume\"""#));
        for rule in [
            "root ::=",
            "tool-name ::=",
            "object ::=",
            "string ::=",
            "ws ::=",
        ] {
            assert!(gbnf.contains(rule), "missing rule {rule}");
        }
        // The tool name enum is a proper alternation.
        assert!(gbnf.contains(r#"tool-name ::= "\"open_file\"" | "\"set_volume\"""#));
    }

    #[test]
    fn tool_names_are_escaped_through_both_layers() {
        assert_eq!(gbnf_json_string("open_file"), r#""\"open_file\"""#);
        // A quote in the name: JSON layer makes `\"`, GBNF layer `\\\"`.
        assert_eq!(gbnf_json_string(r#"a"b"#), r#""\"a\\\"b\"""#);
        assert_eq!(gbnf_json_string(r"a\b"), r#""\"a\\\\b\"""#);
    }

    mod tiered {
        use super::super::super::grammar::Grammar;
        use super::super::super::intent::{IntentClassifier, IntentSet};
        use super::super::super::tool::{RiskTier, Tool};
        use super::*;

        /// Stand-in generation error so tier ordering tests need no LLM types.
        type GenError = String;

        fn sample_tools() -> Vec<Tool> {
            [
                ("open_file", "Open a file by path"),
                (
                    "set_volume",
                    "Set the system volume to a level from 0 to 100",
                ),
            ]
            .map(|(name, description)| Tool {
                name: name.into(),
                description: description.into(),
                input_schema: serde_json::json!({"type": "object"}),
                risk: RiskTier::Mutating,
            })
            .into()
        }

        fn volume_grammar() -> Grammar {
            let mut grammar = Grammar::new();
            grammar
                .add("set_volume", "set [the] volume to {level:0..100}")
                .expect("pattern compiles");
            grammar
        }

        /// Classifier whose lone intent matches only the exact phrase given.
        fn phrase_classifier(intent_id: &str, phrase: &str) -> IntentClassifier {
            let mut intents = IntentSet::new();
            intents.add(intent_id, phrase, vec![1.0, 0.0]);
            let phrase = phrase.to_string();
            IntentClassifier::new(
                move |text: &str| {
                    if text == phrase {
                        vec![1.0, 0.0]
                    } else {
                        vec![0.0, 1.0]
                    }
                },
                intents,
                0.9,
            )
        }

        #[test]
        fn tier1_hit_routes_to_command_without_classifier_or_model() {
            let mut intents = IntentSet::new();
            intents.add("volume_down", "turn it down", vec![1.0, 0.0]);
            let classifier = IntentClassifier::new(
                |_: &str| panic!("embedder must not be called on a tier 1 hit"),
                intents,
                0.5,
            );
            let outcome = route_with(
                &volume_grammar(),
                Some(&classifier),
                &sample_tools(),
                "set the volume to 40",
                |_, _, _| -> Result<String, GenError> {
                    panic!("model must not be called on a tier 1 hit")
                },
            )
            .expect("route");
            match outcome {
                RouteOutcome::Command(matched) => assert_eq!(matched.command_id, "set_volume"),
                other => panic!("expected Command, got {other:?}"),
            }
        }

        #[test]
        fn tier2_hit_routes_to_intent_without_the_model() {
            let classifier = phrase_classifier("volume_down", "make it a bit quieter");
            let outcome = route_with(
                &volume_grammar(),
                Some(&classifier),
                &sample_tools(),
                "make it a bit quieter",
                |_, _, _| -> Result<String, GenError> {
                    panic!("model must not be called on a tier 2 hit")
                },
            )
            .expect("route");
            match outcome {
                RouteOutcome::Intent(matched) => assert_eq!(matched.intent_id, "volume_down"),
                other => panic!("expected Intent, got {other:?}"),
            }
        }

        #[test]
        fn tier2_miss_falls_through_to_the_llm() {
            let classifier = phrase_classifier("volume_down", "make it a bit quieter");
            let outcome = route_with(
                &volume_grammar(),
                Some(&classifier),
                &sample_tools(),
                "open the readme",
                |_, _, _| {
                    Ok::<String, GenError>(
                        r#"{"tool": "open_file", "arguments": {"path": "README.md"}}"#.into(),
                    )
                },
            )
            .expect("route");
            assert_eq!(
                outcome,
                RouteOutcome::ToolCall {
                    tool: "open_file".into(),
                    arguments: serde_json::json!({"path": "README.md"}),
                }
            );
        }

        #[test]
        fn tier1_miss_without_tier2_falls_to_the_llm_and_parses_the_tool_call() {
            let outcome = route_with(
                &volume_grammar(),
                None,
                &sample_tools(),
                "make it a bit quieter",
                |system, user, gbnf| -> Result<String, GenError> {
                    assert!(system.contains("set_volume"));
                    assert_eq!(user, "make it a bit quieter");
                    assert!(gbnf.contains("root ::="));
                    Ok(r#"{"tool": "set_volume", "arguments": {"level": 20}}"#.into())
                },
            )
            .expect("route");
            assert_eq!(
                outcome,
                RouteOutcome::ToolCall {
                    tool: "set_volume".into(),
                    arguments: serde_json::json!({"level": 20}),
                }
            );
        }

        #[test]
        fn off_allowlist_tool_or_truncated_output_degrades_to_no_match() {
            let run = |raw: &'static str| {
                route_with(
                    &Grammar::new(),
                    None,
                    &sample_tools(),
                    "do something",
                    |_, _, _| Ok::<String, GenError>(raw.into()),
                )
                .expect("route")
            };
            assert_eq!(
                run(r#"{"tool": "delete_everything", "arguments": {}}"#),
                RouteOutcome::NoMatch
            );
            assert_eq!(
                run(r#"{"tool": "set_volume", "argu"#),
                RouteOutcome::NoMatch
            );
        }

        #[test]
        fn empty_tool_set_skips_the_model() {
            let outcome = route_with(
                &Grammar::new(),
                None,
                &[],
                "open the hatch",
                |_, _, _| -> Result<String, GenError> {
                    panic!("model must not be called with no tools")
                },
            )
            .expect("route");
            assert_eq!(outcome, RouteOutcome::NoMatch);
        }
    }
}
