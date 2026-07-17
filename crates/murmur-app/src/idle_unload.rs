//! Idle model auto-unload: a background watcher frees the STT engine (and the
//! LLM rewrite engine) after a configurable idle period, so a model the user
//! is not dictating with stops holding hundreds of megabytes of RAM. The next
//! activation reloads it through the same on-demand path as the startup load.

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use tauri::Manager;

use crate::state::AppState;

/// Watcher cadence. Coarse on purpose: the setting's minimum is 60 s, so a
/// 30 s tick bounds overshoot to half the smallest idle window.
const TICK: Duration = Duration::from_secs(30);

/// Idle-unload policy, pure so it is testable without app state: unload only
/// when the feature is on (non-zero), an engine is actually loaded, no
/// session is active, and the idle window has fully elapsed.
fn should_unload(
    now: Instant,
    last_activity: Instant,
    recording: bool,
    loaded: bool,
    idle_unload_secs: u64,
) -> bool {
    idle_unload_secs != 0
        && loaded
        && !recording
        && now.duration_since(last_activity) >= Duration::from_secs(idle_unload_secs)
}

/// Reset the idle clock. Called wherever model activity ends: after an
/// inference, at session end, after a rewrite, and when an engine loads.
pub(crate) fn touch(state: &AppState) {
    *state
        .last_activity
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = Instant::now();
}

/// Spawn the watcher thread (mirrors `dictation_trigger::spawn`'s pattern).
pub(crate) fn spawn(app: tauri::AppHandle) {
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(TICK);
            let Some(state) = app.try_state::<AppState>() else {
                continue;
            };
            let idle_secs = state
                .settings
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .model_idle_unload_secs;
            if idle_secs == 0 {
                continue;
            }
            let last = *state
                .last_activity
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let now = Instant::now();
            unload_stt_if_idle(&state, now, last, idle_secs);
            #[cfg(feature = "llm")]
            unload_llm_if_idle(&state, now, last, idle_secs);
        }
    });
}

fn unload_stt_if_idle(state: &AppState, now: Instant, last: Instant, idle_secs: u64) {
    // Decide and detach under the `recording` lock: a toggle claims
    // `recording` under this same lock before it checks `engine_loaded`, so
    // clearing the flag here can never race a session into a missing engine.
    // The engine mutex is only try-locked (wait-free) inside: a held engine
    // mutex means an inference is still running — that IS activity, so the
    // tick is skipped rather than waited out while blocking toggles.
    let taken = {
        let recording = state.recording.lock().unwrap_or_else(|e| e.into_inner());
        let loaded = state.engine_loaded.load(Ordering::Acquire);
        if !should_unload(now, last, *recording, loaded, idle_secs) {
            return;
        }
        let mut engine = match state.engine.try_lock() {
            Ok(guard) => guard,
            Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => return,
        };
        let taken = engine.take();
        state.engine_loaded.store(false, Ordering::Release);
        state.idle_unloaded.store(true, Ordering::Release);
        taken
    };
    // Drop outside both locks: whisper/ORT teardown can take a moment and
    // must never block a toggle or the reload the next activation kicks.
    if let Some(engine) = taken {
        drop(engine);
        tracing::info!(
            idle_secs,
            "Unloaded idle STT model; it reloads on the next dictation"
        );
    }
}

/// Unload the lazily loaded rewrite LLM under the same idle policy. It has no
/// readiness flag: presence in the slot is its load state, and the rewrite
/// path reloads it on next use. `recording` is irrelevant to rewrites; an
/// in-flight rewrite holds the slot mutex for the whole inference, so a
/// failed try_lock means the engine is busy (= active) and the tick is skipped.
#[cfg(feature = "llm")]
fn unload_llm_if_idle(state: &AppState, now: Instant, last: Instant, idle_secs: u64) {
    if !should_unload(now, last, false, true, idle_secs) {
        return;
    }
    let taken = {
        let mut slot = match state.llm.try_lock() {
            Ok(guard) => guard,
            Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => return,
        };
        slot.take()
    };
    if let Some(engine) = taken {
        // Same rationale as the STT path: drop outside the lock.
        drop(engine);
        tracing::info!(
            idle_secs,
            "Unloaded idle rewrite model; it reloads on the next rewrite"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const IDLE: u64 = 300;

    /// Build (now, last_activity) that are `idle_for_secs` apart by ADDING to
    /// a base instant: subtracting from `Instant::now()` panics on a
    /// freshly booted machine (CI runners), where the clock's epoch is only
    /// seconds in the past.
    fn instants(idle_for_secs: u64) -> (Instant, Instant) {
        let last = Instant::now();
        (last + Duration::from_secs(idle_for_secs), last)
    }

    #[test]
    fn unloads_only_after_the_full_idle_window() {
        let (now, last) = instants(IDLE);
        assert!(should_unload(now, last, false, true, IDLE));

        let (now, last) = instants(IDLE - 1);
        assert!(!should_unload(now, last, false, true, IDLE));
    }

    #[test]
    fn zero_setting_means_never() {
        // A day of idleness, far past any real window.
        let (now, last) = instants(86_400);
        assert!(!should_unload(now, last, false, true, 0));
    }

    #[test]
    fn active_session_blocks_the_unload() {
        let (now, last) = instants(IDLE * 4);
        assert!(!should_unload(now, last, true, true, IDLE));
    }

    #[test]
    fn nothing_loaded_means_nothing_to_unload() {
        let (now, last) = instants(IDLE * 4);
        assert!(!should_unload(now, last, false, false, IDLE));
    }

    #[test]
    fn fresh_activity_resets_the_clock() {
        let now = Instant::now();
        // touch() semantics: last_activity == now → not idle.
        assert!(!should_unload(now, now, false, true, IDLE));
    }
}
