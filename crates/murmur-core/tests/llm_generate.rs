//! End-to-end test for the local LLM runtime. Ignored by default: it
//! downloads a roughly 1 GB GGUF model on first run and does real CPU
//! inference. Run with:
//! `cargo test -p murmur-core --features llm -- --ignored`
#![cfg(feature = "llm")]

use murmur_core::llm;

#[tokio::test]
#[ignore = "downloads a ~1 GB model and runs CPU inference"]
async fn qwen3_downloads_loads_and_generates() {
    let path = if llm::is_downloaded() {
        llm::model_path().expect("model path")
    } else {
        llm::download().await.expect("model download")
    };

    let engine = llm::LlmEngine::load(&path).expect("engine load");
    let output = engine
        .generate(
            "Rewrite this cleanly: \"i went to teh store and buyed milk\"",
            128,
        )
        .expect("generation");

    println!("generated text: {output}");
    assert!(!output.trim().is_empty(), "expected non-empty output");
}
