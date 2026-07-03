//! Local retrieval for the in-app Help search.
//!
//! Embeds a small bundled corpus of help articles and returns the sections most
//! semantically similar to a user's question, so the Help view can answer "how
//! do I configure X" without making anyone scroll. v1 is retrieval-only: it
//! surfaces the real authored passage, no generative model (see the project
//! notes for the deferred LLM phase).
//!
//! This module owns the corpus, chunking, and cosine search. The actual ONNX
//! embedding model plugs in through the [`Embedder`] trait, which keeps the
//! search logic and its tests independent of the model.

use anyhow::Result;

mod embed;
pub use embed::{OnnxEmbedder, download, is_downloaded, model_path};

/// Produces an L2-normalized embedding vector for a piece of text.
pub trait Embedder {
    /// Embed `text`; the returned vector should be unit length so callers can
    /// use a plain dot product as cosine similarity.
    fn embed(&self, text: &str) -> Result<Vec<f32>>;
}

/// One heading section of a help article, the unit we embed and search.
#[derive(Debug, Clone, PartialEq)]
pub struct Section {
    pub heading: String,
    pub body: String,
}

/// An embedded corpus section.
#[derive(Debug, Clone)]
pub struct HelpChunk {
    pub article: String,
    pub heading: String,
    pub body: String,
    pub embedding: Vec<f32>,
}

/// A search hit: the matched section plus its cosine score (1.0 = identical).
#[derive(Debug, Clone)]
pub struct HelpHit {
    pub article: String,
    pub heading: String,
    pub body: String,
    pub score: f32,
}

/// The bundled help articles, as `(title, markdown)` pairs. Authored as plain
/// markdown so they double as docs; embedded into the index at build.
pub fn articles() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            "Getting started",
            include_str!("articles/getting-started.md"),
        ),
        (
            "The floating pill",
            include_str!("articles/floating-pill.md"),
        ),
        (
            "Models, languages and translation",
            include_str!("articles/models.md"),
        ),
        (
            "Output and delivery",
            include_str!("articles/output-delivery.md"),
        ),
        (
            "Voice commands and editing",
            include_str!("articles/voice-commands.md"),
        ),
        (
            "Snippets and personal dictionary",
            include_str!("articles/snippets-dictionary.md"),
        ),
        (
            "Developer mode and code dictation",
            include_str!("articles/developer-mode.md"),
        ),
        (
            "Codebase vocabulary",
            include_str!("articles/codebase-vocabulary.md"),
        ),
        ("Per-app profiles", include_str!("articles/app-profiles.md")),
        (
            "Microphone and audio",
            include_str!("articles/microphone-audio.md"),
        ),
        (
            "Privacy and your data",
            include_str!("articles/privacy-data.md"),
        ),
        (
            "History and analytics",
            include_str!("articles/history-analytics.md"),
        ),
        ("Diagnostics", include_str!("articles/diagnostics.md")),
        (
            "Integrations and updates",
            include_str!("articles/integrations-updates.md"),
        ),
        (
            "Troubleshooting",
            include_str!("articles/troubleshooting.md"),
        ),
        (
            "Shortcuts and commands reference",
            include_str!("articles/shortcuts-reference.md"),
        ),
    ]
}

/// Split markdown into one section per heading. Text before the first heading is
/// dropped; an empty body is skipped. Heading level (`#`..`######`) is ignored.
pub fn chunk_markdown(markdown: &str) -> Vec<Section> {
    let mut sections = Vec::new();
    let mut heading = String::new();
    let mut body = String::new();

    for line in markdown.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('#') {
            push_section(&heading, &body, &mut sections);
            heading = trimmed.trim_start_matches('#').trim().to_string();
            body.clear();
        } else {
            body.push_str(line);
            body.push('\n');
        }
    }
    push_section(&heading, &body, &mut sections);
    sections
}

fn push_section(heading: &str, body: &str, out: &mut Vec<Section>) {
    let body = body.trim();
    if !heading.is_empty() && !body.is_empty() {
        out.push(Section {
            heading: heading.to_string(),
            body: body.to_string(),
        });
    }
}

// One cosine implementation serves Help search and the Tier 2 intent
// classifier; re-exported here to keep the `help::cosine` path stable.
pub use crate::command::cosine;

/// In-memory help index: embedded sections searched by brute-force cosine. The
/// corpus is tiny (dozens of sections), so a flat scan is microseconds and needs
/// no vector database.
pub struct HelpIndex {
    chunks: Vec<HelpChunk>,
}

impl HelpIndex {
    /// Embed every section of every article. The query is embedded at search
    /// time; this is the one-time corpus pass (precomputed at build in release).
    pub fn build(embedder: &dyn Embedder, articles: &[(&str, &str)]) -> Result<Self> {
        let mut chunks = Vec::new();
        for (title, markdown) in articles {
            for section in chunk_markdown(markdown) {
                // Embed heading + body together so the heading's keywords count.
                let embedding =
                    embedder.embed(&format!("{}\n{}", section.heading, section.body))?;
                chunks.push(HelpChunk {
                    article: (*title).to_string(),
                    heading: section.heading,
                    body: section.body,
                    embedding,
                });
            }
        }
        Ok(Self { chunks })
    }

    /// Top `k` sections by cosine similarity to a pre-embedded query, best first.
    pub fn search(&self, query_embedding: &[f32], k: usize) -> Vec<HelpHit> {
        let mut scored: Vec<HelpHit> = self
            .chunks
            .iter()
            .map(|c| HelpHit {
                article: c.article.clone(),
                heading: c.heading.clone(),
                body: c.body.clone(),
                score: cosine(query_embedding, &c.embedding),
            })
            .collect();
        scored.sort_by(|a, b| b.score.total_cmp(&a.score));
        scored.truncate(k);
        scored
    }

    pub fn len(&self) -> usize {
        self.chunks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.chunks.is_empty()
    }
}

/// The ready-to-query Help engine: the embedder plus an index over the bundled
/// corpus. `load()` does the one-time corpus embedding; `search()` embeds the
/// query and returns the best sections.
pub struct HelpEngine {
    embedder: OnnxEmbedder,
    index: HelpIndex,
}

impl HelpEngine {
    /// Load the embedder (model must be downloaded) and embed the corpus.
    pub fn load() -> Result<Self> {
        let embedder = OnnxEmbedder::load()?;
        let index = HelpIndex::build(&embedder, &articles())?;
        Ok(Self { embedder, index })
    }

    /// Top `k` help sections for a natural-language question.
    pub fn search(&self, query: &str, k: usize) -> Result<Vec<HelpHit>> {
        let embedding = self.embedder.embed_query(query)?;
        Ok(self.index.search(&embedding, k))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// A deterministic bag-of-words embedder for tests: each dimension is a word
    /// from a fixed vocabulary, value = 1 if present. Cosine then reflects word
    /// overlap, enough to assert the ranking without the real ONNX model.
    struct BagOfWords {
        vocab: Vec<String>,
    }
    impl BagOfWords {
        fn new(corpus: &[&str]) -> Self {
            let mut set: HashSet<String> = HashSet::new();
            for text in corpus {
                for w in words(text) {
                    set.insert(w);
                }
            }
            let mut vocab: Vec<String> = set.into_iter().collect();
            vocab.sort();
            Self { vocab }
        }
    }
    impl Embedder for BagOfWords {
        fn embed(&self, text: &str) -> Result<Vec<f32>> {
            let present: HashSet<String> = words(text).into_iter().collect();
            Ok(self
                .vocab
                .iter()
                .map(|w| if present.contains(w) { 1.0 } else { 0.0 })
                .collect())
        }
    }
    fn words(text: &str) -> Vec<String> {
        text.split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .map(|w| w.to_lowercase())
            .collect()
    }

    #[test]
    fn chunk_splits_on_headings_and_drops_preamble() {
        let md = "intro before any heading\n# Title\nbody one\n## Section\nbody two\n";
        let sections = chunk_markdown(md);
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].heading, "Title");
        assert_eq!(sections[0].body, "body one");
        assert_eq!(sections[1].heading, "Section");
        assert_eq!(sections[1].body, "body two");
    }

    #[test]
    fn chunk_skips_empty_sections() {
        let md = "# Empty\n\n# Real\ncontent\n";
        let sections = chunk_markdown(md);
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].heading, "Real");
    }

    #[test]
    fn cosine_handles_identical_orthogonal_and_degenerate() {
        assert!((cosine(&[1.0, 2.0, 3.0], &[1.0, 2.0, 3.0]) - 1.0).abs() < 1e-6);
        assert!(cosine(&[1.0, 0.0], &[0.0, 1.0]).abs() < 1e-6);
        assert_eq!(cosine(&[0.0, 0.0], &[1.0, 1.0]), 0.0);
        assert_eq!(cosine(&[1.0], &[1.0, 2.0]), 0.0);
    }

    #[test]
    fn search_ranks_the_semantically_closest_section_first() {
        let articles = [
            (
                "Hotkey",
                "# Change the hotkey\nOpen settings and rebind the push to talk key.\n",
            ),
            (
                "Privacy",
                "# Local only\nEverything runs on device with no cloud or telemetry.\n",
            ),
            (
                "Models",
                "# Pick a model\nChoose Parakeet or Whisper and download it.\n",
            ),
        ];
        let corpus: Vec<&str> = articles.iter().map(|(_, md)| *md).collect();
        let embedder = BagOfWords::new(&corpus);
        let index = HelpIndex::build(&embedder, &articles).unwrap();
        assert_eq!(index.len(), 3);

        let q = embedder
            .embed("how do I rebind my push to talk key")
            .unwrap();
        let hits = index.search(&q, 3);
        assert_eq!(hits[0].article, "Hotkey");
        assert!(hits[0].score > hits[1].score);
    }
}
