//! ONNX sentence embedder for Help search.
//!
//! Loads bge-small-en-v1.5 (int8) through the ORT runtime the STT path already
//! initializes, and turns text into a 384-dim L2-normalized vector. Mirrors the
//! Silero path in `audio::vad`: the tokenizer is bundled, the model is
//! downloaded once and checksum-pinned.

use std::sync::Mutex;

use anyhow::{Context, Result};
use ort::session::Session;
use ort::value::Tensor;
use tokenizers::Tokenizer;

use super::Embedder;

/// bge-small retrieves best when the *query* carries this instruction; passages
/// (the corpus) are embedded as-is.
const QUERY_INSTRUCTION: &str = "Represent this sentence for searching relevant passages: ";

const MODEL_URL: &str =
    "https://huggingface.co/Xenova/bge-small-en-v1.5/resolve/main/onnx/model_quantized.onnx";
/// Pinned SHA256 of the int8 ONNX model, verified against the live download.
const MODEL_SHA256: &str = "6c9c6101a956d62dfb5e7190c538226c0c5bb9cb27b651234b6df063ee7dbfe4";
const MAX_TOKENS: usize = 256;

/// Filesystem path where the embedding model is cached.
pub fn model_path() -> Result<std::path::PathBuf> {
    let dir = dirs::data_dir()
        .ok_or_else(|| anyhow::anyhow!("Could not determine data directory"))?
        .join("murmur")
        .join("help");
    Ok(dir.join("bge-small-en-v1.5.onnx"))
}

/// Whether the embedding model has been downloaded.
pub fn is_downloaded() -> bool {
    model_path().map(|p| p.exists()).unwrap_or(false)
}

/// Download the embedding model (~33 MB) once, verifying its checksum.
/// Idempotent; an interrupted download resumes from its `.partial` file.
pub async fn download() -> Result<std::path::PathBuf> {
    let dest = model_path()?;
    if dest.exists() {
        return Ok(dest);
    }
    tracing::info!("Downloading Help embedding model from {}", MODEL_URL);
    crate::download::fetch_to_file(
        MODEL_URL,
        &dest,
        MODEL_SHA256,
        "Help embedding model",
        |_, _| {},
    )
    .await?;
    tracing::info!("Help embedding model ready at {}", dest.display());
    Ok(dest)
}

/// A bge-small ONNX text embedder. The session is behind a mutex because
/// `Session::run` needs `&mut`, but the [`Embedder`] trait (and shared app
/// state) hold it by `&self`.
pub struct OnnxEmbedder {
    session: Mutex<Session>,
    tokenizer: Tokenizer,
}

impl OnnxEmbedder {
    /// Load from the downloaded model at [`model_path`]. The ORT runtime must be
    /// available (the STT/VAD path initializes it); fails cleanly otherwise.
    pub fn load() -> Result<Self> {
        let path = model_path()?;
        crate::stt::runtime::init_ort()
            .map_err(|e| anyhow::anyhow!("ORT init failed for Help embedder: {e}"))?;
        let builder = Session::builder().context("build embedder session")?;
        let builder =
            crate::stt::runtime::apply_low_memory(builder).context("configure embedder session")?;
        let session = builder
            .commit_from_file(&path)
            .with_context(|| format!("load embedder model from {}", path.display()))?;
        let tokenizer = Tokenizer::from_bytes(include_bytes!("model/tokenizer.json"))
            .map_err(|e| anyhow::anyhow!("load embedder tokenizer: {e}"))?;
        tracing::info!("Help embedder loaded");
        Ok(Self {
            session: Mutex::new(session),
            tokenizer,
        })
    }

    /// Embed a search query (prepends the bge retrieval instruction).
    pub fn embed_query(&self, query: &str) -> Result<Vec<f32>> {
        self.embed(&format!("{QUERY_INSTRUCTION}{query}"))
    }
}

impl Embedder for OnnxEmbedder {
    fn embed(&self, text: &str) -> Result<Vec<f32>> {
        let enc = self
            .tokenizer
            .encode(text, true)
            .map_err(|e| anyhow::anyhow!("tokenize: {e}"))?;
        let mut ids: Vec<i64> = enc.get_ids().iter().map(|&i| i as i64).collect();
        let mut mask: Vec<i64> = enc.get_attention_mask().iter().map(|&i| i as i64).collect();
        ids.truncate(MAX_TOKENS);
        mask.truncate(MAX_TOKENS);
        let seq = ids.len();
        if seq == 0 {
            return Ok(Vec::new());
        }
        let types = vec![0i64; seq];

        let ids_t = Tensor::from_array((vec![1_i64, seq as i64], ids))
            .map_err(|e| anyhow::anyhow!("ids tensor: {e}"))?;
        let mask_t = Tensor::from_array((vec![1_i64, seq as i64], mask.clone()))
            .map_err(|e| anyhow::anyhow!("mask tensor: {e}"))?;
        let types_t = Tensor::from_array((vec![1_i64, seq as i64], types))
            .map_err(|e| anyhow::anyhow!("types tensor: {e}"))?;

        let mut session = self.session.lock().unwrap_or_else(|e| e.into_inner());
        let outputs = session
            .run(ort::inputs![
                "input_ids" => ids_t,
                "attention_mask" => mask_t,
                "token_type_ids" => types_t,
            ])
            .map_err(|e| anyhow::anyhow!("embedder inference: {e}"))?;

        // First output is last_hidden_state [1, seq, dim]. Mean-pool over tokens
        // weighted by the attention mask, then L2-normalize.
        let (_, data) = outputs[0]
            .try_extract_tensor::<f32>()
            .map_err(|e| anyhow::anyhow!("embedder output extract: {e}"))?;
        let dim = data.len() / seq;
        if dim == 0 {
            return Ok(Vec::new());
        }
        let mut pooled = vec![0f32; dim];
        let mut mask_sum = 0f32;
        for (t, &m) in mask.iter().enumerate() {
            let m = m as f32;
            mask_sum += m;
            let base = t * dim;
            for d in 0..dim {
                pooled[d] += data[base + d] * m;
            }
        }
        let denom = mask_sum.max(1.0);
        for v in &mut pooled {
            *v /= denom;
        }
        let norm = pooled.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            for v in &mut pooled {
                *v /= norm;
            }
        }
        Ok(pooled)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::help::{HelpIndex, articles};

    /// Needs the downloaded model + the ORT DLL, so it is ignored in CI. Run
    /// locally with the model present:
    ///   cargo test -p murmur-core --features help embedder_smoke -- --ignored --nocapture
    #[test]
    #[ignore]
    fn embedder_smoke_ranks_relevant_section() {
        let embedder = OnnxEmbedder::load().expect("load embedder (model must be downloaded)");
        let index = HelpIndex::build(&embedder, &articles()).expect("build index");
        assert!(index.len() > 50, "corpus should produce many chunks");

        for q in [
            "why does my microphone keep stopping",
            "how do I change the keyboard shortcut",
            "is my voice sent to the cloud",
        ] {
            let emb = embedder.embed_query(q).unwrap();
            let hits = index.search(&emb, 3);
            println!("\nQ: {q}");
            for h in &hits {
                println!("  {:.3}  {} / {}", h.score, h.article, h.heading);
            }
            assert!(hits[0].score > 0.3, "top hit should be reasonably similar");
        }
    }
}
