//! The one place keystroke simulators are created.

use std::sync::atomic::{AtomicBool, Ordering};

use enigo::{Enigo, NewConError, Settings};

static PERMISSION_PROMPTED: AtomicBool = AtomicBool::new(false);

/// A keystroke simulator for typing and paste shortcuts.
///
/// # Errors
/// [`NewConError::NoPermission`] on macOS while the app (or, for a dev build,
/// the terminal that launched it) lacks Accessibility permission. Only the
/// first attempt in a run asks macOS to show its permission dialog; output
/// falls back to the clipboard, and re-asking on every phrase would open the
/// dialog each time the user speaks.
pub fn new_simulator() -> Result<Enigo, NewConError> {
    Enigo::new(&settings(&PERMISSION_PROMPTED))
}

fn settings(prompted: &AtomicBool) -> Settings {
    Settings {
        open_prompt_to_get_permissions: !prompted.swap(true, Ordering::Relaxed),
        ..Settings::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_first_simulator_may_open_the_permission_dialog() {
        let prompted = AtomicBool::new(false);
        assert!(settings(&prompted).open_prompt_to_get_permissions);
        assert!(!settings(&prompted).open_prompt_to_get_permissions);
        assert!(!settings(&prompted).open_prompt_to_get_permissions);
    }

    #[test]
    fn other_settings_keep_their_defaults() {
        let prompted = AtomicBool::new(true);
        let expected = Settings {
            open_prompt_to_get_permissions: false,
            ..Settings::default()
        };
        assert_eq!(settings(&prompted), expected);
    }
}
