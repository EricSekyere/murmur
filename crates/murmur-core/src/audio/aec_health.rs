//! Per-run health tracking for the OS voice-capture (echo cancellation) path.
//!
//! Attempting that path is not free: a failed or silent start costs the
//! session a bounded probe wait (hundreds of milliseconds) before falling
//! back to the raw microphone. Once the path has proven unusable this run,
//! later sessions skip the attempt entirely and go straight to CPAL. One
//! demotion state, two triggers: a start failure demotes immediately (an
//! endpoint that failed to open will not fix itself this run), and silent
//! probes demote after a small budget (the user may simply not have spoken
//! yet). A demotion resets when the observed capture configuration changes
//! (device or echo-cancellation setting) or on app restart (new instance).
//!
//! Pure state, no hardware, so the demotion rules are unit-testable.

use super::warm::StreamKey;

/// Give up on the AEC path for the run after this many consecutive probes
/// that saw only silence. A single silent probe can just mean the user had
/// not spoken yet, so it only skips AEC for that one session.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) const AEC_MAX_SILENT_PROBES: u8 = 3;

/// Outcome of recording one silent probe.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ProbeVerdict {
    /// Use the raw mic for this session, but attempt AEC again next session.
    RetryNextSession,
    /// The probe budget is spent: AEC is demoted for the rest of the run.
    Demoted,
}

/// Demotion state of the voice-capture path for this run.
pub(crate) struct AecHealth {
    /// Skip the AEC attempt entirely until a reset trigger fires.
    demoted: bool,
    /// The path delivered real signal this run; no further probing needed.
    proven: bool,
    silent_probes: u8,
    /// Capture config seen at the last session start / pre-warm. AEC itself
    /// always captures the default device, so this never varies while AEC is
    /// in use — observing every config is what lets a device or
    /// echo-cancellation setting change (the reset triggers) show up as a
    /// key change even when the new config does not route to AEC.
    observed_key: Option<StreamKey>,
}

impl AecHealth {
    pub(crate) fn new() -> Self {
        Self {
            demoted: false,
            proven: false,
            silent_probes: 0,
            observed_key: None,
        }
    }

    /// Record the capture config this session (or pre-warm) runs under. A
    /// change re-arms the path — the demotion belonged to the old config.
    /// Returns true when a previously demoted path was re-enabled.
    pub(crate) fn observe(&mut self, key: &StreamKey) -> bool {
        let changed = self.observed_key.as_ref().is_some_and(|k| k != key);
        if self.observed_key.as_ref() != Some(key) {
            self.observed_key = Some(key.clone());
        }
        if !changed {
            return false;
        }
        let was_demoted = self.demoted;
        self.demoted = false;
        self.proven = false;
        self.silent_probes = 0;
        was_demoted
    }

    pub(crate) fn is_demoted(&self) -> bool {
        self.demoted
    }

    /// Whether a successful open still needs its first-signal probe.
    #[cfg_attr(not(any(windows, target_os = "linux")), allow(dead_code))]
    pub(crate) fn needs_probe(&self) -> bool {
        !self.proven && !self.demoted
    }

    /// The path delivered real signal: trust it for the rest of the run.
    #[cfg_attr(not(any(windows, target_os = "linux")), allow(dead_code))]
    pub(crate) fn on_signal(&mut self) {
        self.proven = true;
        self.silent_probes = 0;
    }

    /// The path failed to start (open error, dead subprocess, or no
    /// implementation on this platform): demote immediately.
    pub(crate) fn on_start_failure(&mut self) {
        self.demoted = true;
    }

    /// A probe saw only silence: retry next session until the budget is
    /// spent, then demote.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub(crate) fn on_silent_probe(&mut self) -> ProbeVerdict {
        self.silent_probes = self.silent_probes.saturating_add(1);
        if self.silent_probes >= AEC_MAX_SILENT_PROBES {
            self.demoted = true;
            ProbeVerdict::Demoted
        } else {
            ProbeVerdict::RetryNextSession
        }
    }

    /// Silent probes consumed so far (log field).
    #[cfg_attr(not(windows), allow(dead_code))]
    pub(crate) fn silent_probes(&self) -> u8 {
        self.silent_probes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_aec_key() -> StreamKey {
        // AEC only runs with echo cancellation on the default device.
        StreamKey::new(None, true)
    }

    #[test]
    fn fresh_state_attempts_and_probes() {
        let health = AecHealth::new();
        assert!(!health.is_demoted());
        assert!(health.needs_probe());
    }

    #[test]
    fn start_failure_demotes_immediately_and_skips_stick() {
        let mut health = AecHealth::new();
        health.observe(&default_aec_key());
        health.on_start_failure();
        assert!(health.is_demoted());
        assert!(!health.needs_probe());
        // Re-observing the same config must not re-enable the attempt.
        assert!(!health.observe(&default_aec_key()));
        assert!(health.is_demoted());
    }

    #[test]
    fn silent_probes_retry_until_the_budget_is_spent() {
        let mut health = AecHealth::new();
        health.observe(&default_aec_key());
        for _ in 1..AEC_MAX_SILENT_PROBES {
            assert_eq!(health.on_silent_probe(), ProbeVerdict::RetryNextSession);
            assert!(!health.is_demoted());
            assert!(health.needs_probe());
        }
        assert_eq!(health.on_silent_probe(), ProbeVerdict::Demoted);
        assert!(health.is_demoted());
    }

    #[test]
    fn signal_proves_the_path_and_resets_the_probe_count() {
        let mut health = AecHealth::new();
        health.observe(&default_aec_key());
        assert_eq!(health.on_silent_probe(), ProbeVerdict::RetryNextSession);
        health.on_signal();
        assert!(!health.is_demoted());
        // Proven: no further probing, and the silent streak restarted.
        assert!(!health.needs_probe());
        assert_eq!(health.silent_probes(), 0);
    }

    #[test]
    fn device_change_resets_a_demotion() {
        let mut health = AecHealth::new();
        health.observe(&default_aec_key());
        health.on_start_failure();
        assert!(health.is_demoted());

        // The user picks a named mic (which never routes to AEC)...
        assert!(health.observe(&StreamKey::new(Some("Mic B"), true)));
        assert!(!health.is_demoted());
        // ...and back to the default: the path gets a fresh attempt + probe.
        health.observe(&default_aec_key());
        assert!(!health.is_demoted());
        assert!(health.needs_probe());
    }

    #[test]
    fn echo_cancellation_toggle_resets_a_demotion() {
        let mut health = AecHealth::new();
        health.observe(&default_aec_key());
        health.on_start_failure();
        assert!(health.is_demoted());

        // Setting off, then on again: both are key changes.
        assert!(health.observe(&StreamKey::new(None, false)));
        health.observe(&default_aec_key());
        assert!(!health.is_demoted());
        assert!(health.needs_probe());
    }

    #[test]
    fn observe_reports_reenable_only_from_a_demoted_state() {
        let mut health = AecHealth::new();
        // First observation records the key without any reset.
        assert!(!health.observe(&default_aec_key()));
        // A key change without a demotion resets silently.
        assert!(!health.observe(&StreamKey::new(Some("Mic B"), true)));
    }
}
