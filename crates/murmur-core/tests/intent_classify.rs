//! Real-embedder smoke test for the Tier 2 intent classifier. Ignored by
//! default: it needs the downloaded bge-small model and the ORT runtime.
//! Run with:
//! `cargo test -p murmur-core --features help --test intent_classify -- --ignored --nocapture`
#![cfg(feature = "help")]

use murmur_core::command::{IntentSet, classify};
use murmur_core::help::{Embedder, OnnxEmbedder};

#[test]
#[ignore = "needs the downloaded bge-small model and the ORT runtime"]
fn paraphrase_classifies_to_the_right_intent() {
    let embedder = OnnxEmbedder::load().expect("load embedder (model must be downloaded)");
    let intents = IntentSet::embed_examples(
        &embedder,
        &[
            ("volume_down", "turn the volume down"),
            ("volume_down", "lower the sound"),
            ("open_browser", "open the browser"),
            ("open_browser", "launch a web browser"),
        ],
    )
    .expect("embed intent examples");

    // Deliberately a single classification: one utterance embedding.
    let matched = classify(
        |text| embedder.embed(text).unwrap_or_default(),
        &intents,
        "make it a bit quieter",
        0.5,
    )
    .expect("paraphrase should classify above threshold");
    println!(
        "chosen intent: {} (similarity {:.3})",
        matched.intent_id, matched.similarity
    );
    assert_eq!(matched.intent_id, "volume_down");
}
