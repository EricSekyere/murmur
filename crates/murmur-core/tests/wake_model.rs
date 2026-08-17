//! Real-model wake tests. Skip at runtime when artifacts are absent —
//! the ask-the-loader pattern (see stub_build_refuses_to_transcribe):
//! a compile-time cfg cannot know whether files exist on this machine.
#![cfg(feature = "wake")]

use murmur_core::audio::wake;

fn models_present() -> bool {
    wake::resolved_model_paths()
        .map(|p| p.melspectrogram.exists() && p.embedding.exists() && p.head.exists())
        .unwrap_or(false)
}

#[test]
fn onnx_scorer_scores_silence_below_any_threshold() {
    if !models_present() {
        eprintln!("wake models not present; skipping");
        return;
    }
    let paths = wake::resolved_model_paths().expect("paths");
    let mut scorer = murmur_core::audio::wake_onnx::OnnxWakeScorer::new(&paths).expect("scorer");
    use murmur_core::audio::wake::WakeScorer;
    // 2 s of digital silence must never cross even the High threshold.
    for _ in 0..25 {
        let score = scorer
            .score(&vec![0.0; wake::WAKE_FRAME_SAMPLES])
            .expect("score");
        assert!(score < wake::WakeSensitivity::High.threshold());
    }
}
