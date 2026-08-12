//! In-memory store of recent utterance audio so a history entry can be
//! re-transcribed. RAM only, by design: nothing in this module may ever write
//! audio to disk, under any setting (see PRIVACY.md). The store empties itself
//! on app exit, and is cleared explicitly on Clear History and when the user
//! turns Save History off.

use std::collections::VecDeque;

/// Count bound: at most this many recent utterances are kept.
const MAX_UTTERANCES: usize = 10;
/// Duration bound: total retained audio may not exceed this many seconds.
/// Whichever bound binds first evicts the oldest utterance.
const MAX_TOTAL_SECS: f32 = 60.0;

/// One retained utterance, keyed by the history entry it produced.
pub(crate) struct RetainedUtterance {
    pub entry_id: String,
    pub samples: Vec<f32>,
    pub sample_rate: u32,
}

impl RetainedUtterance {
    fn duration_secs(&self) -> f32 {
        if self.sample_rate == 0 {
            return 0.0;
        }
        self.samples.len() as f32 / self.sample_rate as f32
    }
}

/// Bounded queue of recent utterance buffers, oldest first.
#[derive(Default)]
pub(crate) struct RetainedAudio {
    buffers: VecDeque<RetainedUtterance>,
}

impl RetainedAudio {
    /// Keep an utterance, evicting the oldest until both bounds hold. The
    /// newest utterance always survives, even if it alone exceeds the
    /// duration bound: evicting it would make retention silently useless.
    pub(crate) fn retain(&mut self, entry_id: String, samples: Vec<f32>, sample_rate: u32) {
        self.buffers.push_back(RetainedUtterance {
            entry_id,
            samples,
            sample_rate,
        });
        while self.buffers.len() > 1
            && (self.buffers.len() > MAX_UTTERANCES || self.total_secs() > MAX_TOTAL_SECS)
        {
            self.buffers.pop_front();
        }
    }

    pub(crate) fn get(&self, entry_id: &str) -> Option<&RetainedUtterance> {
        (!entry_id.is_empty()).then(|| self.buffers.iter().find(|u| u.entry_id == entry_id))?
    }

    pub(crate) fn contains(&self, entry_id: &str) -> bool {
        self.get(entry_id).is_some()
    }

    pub(crate) fn clear(&mut self) {
        self.buffers.clear();
    }

    fn total_secs(&self) -> f32 {
        self.buffers
            .iter()
            .map(RetainedUtterance::duration_secs)
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 16_000;

    fn secs(n: f32) -> Vec<f32> {
        vec![0.0; (n * RATE as f32) as usize]
    }

    #[test]
    fn count_bound_evicts_oldest() {
        let mut r = RetainedAudio::default();
        for i in 0..(MAX_UTTERANCES + 3) {
            r.retain(format!("id-{i}"), secs(1.0), RATE);
        }
        assert_eq!(r.buffers.len(), MAX_UTTERANCES);
        assert!(!r.contains("id-0"));
        assert!(!r.contains("id-2"));
        assert!(r.contains("id-3"));
        assert!(r.contains(&format!("id-{}", MAX_UTTERANCES + 2)));
    }

    #[test]
    fn duration_bound_evicts_before_count_bound() {
        let mut r = RetainedAudio::default();
        // Four 25s utterances: 100s total, well under the count bound, so the
        // duration bound must do the evicting (down to 50s = two entries).
        for i in 0..4 {
            r.retain(format!("id-{i}"), secs(25.0), RATE);
        }
        assert_eq!(r.buffers.len(), 2);
        assert!(!r.contains("id-0"));
        assert!(!r.contains("id-1"));
        assert!(r.contains("id-2"));
        assert!(r.contains("id-3"));
        assert!(r.total_secs() <= MAX_TOTAL_SECS);
    }

    #[test]
    fn newest_utterance_survives_even_when_oversized() {
        let mut r = RetainedAudio::default();
        r.retain("small".into(), secs(1.0), RATE);
        r.retain("huge".into(), secs(MAX_TOTAL_SECS + 30.0), RATE);
        assert!(!r.contains("small"));
        assert!(r.contains("huge"));
        assert_eq!(r.buffers.len(), 1);
    }

    #[test]
    fn clear_forgets_everything() {
        let mut r = RetainedAudio::default();
        r.retain("a".into(), secs(1.0), RATE);
        r.clear();
        assert!(!r.contains("a"));
        assert_eq!(r.buffers.len(), 0);
    }

    #[test]
    fn empty_id_never_matches() {
        let mut r = RetainedAudio::default();
        // A pre-id history entry has an empty id; it must not accidentally
        // match an (invalid) empty-keyed buffer.
        r.retain(String::new(), secs(1.0), RATE);
        assert!(!r.contains(""));
        assert!(r.get("").is_none());
    }
}
