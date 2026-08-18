//! ONNX scorer for the openWakeWord pipeline: melspectrogram → speech
//! embedding → trained head. Each model is single-input/single-output, so
//! inputs are passed positionally and the first output is extracted —
//! export-time tensor names vary across converters and are irrelevant here.
//!
//! Streaming contract (openWakeWord reference implementation):
//! - melspectrogram: f32 `[1, 1280]` raw audio → mel frames `[.., 32]`,
//!   transformed `x/10 + 2`.
//! - embedding: a window of 76 mel frames `[1, 76, 32, 1]`, advanced 8 mel
//!   frames per step → one 96-dim embedding.
//! - head: the latest 16 embeddings `[1, 16, 96]` → wake score `[1, 1]`.

use anyhow::Result;
use std::collections::VecDeque;

use super::wake::{WAKE_FRAME_SAMPLES, WakeModelPaths, WakeScorer};

/// Mel bins per frame produced by the melspectrogram model.
const MEL_BINS: usize = 32;
/// Mel frames per embedding window.
const EMB_WINDOW: usize = 76;
/// Mel frames the embedding window advances per step.
const EMB_STEP: usize = 8;
/// Embedding dimension produced by the backbone.
const EMB_DIM: usize = 96;
/// Consecutive embeddings the head scores at once.
const HEAD_WINDOW: usize = 16;
/// Cap on retained embeddings (the "few seconds" the privacy policy names).
const EMB_RING_CAP: usize = 32;

pub struct OnnxWakeScorer {
    mel: ort::session::Session,
    embedding: ort::session::Session,
    head: ort::session::Session,
    /// Rolling mel frames not yet consumed by an embedding window.
    mel_ring: VecDeque<[f32; MEL_BINS]>,
    /// Latest embeddings, oldest first. No raw audio is retained here.
    emb_ring: VecDeque<[f32; EMB_DIM]>,
}

impl OnnxWakeScorer {
    pub fn new(paths: &WakeModelPaths) -> Result<Self> {
        crate::stt::runtime::init_ort()
            .map_err(|e| anyhow::anyhow!("ORT init failed for wake scorer: {}", e))?;
        Ok(Self {
            mel: build_session(&paths.melspectrogram, "wake melspectrogram")?,
            embedding: build_session(&paths.embedding, "wake embedding")?,
            head: build_session(&paths.head, "wake head")?,
            mel_ring: VecDeque::with_capacity(EMB_WINDOW + EMB_STEP),
            emb_ring: VecDeque::with_capacity(EMB_RING_CAP),
        })
    }

    /// Run the melspectrogram model on one frame and append its mel rows.
    fn push_mel_frames(&mut self, frame: &[f32]) -> Result<()> {
        let input =
            ort::value::Tensor::from_array((vec![1_i64, frame.len() as i64], frame.to_vec()))
                .map_err(|e| anyhow::anyhow!("wake mel input tensor: {}", e))?;
        let outputs = self
            .mel
            .run(ort::inputs![input])
            .map_err(|e| anyhow::anyhow!("wake mel inference: {}", e))?;
        let (_, data) = outputs[0]
            .try_extract_tensor::<f32>()
            .map_err(|e| anyhow::anyhow!("wake mel output extract: {}", e))?;
        for row in data.chunks_exact(MEL_BINS) {
            let mut mel = [0.0_f32; MEL_BINS];
            for (dst, src) in mel.iter_mut().zip(row) {
                // openWakeWord's fixed post-transform of the mel output.
                *dst = src / 10.0 + 2.0;
            }
            self.mel_ring.push_back(mel);
        }
        Ok(())
    }

    /// Consume full embedding windows from the mel ring (advancing by
    /// EMB_STEP per window, matching the reference streaming pipeline).
    fn drain_embeddings(&mut self) -> Result<()> {
        while self.mel_ring.len() >= EMB_WINDOW {
            let mut window = Vec::with_capacity(EMB_WINDOW * MEL_BINS);
            for mel in self.mel_ring.iter().take(EMB_WINDOW) {
                window.extend_from_slice(mel);
            }
            let input = ort::value::Tensor::from_array((
                vec![1_i64, EMB_WINDOW as i64, MEL_BINS as i64, 1],
                window,
            ))
            .map_err(|e| anyhow::anyhow!("wake embedding input tensor: {}", e))?;
            let outputs = self
                .embedding
                .run(ort::inputs![input])
                .map_err(|e| anyhow::anyhow!("wake embedding inference: {}", e))?;
            let (_, data) = outputs[0]
                .try_extract_tensor::<f32>()
                .map_err(|e| anyhow::anyhow!("wake embedding output extract: {}", e))?;
            if data.len() != EMB_DIM {
                anyhow::bail!(
                    "wake embedding output has {} values, expected {}",
                    data.len(),
                    EMB_DIM
                );
            }
            let mut emb = [0.0_f32; EMB_DIM];
            emb.copy_from_slice(data);
            self.emb_ring.push_back(emb);
            while self.emb_ring.len() > EMB_RING_CAP {
                let _ = self.emb_ring.pop_front();
            }
            for _ in 0..EMB_STEP {
                let _ = self.mel_ring.pop_front();
            }
        }
        Ok(())
    }

    /// Score the latest HEAD_WINDOW embeddings, if enough have accumulated.
    fn run_head(&mut self) -> Result<Option<f32>> {
        if self.emb_ring.len() < HEAD_WINDOW {
            return Ok(None);
        }
        let mut window = Vec::with_capacity(HEAD_WINDOW * EMB_DIM);
        for emb in self.emb_ring.iter().skip(self.emb_ring.len() - HEAD_WINDOW) {
            window.extend_from_slice(emb);
        }
        let input = ort::value::Tensor::from_array((
            vec![1_i64, HEAD_WINDOW as i64, EMB_DIM as i64],
            window,
        ))
        .map_err(|e| anyhow::anyhow!("wake head input tensor: {}", e))?;
        let outputs = self
            .head
            .run(ort::inputs![input])
            .map_err(|e| anyhow::anyhow!("wake head inference: {}", e))?;
        let (_, data) = outputs[0]
            .try_extract_tensor::<f32>()
            .map_err(|e| anyhow::anyhow!("wake head output extract: {}", e))?;
        Ok(data.first().copied())
    }
}

impl WakeScorer for OnnxWakeScorer {
    fn score(&mut self, frame: &[f32]) -> Result<f32> {
        debug_assert_eq!(frame.len(), WAKE_FRAME_SAMPLES);
        self.push_mel_frames(frame)?;
        self.drain_embeddings()?;
        // While buffers are still filling there is nothing to score yet.
        Ok(self.run_head()?.unwrap_or(0.0))
    }

    fn reset(&mut self) {
        self.mel_ring.clear();
        self.emb_ring.clear();
    }
}

fn build_session(path: &std::path::Path, label: &str) -> Result<ort::session::Session> {
    let builder = ort::session::Session::builder()
        .map_err(|e| anyhow::anyhow!("Failed to build {label} session: {}", e))?;
    let builder = crate::stt::runtime::apply_low_memory(builder)
        .map_err(|e| anyhow::anyhow!("Failed to configure {label} session: {}", e))?;
    builder
        .commit_from_file(path)
        .map_err(|e| anyhow::anyhow!("Failed to load {label} model: {}", e))
}
