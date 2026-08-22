//! Symbols an editor reports as currently on screen.
//!
//! The indexer already biases the decoder toward every symbol in the project,
//! which is broad but undirected: a large codebase offers far more candidates
//! than fit the prompt budget. An editor knows something narrower and much
//! better, namely what the user is looking at right now, and those are the
//! identifiers they are most likely to say out loud.
//!
//! Deliberately not an LLM pass over the transcript. Biasing the decoder costs
//! nothing at inference time, where a rewrite step would spend the latency
//! budget `prd.md` reserves for the whole dictation round trip.
//!
//! Contents are identifiers out of the user's own source, held in memory only.
//! They are never written to disk and never logged above `trace`.

use std::time::{Duration, Instant};

/// How many symbols one editor update may contribute.
///
/// The prompt budget is far smaller than this, and the user's own glossary is
/// merged first, so this is a bound on memory and work rather than a tuning
/// knob; anything past the budget is dropped downstream anyway.
pub(crate) const MAX_SYMBOLS: usize = 200;

/// Longest symbol accepted. Real identifiers are far shorter, and a very long
/// one would only crowd out others.
const MAX_SYMBOL_CHARS: usize = 80;

/// How long an update stays relevant.
///
/// Context goes stale silently: an editor that closes, crashes, or loses focus
/// stops sending, and biasing the decoder toward a file the user left ten
/// minutes ago is worse than not biasing it at all. Expiry means the worst a
/// dead client can do is stop helping.
const TTL: Duration = Duration::from_secs(300);

/// The most recent editor update, or nothing if none has arrived or it aged out.
#[derive(Debug, Default)]
pub(crate) struct EditorContext {
    symbols: Vec<String>,
    updated: Option<Instant>,
}

impl EditorContext {
    /// Replace the context, returning how many symbols were kept.
    ///
    /// Replace rather than accumulate: the editor reports what is on screen
    /// now, so merging with an older view would bias toward files the user has
    /// already moved on from.
    pub(crate) fn replace(&mut self, symbols: Vec<String>) -> usize {
        self.replace_at(symbols, Instant::now())
    }

    /// Injected clock so expiry is testable without sleeping.
    fn replace_at(&mut self, symbols: Vec<String>, now: Instant) -> usize {
        let mut kept: Vec<String> = Vec::new();
        for symbol in symbols {
            let trimmed = symbol.trim();
            if trimmed.is_empty() || trimmed.chars().count() > MAX_SYMBOL_CHARS {
                continue;
            }
            // Case-insensitive dedupe, matching how the glossary merge treats
            // the project index, so the two cannot disagree about duplicates.
            if kept.iter().any(|k| k.eq_ignore_ascii_case(trimmed)) {
                continue;
            }
            kept.push(trimmed.to_string());
            if kept.len() == MAX_SYMBOLS {
                break;
            }
        }
        let count = kept.len();
        self.symbols = kept;
        self.updated = Some(now);
        count
    }

    /// Symbols still fresh enough to bias the decoder.
    pub(crate) fn fresh(&self) -> &[String] {
        self.fresh_at(Instant::now())
    }

    fn fresh_at(&self, now: Instant) -> &[String] {
        match self.updated {
            Some(at) if now.duration_since(at) < TTL => &self.symbols,
            _ => &[],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(symbols: &[&str]) -> EditorContext {
        let mut c = EditorContext::default();
        c.replace(symbols.iter().map(|s| s.to_string()).collect());
        c
    }

    #[test]
    fn nothing_is_offered_before_an_editor_reports_anything() {
        assert!(EditorContext::default().fresh().is_empty());
    }

    #[test]
    fn symbols_round_trip_and_report_their_count() {
        let mut c = EditorContext::default();
        let kept = c.replace(vec!["initializeServer".into(), "serverOptions".into()]);
        assert_eq!(kept, 2);
        assert_eq!(c.fresh(), ["initializeServer", "serverOptions"]);
    }

    #[test]
    fn an_update_replaces_rather_than_accumulates() {
        // The editor reports what is on screen now; keeping the old view would
        // bias toward a file the user already left.
        let mut c = ctx(&["oldSymbol"]);
        c.replace(vec!["newSymbol".into()]);
        assert_eq!(c.fresh(), ["newSymbol"]);
    }

    #[test]
    fn blank_overlong_and_duplicate_entries_are_dropped() {
        let long = "x".repeat(MAX_SYMBOL_CHARS + 1);
        let c = ctx(&["  keep  ", "", "   ", &long, "KEEP", "other"]);
        assert_eq!(c.fresh(), ["keep", "other"]);
    }

    #[test]
    fn the_symbol_count_is_capped() {
        let many: Vec<String> = (0..MAX_SYMBOLS + 50).map(|i| format!("sym{i}")).collect();
        let mut c = EditorContext::default();
        assert_eq!(c.replace(many), MAX_SYMBOLS);
        assert_eq!(c.fresh().len(), MAX_SYMBOLS);
    }

    #[test]
    fn context_goes_stale_so_a_dead_editor_stops_biasing() {
        let mut c = EditorContext::default();
        let long_ago = Instant::now() - (TTL + Duration::from_secs(1));
        c.replace_at(vec!["staleSymbol".into()], long_ago);
        assert!(
            c.fresh().is_empty(),
            "an editor that stopped reporting kept biasing the decoder"
        );
    }

    #[test]
    fn context_just_inside_the_ttl_still_counts() {
        let mut c = EditorContext::default();
        let recent = Instant::now() - (TTL - Duration::from_secs(5));
        c.replace_at(vec!["liveSymbol".into()], recent);
        assert_eq!(c.fresh(), ["liveSymbol"]);
    }
}
