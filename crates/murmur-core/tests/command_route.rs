//! End-to-end test for Tier 3 grammar-constrained tool selection. Ignored by
//! default: it needs the roughly 1 GB Qwen3 GGUF (downloaded on first run)
//! and real CPU inference. Run with:
//! `cargo test -p murmur-core --features llm --test command_route -- --ignored`
#![cfg(feature = "llm")]

use murmur_core::command::tool_call_grammar;
use murmur_core::llm;

#[tokio::test]
#[ignore = "downloads a ~1 GB model and runs CPU inference"]
async fn constrained_generation_yields_a_parseable_tool_call() {
    let path = if llm::is_downloaded() {
        llm::model_path().expect("model path")
    } else {
        llm::download().await.expect("model download")
    };
    let engine = llm::LlmEngine::load(&path).expect("engine load");

    let names = ["open_file", "set_volume"];
    let gbnf = tool_call_grammar(&names);
    let system = "Select the tool that fulfils the user's spoken request and reply \
                  with a single JSON object {\"tool\": ..., \"arguments\": ...}. Tools:\n\
                  - open_file: Open a file by path\n\
                  - set_volume: Set the system volume to a level from 0 to 100";

    // Deliberately one short generation: this runs on CPU.
    let output = engine
        .generate_constrained(system, "set the volume to forty", &gbnf, 64)
        .expect("constrained generation");
    println!("constrained output: {output}");

    let value: serde_json::Value =
        serde_json::from_str(&output).expect("constrained output must parse as JSON");
    let tool = value["tool"].as_str().expect("tool field must be a string");
    assert!(names.contains(&tool), "unexpected tool {tool:?}");
    assert!(
        value["arguments"].is_object(),
        "arguments must be a JSON object"
    );
}
