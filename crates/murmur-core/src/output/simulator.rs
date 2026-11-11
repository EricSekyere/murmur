//! The one place keystroke simulators are created and driven.

use std::sync::atomic::{AtomicBool, Ordering};

use enigo::{Enigo, NewConError, Settings};

static PERMISSION_PROMPTED: AtomicBool = AtomicBool::new(false);

/// How long keystrokes wait for a busy main thread before output falls back.
#[cfg(target_os = "macos")]
const MAIN_THREAD_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

/// Run `keystrokes` against a fresh simulator: on the main thread on macOS,
/// where enigo's key mapping traps anywhere else (see `output::main_thread`),
/// and inline on other platforms.
///
/// # Errors
/// [`NewConError::NoPermission`] on macOS while the app (or, for a dev build,
/// the terminal that launched it) lacks Accessibility permission; whatever
/// `keystrokes` returns; and on macOS, a main thread too busy to take the work.
pub fn with_simulator<R, F>(keystrokes: F) -> anyhow::Result<R>
where
    F: FnOnce(&mut Enigo) -> anyhow::Result<R> + Send + 'static,
    R: Send + 'static,
{
    let task = move || keystrokes(&mut new_simulator()?);
    #[cfg(target_os = "macos")]
    {
        super::main_thread::run(task, MAIN_THREAD_WAIT)?
    }
    #[cfg(not(target_os = "macos"))]
    {
        task()
    }
}

/// Only the first attempt in a run asks macOS to show its Accessibility
/// dialog; output falls back to the clipboard, and re-asking on every phrase
/// would open the dialog each time the user speaks.
fn new_simulator() -> Result<Enigo, NewConError> {
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
