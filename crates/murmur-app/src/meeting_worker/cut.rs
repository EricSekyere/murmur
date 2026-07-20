//! Chunk cut-point selection for the meeting worker: pure, so it unit-tests
//! on synthetic vectors without audio devices or models.

/// The cut point is the quietest window in this much trailing audio...
const CUT_SEARCH_SECS: usize = 5;
/// ...scanned with an RMS window this long (no VAD dependency).
const CUT_WINDOW_MS: usize = 200;

/// Pick the chunk cut index: the end of the quietest [`CUT_WINDOW_MS`] RMS
/// window inside the last [`CUT_SEARCH_SECS`] of the (hard-capped) chunk
/// region, so the cut lands in a speech pause instead of mid-word.
pub(super) fn select_cut_index(pending: &[f32], rate: usize) -> usize {
    let region_end = pending.len().min(super::HARD_CAP_SECS * rate);
    let window = (CUT_WINDOW_MS * rate) / 1000;
    let search_start = region_end.saturating_sub(CUT_SEARCH_SECS * rate);
    if region_end <= search_start + window {
        return region_end;
    }

    // Step at a quarter window: fine enough to find a 200 ms pause, cheap
    // enough to stay negligible next to inference.
    let step = (window / 4).max(1);
    let mut best_end = region_end;
    let mut best_energy = f32::INFINITY;
    let mut start = search_start;
    while start + window <= region_end {
        let energy: f32 = pending[start..start + window].iter().map(|s| s * s).sum();
        if energy < best_energy {
            best_energy = energy;
            best_end = start + window;
        }
        start += step;
    }
    best_end
}

#[cfg(test)]
mod tests {
    use super::super::HARD_CAP_SECS;
    use super::*;

    #[test]
    fn cut_prefers_the_quietest_window() {
        let rate = 1_000; // small synthetic rate keeps vectors tiny
        // 22 "seconds" of loud audio with one quiet dip inside the last 5.
        let mut pending = vec![0.5_f32; 22 * rate];
        let dip_start = 19 * rate;
        let dip_len = (CUT_WINDOW_MS * rate) / 1000;
        for s in &mut pending[dip_start..dip_start + dip_len] {
            *s = 0.0;
        }
        let cut = select_cut_index(&pending, rate);
        assert_eq!(cut, dip_start + dip_len, "cut must end at the quiet dip");
    }

    #[test]
    fn cut_on_uniform_audio_lands_in_the_search_region() {
        let rate = 1_000;
        let pending = vec![0.3_f32; 22 * rate];
        let cut = select_cut_index(&pending, rate);
        assert!(cut > 17 * rate && cut <= 22 * rate, "cut {cut}");
    }

    #[test]
    fn cut_never_exceeds_the_hard_cap() {
        let rate = 1_000;
        // A stalled loop let 40 "seconds" accumulate; the cut must stay
        // within the 30-second cap.
        let pending = vec![0.3_f32; 40 * rate];
        let cut = select_cut_index(&pending, rate);
        assert!(cut <= HARD_CAP_SECS * rate, "cut {cut}");
        assert!(cut > (HARD_CAP_SECS - CUT_SEARCH_SECS) * rate);
    }

    #[test]
    fn short_input_cut_stays_in_bounds() {
        let rate = 1_000;
        // Shorter than the search region: the scan covers the whole input
        // and the cut must stay a valid, non-empty index. (The final flush
        // itself bypasses the scan and drains everything — see
        // `finish_meeting`.)
        let pending = vec![0.2_f32; 2 * rate];
        let cut = select_cut_index(&pending, rate);
        assert!(cut > 0 && cut <= pending.len(), "cut {cut}");
        assert_eq!(select_cut_index(&[], rate), 0);
    }
}
