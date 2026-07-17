//! Session arming, stream keying, and device-config caching for low-latency
//! capture starts.
//!
//! `SessionState` is the contract between `AudioCapture` and the realtime
//! CPAL callback: while a warm stream sits idle between dictation sessions the
//! callback must discard every sample immediately (never buffer, never store),
//! and only an armed session accumulates audio. The flags are atomics so the
//! callback never takes a lock or allocates to decide.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

/// Fixed process epoch so the callback can compute elapsed time from a single
/// atomic load (`Instant` itself cannot live in an atomic).
static EPOCH: OnceLock<Instant> = OnceLock::new();

fn nanos_since_epoch() -> u64 {
    // u64 nanos overflow after ~584 years of process uptime.
    EPOCH.get_or_init(Instant::now).elapsed().as_nanos() as u64
}

/// Shared session flags checked by the realtime audio callback.
///
/// Lifecycle: `start_session` (timestamps the hotkey-to-audio window) →
/// `arm` (buffering allowed) → `disarm` (back to discarding). `on_delivery`
/// fires once per session, yielding the time-to-first-chunk metric.
pub(crate) struct SessionState {
    /// Buffer-vs-discard gate for the callback. False while idle-but-warm.
    armed: AtomicBool,
    /// Set at session start, cleared by the first armed delivery, so the
    /// TTFC metric is recorded exactly once per session.
    ttfc_pending: AtomicBool,
    /// Nanos since [`EPOCH`] when the session started (before device work,
    /// so a cold start's HAL queries count toward TTFC).
    session_start_nanos: AtomicU64,
    /// Whether this session reused an already-open warm stream (log field).
    warm: AtomicBool,
}

impl SessionState {
    pub(crate) fn new() -> Self {
        Self {
            armed: AtomicBool::new(false),
            ttfc_pending: AtomicBool::new(false),
            session_start_nanos: AtomicU64::new(0),
            warm: AtomicBool::new(false),
        }
    }

    /// Mark the beginning of a session, before any device selection or
    /// stream work, so TTFC measures everything the user actually waits for.
    pub(crate) fn start_session(&self) {
        self.session_start_nanos
            .store(nanos_since_epoch(), Ordering::Relaxed);
        self.ttfc_pending.store(true, Ordering::Relaxed);
    }

    /// Allow the callback to buffer samples. `warm` records whether the
    /// session reused an open stream (surfaces in the TTFC log line).
    pub(crate) fn arm(&self, warm: bool) {
        self.warm.store(warm, Ordering::Relaxed);
        self.armed.store(true, Ordering::Release);
    }

    /// Return the callback to discard mode (idle-but-warm, or stopped).
    pub(crate) fn disarm(&self) {
        self.armed.store(false, Ordering::Release);
    }

    pub(crate) fn is_armed(&self) -> bool {
        self.armed.load(Ordering::Acquire)
    }

    /// Record the first delivery of the current session. Returns
    /// `Some((ttfc_ms, warm))` exactly once per session, `None` afterwards.
    /// Called from the realtime callback: one swap + one subtraction, no lock.
    pub(crate) fn on_delivery(&self) -> Option<(u64, bool)> {
        if !self.ttfc_pending.swap(false, Ordering::Relaxed) {
            return None;
        }
        let start = self.session_start_nanos.load(Ordering::Relaxed);
        let elapsed_ms = nanos_since_epoch().saturating_sub(start) / 1_000_000;
        Some((elapsed_ms, self.warm.load(Ordering::Relaxed)))
    }
}

/// Identity of a capture stream configuration: which device the user asked
/// for and whether echo cancellation was requested. A cached config or an
/// idle warm stream is only reusable when the key matches exactly.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StreamKey {
    device: String,
    echo_cancellation: bool,
}

impl StreamKey {
    /// Blank or whitespace-only preferred names mean the system default,
    /// matching `select_input_device`'s interpretation.
    pub(crate) fn new(preferred_device: Option<&str>, echo_cancellation: bool) -> Self {
        let device = preferred_device
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .unwrap_or("default")
            .to_string();
        Self {
            device,
            echo_cancellation,
        }
    }
}

/// Single-entry cache of the resolved device + config for a [`StreamKey`],
/// so repeat opens skip the CPAL HAL queries (device enumeration and
/// supported-config scans cost tens of milliseconds per open). One entry is
/// enough: users dictate on one device at a time, and a key change simply
/// re-queries once. Generic over the value so the keying/invalidation logic
/// is testable without audio hardware.
pub(crate) struct ConfigCache<V> {
    entry: Option<(StreamKey, V)>,
}

impl<V> ConfigCache<V> {
    pub(crate) fn new() -> Self {
        Self { entry: None }
    }

    /// Remove and return the cached value when the key matches. The caller
    /// re-`store`s it after a successful open; an open failure simply never
    /// stores it back, which is the self-heal invalidation.
    pub(crate) fn take(&mut self, key: &StreamKey) -> Option<V> {
        match self.entry.take() {
            Some((cached_key, value)) if cached_key == *key => Some(value),
            // A mismatched entry is stale (device or echo-cancellation input
            // changed); drop it rather than keep a config nothing will use.
            _ => None,
        }
    }

    pub(crate) fn store(&mut self, key: StreamKey, value: V) {
        self.entry = Some((key, value));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_key_normalizes_blank_names_to_default() {
        assert_eq!(StreamKey::new(None, false), StreamKey::new(Some(""), false));
        assert_eq!(
            StreamKey::new(None, false),
            StreamKey::new(Some("   "), false)
        );
        assert_eq!(
            StreamKey::new(Some(" Mic A "), false),
            StreamKey::new(Some("Mic A"), false)
        );
    }

    #[test]
    fn stream_key_distinguishes_device_and_echo_cancellation() {
        assert_ne!(
            StreamKey::new(Some("Mic A"), false),
            StreamKey::new(Some("Mic B"), false)
        );
        assert_ne!(StreamKey::new(None, false), StreamKey::new(None, true));
    }

    #[test]
    fn cache_hits_only_on_matching_key() {
        let mut cache = ConfigCache::new();
        let key = StreamKey::new(Some("Mic A"), false);
        cache.store(key.clone(), 7u32);
        assert_eq!(cache.take(&key), Some(7));
        // take removes the entry; a second lookup misses until re-stored.
        assert_eq!(cache.take(&key), None);
    }

    #[test]
    fn cache_invalidates_on_device_or_echo_cancellation_change() {
        let mut cache = ConfigCache::new();
        cache.store(StreamKey::new(Some("Mic A"), false), 7u32);
        // Same device, echo cancellation flipped: stale, dropped entirely.
        assert_eq!(cache.take(&StreamKey::new(Some("Mic A"), true)), None);
        assert_eq!(cache.take(&StreamKey::new(Some("Mic A"), false)), None);

        cache.store(StreamKey::new(Some("Mic A"), false), 7u32);
        // Different device: also stale.
        assert_eq!(cache.take(&StreamKey::new(Some("Mic B"), false)), None);
        assert_eq!(cache.take(&StreamKey::new(Some("Mic A"), false)), None);
    }

    #[test]
    fn unarmed_session_discards_without_recording_ttfc() {
        let state = SessionState::new();
        let mut buffered: Vec<f32> = Vec::new();

        // Fake callback mirroring the real one's decision path.
        let deliver = |data: &[f32], buf: &mut Vec<f32>| {
            if !state.is_armed() {
                return None;
            }
            let ttfc = state.on_delivery();
            buf.extend_from_slice(data);
            ttfc
        };

        state.start_session();
        // Idle-but-warm: nothing buffered, no TTFC consumed.
        assert_eq!(deliver(&[0.1, 0.2], &mut buffered), None);
        assert!(buffered.is_empty());

        // Arming lets audio through; the pending TTFC fires on first delivery.
        state.arm(true);
        let first = deliver(&[0.3], &mut buffered);
        assert!(matches!(first, Some((_, true))));
        assert_eq!(buffered, vec![0.3]);

        // Subsequent deliveries buffer but never re-record TTFC.
        assert_eq!(deliver(&[0.4], &mut buffered), None);
        assert_eq!(buffered, vec![0.3, 0.4]);

        // Disarmed again: back to discarding.
        state.disarm();
        assert_eq!(deliver(&[0.5], &mut buffered), None);
        assert_eq!(buffered, vec![0.3, 0.4]);
    }

    #[test]
    fn ttfc_fires_once_per_session_and_resets_on_next_session() {
        let state = SessionState::new();

        state.start_session();
        state.arm(false);
        let first = state.on_delivery();
        assert!(matches!(first, Some((_, false))));
        assert_eq!(state.on_delivery(), None);
        state.disarm();

        // A new session re-arms the metric, now flagged as warm.
        state.start_session();
        state.arm(true);
        assert!(matches!(state.on_delivery(), Some((_, true))));
        assert_eq!(state.on_delivery(), None);
    }

    #[test]
    fn arming_without_a_session_start_yields_no_ttfc() {
        // A prewarmed stream is opened disarmed with no session pending; if it
        // were ever armed without start_session, no stale metric may fire.
        let state = SessionState::new();
        state.arm(false);
        assert_eq!(state.on_delivery(), None);
    }
}
