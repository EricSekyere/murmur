//! Tier 2 embedding intent classifier (docs/command-mode-design.md,
//! Section 4): on a Tier 1 grammar miss, match the utterance against
//! registered intents by cosine similarity over sentence embeddings, catching
//! paraphrases the deterministic grammar cannot ("make it quieter" versus
//! "set volume to 40").
//!
//! The embedding function is injected (any text-to-vector closure), so the
//! scoring logic is pure math, testable without a model. The Help feature's
//! ONNX embedder plugs in behind `feature = "help"` via
//! [`IntentSet::embed_examples`].

/// A Tier 2 classification: the winning intent and its cosine similarity.
#[derive(Debug, Clone, PartialEq)]
pub struct IntentMatch {
    pub intent_id: String,
    pub similarity: f32,
}

#[derive(Debug, Clone)]
struct Example {
    phrase: String,
    embedding: Vec<f32>,
}

#[derive(Debug, Clone)]
struct Intent {
    id: String,
    examples: Vec<Example>,
}

/// Registered intents, each holding example phrases with their precomputed
/// embedding vectors. Embed examples once at registration; only the utterance
/// is embedded per classification.
#[derive(Debug, Clone, Default)]
pub struct IntentSet {
    intents: Vec<Intent>,
}

impl IntentSet {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register an example `phrase` (with its precomputed `embedding`) for
    /// `intent_id`, creating the intent on first use.
    pub fn add(&mut self, intent_id: &str, phrase: &str, embedding: Vec<f32>) {
        let example = Example {
            phrase: phrase.to_string(),
            embedding,
        };
        match self.intents.iter_mut().find(|i| i.id == intent_id) {
            Some(intent) => intent.examples.push(example),
            None => self.intents.push(Intent {
                id: intent_id.to_string(),
                examples: vec![example],
            }),
        }
    }

    /// Number of registered intents (not examples).
    pub fn len(&self) -> usize {
        self.intents.len()
    }

    pub fn is_empty(&self) -> bool {
        self.intents.is_empty()
    }

    /// Build a set by embedding `(intent_id, phrase)` examples with the Help
    /// sentence embedder (or any [`crate::help::Embedder`]).
    ///
    /// # Errors
    /// Propagates the first embedding failure.
    #[cfg(feature = "help")]
    pub fn embed_examples(
        embedder: &dyn crate::help::Embedder,
        examples: &[(&str, &str)],
    ) -> anyhow::Result<Self> {
        let mut set = Self::new();
        for (intent_id, phrase) in examples {
            set.add(intent_id, phrase, embedder.embed(phrase)?);
        }
        Ok(set)
    }
}

/// Classify `utterance` against `intents`: embed it, score cosine similarity
/// against every example (an intent scores the max over its examples), and
/// return the best intent when its similarity reaches `threshold`.
///
/// `embed` is injected so scoring is testable with hand-written vectors. An
/// empty embedding (an embedder failure signal) yields `None`.
pub fn classify<E>(
    embed: E,
    intents: &IntentSet,
    utterance: &str,
    threshold: f32,
) -> Option<IntentMatch>
where
    E: Fn(&str) -> Vec<f32>,
{
    if intents.is_empty() {
        return None;
    }
    let query = embed(utterance);
    if query.is_empty() {
        tracing::debug!("tier 2 utterance embedding is empty, skipping");
        return None;
    }

    // Global max over (intent, example) pairs equals max-over-examples per
    // intent followed by best-intent selection; ties keep the first seen.
    let mut best: Option<(&Intent, &Example, f32)> = None;
    for intent in &intents.intents {
        for example in &intent.examples {
            let score = cosine(&query, &example.embedding);
            if best.as_ref().is_none_or(|(_, _, top)| score > *top) {
                best = Some((intent, example, score));
            }
        }
    }
    let (intent, example, similarity) = best?;
    if similarity < threshold {
        tracing::debug!(similarity, threshold, "tier 2 best score below threshold");
        return None;
    }
    tracing::debug!(
        intent = %intent.id,
        example = %example.phrase,
        similarity,
        "tier 2 matched intent"
    );
    Some(IntentMatch {
        intent_id: intent.id.clone(),
        similarity,
    })
}

/// Boxed text-to-vector embedding function held by [`IntentClassifier`].
type EmbedFn = Box<dyn Fn(&str) -> Vec<f32> + Send + Sync>;

/// A ready-to-route Tier 2 classifier: an embedding function, the registered
/// intents, and the acceptance threshold. The router treats it as optional;
/// without one, a Tier 1 miss falls straight to Tier 3.
pub struct IntentClassifier {
    embed: EmbedFn,
    intents: IntentSet,
    threshold: f32,
}

impl IntentClassifier {
    pub fn new(
        embed: impl Fn(&str) -> Vec<f32> + Send + Sync + 'static,
        intents: IntentSet,
        threshold: f32,
    ) -> Self {
        Self {
            embed: Box::new(embed),
            intents,
            threshold,
        }
    }

    /// Classify one utterance; see [`classify`].
    pub fn classify(&self, utterance: &str) -> Option<IntentMatch> {
        classify(&self.embed, &self.intents, utterance, self.threshold)
    }
}

/// Cosine similarity of two equal-length vectors. Returns 0.0 on a length
/// mismatch or a zero-magnitude vector rather than NaN.
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    let mut norm_a = 0.0f32;
    let mut norm_b = 0.0f32;
    for (x, y) in a.iter().zip(b) {
        dot += x * y;
        norm_a += x * x;
        norm_b += y * y;
    }
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a.sqrt() * norm_b.sqrt())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cosine_identical_orthogonal_zero_norm_and_mismatch() {
        assert!((cosine(&[1.0, 2.0, 3.0], &[1.0, 2.0, 3.0]) - 1.0).abs() < 1e-6);
        assert!(cosine(&[1.0, 0.0], &[0.0, 1.0]).abs() < 1e-6);
        assert_eq!(cosine(&[0.0, 0.0], &[1.0, 1.0]), 0.0);
        assert_eq!(cosine(&[1.0, 1.0], &[0.0, 0.0]), 0.0);
        assert_eq!(cosine(&[], &[]), 0.0);
        assert_eq!(cosine(&[1.0], &[1.0, 2.0]), 0.0);
    }

    /// Test embedder: returns a fixed vector per known phrase.
    fn lookup(pairs: &[(&str, [f32; 2])]) -> impl Fn(&str) -> Vec<f32> + use<> {
        let pairs: Vec<(String, [f32; 2])> =
            pairs.iter().map(|(t, v)| (t.to_string(), *v)).collect();
        move |text: &str| {
            pairs
                .iter()
                .find(|(t, _)| t == text)
                .map(|(_, v)| v.to_vec())
                .unwrap_or_default()
        }
    }

    fn two_intent_set() -> IntentSet {
        let mut intents = IntentSet::new();
        intents.add("volume_down", "turn it down", vec![1.0, 0.0]);
        intents.add("open_browser", "open the browser", vec![0.0, 1.0]);
        intents
    }

    #[test]
    fn classify_returns_best_intent_above_threshold() {
        let embed = lookup(&[("quieter please", [0.9, 0.1])]);
        let matched = classify(embed, &two_intent_set(), "quieter please", 0.8)
            .expect("should match volume_down");
        assert_eq!(matched.intent_id, "volume_down");
        assert!(matched.similarity > 0.9);
    }

    #[test]
    fn classify_returns_none_below_threshold() {
        let embed = lookup(&[("quieter please", [0.9, 0.1])]);
        assert_eq!(
            classify(embed, &two_intent_set(), "quieter please", 0.999),
            None
        );
    }

    #[test]
    fn intent_score_is_the_max_over_its_examples() {
        let mut intents = two_intent_set();
        // A second volume_down example orthogonal to the query must not drag
        // the intent down; open_browser sits closer than that weak example.
        intents.add("volume_down", "mute everything", vec![0.0, 1.0]);
        let embed = lookup(&[("quieter please", [1.0, 0.0])]);
        let matched = classify(embed, &intents, "quieter please", 0.5).expect("match");
        assert_eq!(matched.intent_id, "volume_down");
        assert!((matched.similarity - 1.0).abs() < 1e-6);
    }

    #[test]
    fn empty_intents_or_empty_embedding_yield_none() {
        let embed = |_: &str| vec![1.0, 0.0];
        assert_eq!(classify(embed, &IntentSet::new(), "anything", 0.0), None);
        // Unknown utterance embeds to an empty vector: no panic, no match.
        let embed = lookup(&[]);
        assert_eq!(classify(embed, &two_intent_set(), "anything", 0.0), None);
    }

    #[test]
    fn classifier_wraps_the_same_scoring() {
        let classifier = IntentClassifier::new(
            lookup(&[("quieter please", [0.9, 0.1])]),
            two_intent_set(),
            0.8,
        );
        let matched = classifier.classify("quieter please").expect("match");
        assert_eq!(matched.intent_id, "volume_down");
        assert_eq!(classifier.classify("unknown phrase"), None);
    }
}
