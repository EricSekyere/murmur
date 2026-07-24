//! End-of-session auto-submit ("send the message for me").
//!
//! When a dictation session ends and the target app's profile opts in,
//! Murmur presses Enter (or Ctrl+Enter) once so a dictated chat message goes
//! out fully hands-free. Pure decision logic only — the app layer verifies
//! the environment (focus, deliveries, output mode) and performs the
//! keystroke. Deliberately per-app-profile only: a global Enter is
//! destructive in the wrong window.

use crate::config::settings::AutoSubmit;

/// Environment conditions the app layer verifies; the decision trusts them
/// and skips the submit when any is false.
#[derive(Debug, Clone, Copy)]
pub struct SubmitGates {
    /// The session ended normally (auto-stop or hotkey stop), not through a
    /// streaming error or crash.
    pub ended_normally: bool,
    /// At least one phrase was delivered during the session. A session that
    /// delivered nothing must not submit an empty or stale message.
    pub delivered_phrase: bool,
    /// The output mode places text in the target (Auto, Keyboard, or
    /// ClipboardPaste). Clipboard-only and stdout put nothing there, so
    /// there is nothing to send.
    pub places_text: bool,
    /// The foreground window at submit time is the window the session
    /// delivered into (same-hwnd discipline as junction repair).
    pub same_target: bool,
}

/// The keystroke to press once at session end, or `None` to skip silently.
pub fn submit_action(configured: Option<AutoSubmit>, gates: &SubmitGates) -> Option<AutoSubmit> {
    let submit = configured?;
    (gates.ended_normally && gates.delivered_phrase && gates.places_text && gates.same_target)
        .then_some(submit)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open_gates() -> SubmitGates {
        SubmitGates {
            ended_normally: true,
            delivered_phrase: true,
            places_text: true,
            same_target: true,
        }
    }

    #[test]
    fn no_profile_option_never_submits() {
        assert_eq!(submit_action(None, &open_gates()), None);
    }

    #[test]
    fn all_gates_open_returns_the_configured_keystroke() {
        assert_eq!(
            submit_action(Some(AutoSubmit::Enter), &open_gates()),
            Some(AutoSubmit::Enter)
        );
        assert_eq!(
            submit_action(Some(AutoSubmit::CtrlEnter), &open_gates()),
            Some(AutoSubmit::CtrlEnter)
        );
    }

    #[test]
    fn each_gate_individually_blocks() {
        let flips: [fn(&mut SubmitGates); 4] = [
            |g| g.ended_normally = false,
            |g| g.delivered_phrase = false,
            |g| g.places_text = false,
            |g| g.same_target = false,
        ];
        for (i, flip) in flips.iter().enumerate() {
            let mut gates = open_gates();
            flip(&mut gates);
            assert_eq!(
                submit_action(Some(AutoSubmit::Enter), &gates),
                None,
                "gate {i} should block"
            );
        }
    }
}
