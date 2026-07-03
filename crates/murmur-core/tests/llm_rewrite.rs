//! End-to-end test for the LLM rewrite layer. Ignored by default: it needs
//! the roughly 1 GB Qwen3 GGUF (downloaded on first run) and does real CPU
//! inference. Run with:
//! `cargo test -p murmur-core --features llm -- --ignored`
#![cfg(feature = "llm")]

use murmur_core::llm::{self, LlmEngine, RewriteMode};

const INPUT: &str = "so um i think we should like ship the thing on friday and then tell the team";

#[tokio::test]
#[ignore = "downloads a ~1 GB model and runs CPU inference"]
async fn rewrite_transforms_sample_dictation() {
    let path = if llm::is_downloaded() {
        llm::model_path().expect("model path")
    } else {
        llm::download().await.expect("model download")
    };
    let engine = LlmEngine::load(&path).expect("engine load");

    for mode in [
        RewriteMode::CleanUp,
        RewriteMode::Formal,
        RewriteMode::BulletList,
    ] {
        let output = llm::rewrite(&engine, INPUT, mode, 256).expect("rewrite");
        println!("mode: {mode:?}\nbefore: {INPUT}\nafter:  {output}\n");
        assert!(!output.trim().is_empty(), "empty output for {mode:?}");
        if mode == RewriteMode::BulletList {
            assert!(
                output.contains('\n') || output.starts_with("- ") || output.contains('\u{2022}'),
                "expected bullet formatting, got: {output}"
            );
        }
    }
}
