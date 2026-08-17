//! Wake-word detection ("Hey Murmur") on the openWakeWord architecture:
//! melspectrogram → Google speech_embedding backbone → trained head.
//! Feature-gated (`wake`); `WakeSensitivity` exists unconditionally because
//! the config type needs it on every build.

use anyhow::Result;

/// openWakeWord consumes 1280-sample (80 ms) frames at 16 kHz.
pub const WAKE_FRAME_SAMPLES: usize = 1280;

/// Frames ignored after a detection (~2 s at 80 ms/frame) so one utterance
/// cannot double-trigger while its tail still scores high.
pub const WAKE_REFRACTORY_FRAMES: usize = 25;

/// A wake-word hit: the score that crossed the threshold.
pub struct WakeDetection {
    pub score: f32,
}

/// Scores one 1280-sample 16 kHz mono frame, returning a wake probability in
/// `[0.0, 1.0]`. Object-safe so the app worker can hold `Box<dyn WakeScorer>`
/// and tests can inject scripted scorers.
pub trait WakeScorer: Send {
    fn score(&mut self, frame: &[f32]) -> Result<f32>;
    /// Clear any internal feature buffers (between armed periods).
    fn reset(&mut self);
}

impl WakeScorer for Box<dyn WakeScorer> {
    fn score(&mut self, frame: &[f32]) -> Result<f32> {
        (**self).score(frame)
    }
    fn reset(&mut self) {
        (**self).reset();
    }
}

/// Streaming wake-word detector: buffers arbitrary-length sample slices into
/// complete frames, scores each, applies the threshold and refractory.
pub struct WakeWordDetector<S: WakeScorer> {
    scorer: S,
    threshold: f32,
    pending: Vec<f32>,
    refractory_left: usize,
}

impl<S: WakeScorer> WakeWordDetector<S> {
    pub fn new(scorer: S, threshold: f32) -> Self {
        Self {
            scorer,
            threshold,
            pending: Vec::with_capacity(WAKE_FRAME_SAMPLES * 2),
            refractory_left: 0,
        }
    }

    /// Feed 16 kHz mono samples. Returns the first detection in this batch;
    /// remaining complete frames are still scored (state advances) so a
    /// detection never leaves half a batch unconsumed.
    pub fn feed(&mut self, samples: &[f32]) -> Result<Option<WakeDetection>> {
        self.pending.extend_from_slice(samples);
        let mut detection = None;
        while self.pending.len() >= WAKE_FRAME_SAMPLES {
            // Per-frame allocation is fine here: the armed loop ticks at
            // 80 ms on its own thread, not the realtime capture callback.
            let frame: Vec<f32> = self.pending.drain(..WAKE_FRAME_SAMPLES).collect();
            let score = self.scorer.score(&frame)?;
            if self.refractory_left > 0 {
                self.refractory_left -= 1;
                continue;
            }
            if score >= self.threshold && detection.is_none() {
                self.refractory_left = WAKE_REFRACTORY_FRAMES;
                detection = Some(WakeDetection { score });
            }
        }
        Ok(detection)
    }

    pub fn reset(&mut self) {
        self.pending.clear();
        self.refractory_left = 0;
        self.scorer.reset();
    }
}

/// User-facing sensitivity; thresholds come from the training report's
/// measured operating points (initial values pending the measurement task).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum WakeSensitivity {
    Low,
    #[default]
    Medium,
    High,
}

impl WakeSensitivity {
    /// Wake-probability threshold this sensitivity maps to. Lower threshold
    /// means the detector fires more readily (High sensitivity).
    ///
    /// Measured, not chosen: these are the operating points `evaluate.py`
    /// reports, each the loosest threshold inside its own false-accept budget
    /// on the validation half, then measured once on the held-out half, where
    /// they cost 0.043, 0.065 and 0.910 false accepts an hour. An earlier set
    /// was picked and reported on one sample, so those numbers were the best
    /// of ~126 tries rather than an estimate.
    ///
    /// Only Medium is a release gate, and only Medium's budget is demonstrated
    /// rather than merely measured: its 3 events over 46 h give an interval
    /// upper bound of 0.190 against a 0.5 ceiling. Low measures 0.043 but on
    /// 2 events, so its interval reaches 0.157 and exceeds its own 0.1 budget;
    /// validation resolves a rate only to 0.136/h, above that budget, so Low
    /// can only ever be chosen where validation saw zero events. Treat Low's
    /// budget as a point estimate, not a promise.
    ///
    /// The head's positive scores span only about 0.002, so rounding these to
    /// two decimals collapses recall to nearly zero. Seven digits is all an f32
    /// can hold; the report's eighth costs 2e-8, which is three orders inside
    /// the gap between adjacent positive scores. Re-derive them from
    /// `report.json` whenever the head is retrained or the calibration changes;
    /// a threshold from one model says nothing about another.
    pub fn threshold(self) -> f32 {
        match self {
            WakeSensitivity::Low => 0.9842523,
            WakeSensitivity::Medium => 0.9838503,
            WakeSensitivity::High => 0.9663407,
        }
    }
}

/// Filesystem locations of the three wake model files.
#[cfg(feature = "wake")]
pub struct WakeModelPaths {
    pub melspectrogram: std::path::PathBuf,
    pub embedding: std::path::PathBuf,
    pub head: std::path::PathBuf,
}

/// Directory the wake models are cached under.
#[cfg(feature = "wake")]
pub fn model_dir() -> Result<std::path::PathBuf> {
    let dir = crate::fsutil::data_base_dir()
        .ok_or_else(|| anyhow::anyhow!("Could not determine data directory"))?
        .join("murmur")
        .join("wake");
    Ok(dir)
}

#[cfg(feature = "wake")]
pub fn model_paths() -> Result<WakeModelPaths> {
    let dir = model_dir()?;
    Ok(WakeModelPaths {
        melspectrogram: dir.join("melspectrogram.onnx"),
        embedding: dir.join("embedding_model.onnx"),
        head: dir.join("hey_murmur.onnx"),
    })
}

/// Whether all three model files are present in the cache.
#[cfg(feature = "wake")]
pub fn is_downloaded() -> bool {
    model_paths()
        .map(|p| p.melspectrogram.exists() && p.embedding.exists() && p.head.exists())
        .unwrap_or(false)
}

/// Model paths honouring the `MURMUR_WAKE_MODEL_DIR` override (tests and
/// development builds point it at a local artifact directory).
#[cfg(feature = "wake")]
pub fn resolved_model_paths() -> Result<WakeModelPaths> {
    if let Ok(dir) = std::env::var("MURMUR_WAKE_MODEL_DIR") {
        let dir = std::path::PathBuf::from(dir);
        return Ok(WakeModelPaths {
            melspectrogram: dir.join("melspectrogram.onnx"),
            embedding: dir.join("embedding_model.onnx"),
            head: dir.join("hey_murmur.onnx"),
        });
    }
    model_paths()
}

// ── Model download ──────────────────────────────────────────────────────────
//
// All three files are rehosted in Murmur's own GitHub releases so pinned
// hashes can never drift under us. The melspectrogram + embedding backbone is
// openWakeWord's Apache-2.0 feature extractor (Google speech_embedding); the
// head is Murmur's own trained artifact (see training/wake-word/).
// Pins hold a fail-closed non-hex sentinel until the `wake-models-v1` release
// assets exist; `ensure_pinned` rejects the sentinel, so the feature cannot
// fetch unpinned files (filled in by the training/release task).

#[cfg(feature = "wake")]
const RELEASE_BASE: &str = "https://github.com/EricSekyere/murmur/releases/download/wake-models-v1";

#[cfg(feature = "wake")]
const MELSPECTROGRAM_SHA256: &str = "pending-release-upload";
#[cfg(feature = "wake")]
const EMBEDDING_SHA256: &str = "pending-release-upload";
#[cfg(feature = "wake")]
const HEY_MURMUR_SHA256: &str = "pending-release-upload";

/// Combined size of the three model files, shown to the user before the
/// first-enable download. Zero until the release assets exist (the download
/// itself is blocked by the pin sentinel until then).
#[cfg(feature = "wake")]
pub const TOTAL_DOWNLOAD_BYTES: u64 = 0;

/// Download all three wake model files into the cache, verifying each
/// against its pinned SHA256. Idempotent per file; progress is reported as
/// `(label, bytes_done, bytes_total)` per file — the total is `None` when
/// the server does not announce a length.
#[cfg(feature = "wake")]
pub async fn download(progress: impl Fn(&str, u64, Option<u64>)) -> Result<WakeModelPaths> {
    let paths = model_paths()?;
    let files: [(&std::path::Path, &str, &str); 3] = [
        (
            &paths.melspectrogram,
            MELSPECTROGRAM_SHA256,
            "wake melspectrogram model",
        ),
        (&paths.embedding, EMBEDDING_SHA256, "wake embedding model"),
        (&paths.head, HEY_MURMUR_SHA256, "Hey Murmur wake model"),
    ];
    // Validate every pin before any network work so a partial set of pins
    // can never leave a half-downloaded model directory behind.
    for (_, sha, label) in &files {
        crate::integrity::ensure_pinned(sha, label)?;
    }
    for (dest, sha, label) in files {
        if dest.exists() {
            continue;
        }
        let file_name = dest
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| anyhow::anyhow!("Invalid wake model path: {}", dest.display()))?;
        let url = format!("{RELEASE_BASE}/{file_name}");
        tracing::info!(%url, "downloading wake model file");
        crate::download::fetch_to_file(&url, dest, sha, label, |done, total| {
            progress(label, done, total);
        })
        .await?;
    }
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sensitivity_maps_to_descending_thresholds() {
        assert!(WakeSensitivity::High.threshold() < WakeSensitivity::Medium.threshold());
        assert!(WakeSensitivity::Medium.threshold() < WakeSensitivity::Low.threshold());
    }

    #[test]
    fn sensitivity_serde_roundtrip() {
        let json = serde_json::to_string(&WakeSensitivity::Medium).unwrap();
        assert_eq!(json, "\"medium\"");
        let back: WakeSensitivity = serde_json::from_str(&json).unwrap();
        assert_eq!(back, WakeSensitivity::Medium);
    }

    struct ScriptedScorer {
        scores: std::collections::VecDeque<f32>,
    }
    impl WakeScorer for ScriptedScorer {
        fn score(&mut self, _frame: &[f32]) -> Result<f32> {
            Ok(self.scores.pop_front().unwrap_or(0.0))
        }
        fn reset(&mut self) {}
    }

    fn frames(n: usize) -> Vec<f32> {
        vec![0.0; n * WAKE_FRAME_SAMPLES]
    }

    #[test]
    fn feed_buffers_partial_frames_across_calls() {
        let scorer = ScriptedScorer {
            scores: [0.9].into(),
        };
        let mut det = WakeWordDetector::new(scorer, 0.5);
        // 1279 samples: no complete frame, no score consumed.
        assert!(
            det.feed(&vec![0.0; WAKE_FRAME_SAMPLES - 1])
                .unwrap()
                .is_none()
        );
        // 1 more sample completes the frame -> detection fires.
        let hit = det.feed(&[0.0]).unwrap().expect("detection");
        assert!((hit.score - 0.9).abs() < f32::EPSILON);
    }

    #[test]
    fn below_threshold_never_detects() {
        let scorer = ScriptedScorer {
            scores: [0.49, 0.49, 0.49].into(),
        };
        let mut det = WakeWordDetector::new(scorer, 0.5);
        assert!(det.feed(&frames(3)).unwrap().is_none());
    }

    #[test]
    fn refractory_suppresses_double_trigger() {
        // Frame 1 detects; the next WAKE_REFRACTORY_FRAMES frames score high
        // but must not re-fire; the frame after the window may fire again.
        let mut scores = vec![0.9];
        scores.extend(std::iter::repeat_n(0.9, WAKE_REFRACTORY_FRAMES));
        scores.push(0.9);
        let scorer = ScriptedScorer {
            scores: scores.into(),
        };
        let mut det = WakeWordDetector::new(scorer, 0.5);
        assert!(det.feed(&frames(1)).unwrap().is_some());
        assert!(det.feed(&frames(WAKE_REFRACTORY_FRAMES)).unwrap().is_none());
        assert!(det.feed(&frames(1)).unwrap().is_some());
    }

    #[test]
    fn feed_returns_first_detection_in_a_batch() {
        let scorer = ScriptedScorer {
            scores: [0.1, 0.8, 0.9].into(),
        };
        let mut det = WakeWordDetector::new(scorer, 0.5);
        let hit = det.feed(&frames(3)).unwrap().expect("detection");
        assert!((hit.score - 0.8).abs() < f32::EPSILON);
    }

    #[test]
    fn reset_clears_pending_and_refractory() {
        let scorer = ScriptedScorer {
            scores: [0.9, 0.9].into(),
        };
        let mut det = WakeWordDetector::new(scorer, 0.5);
        assert!(det.feed(&frames(1)).unwrap().is_some());
        det.reset();
        // Refractory cleared: the very next frame may fire again.
        assert!(det.feed(&frames(1)).unwrap().is_some());
    }

    #[cfg(feature = "wake")]
    #[test]
    fn model_paths_live_under_wake_dir() {
        let paths = model_paths().unwrap();
        assert!(paths.melspectrogram.ends_with("melspectrogram.onnx"));
        assert!(paths.embedding.ends_with("embedding_model.onnx"));
        assert!(paths.head.ends_with("hey_murmur.onnx"));
    }
}
