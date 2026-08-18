//! App-side lifecycle for always-listening mode.
//!
//! Owns the arm/disarm policy: the user's `wake_word_enabled` setting is the
//! *intent*; the worker's own events are the *truth* about whether the mic is
//! open, and the UI renders only from those events. This module reconciles
//! the two (`sync`) and translates worker events into UI state and session
//! starts (`spawn_event_pump`).

use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::time::Duration;

use tauri::Manager;

use crate::audio_worker::WakeEvent;
use crate::state::{AppState, emit_hotkey_error};

/// Backoff schedule for automatic re-arm attempts after a failure (mic loss,
/// scorer breakage). Expected exits (disarm, session handoff) never retry.
const REARM_BACKOFF: [Duration; 3] = [
    Duration::from_secs(1),
    Duration::from_secs(5),
    Duration::from_secs(30),
];

/// What the worker has told us, which lags what it has been asked to do.
#[derive(Clone, Copy, Debug)]
struct WorkerWakeState {
    /// Confirmed by a `WakeEvent::Armed`.
    armed: bool,
    /// An arm is queued but unanswered.
    arm_pending: bool,
}

impl WorkerWakeState {
    /// An in-flight arm counts as armed: it will open the stream unless it is
    /// countermanded, so reconciliation must be willing to countermand it.
    fn effective(self) -> bool {
        self.armed || self.arm_pending
    }
}

#[derive(Debug, PartialEq)]
enum SyncAction {
    Arm,
    Disarm,
    Nothing,
}

/// Reconciliation decision, split from [`sync`] so the in-flight cases are
/// testable without a worker, a device, or a Tauri handle.
fn sync_action(desired: bool, worker: WorkerWakeState) -> SyncAction {
    if desired == worker.effective() {
        return SyncAction::Nothing;
    }
    if desired {
        SyncAction::Arm
    } else {
        SyncAction::Disarm
    }
}

/// Whether the worker should be armed right now.
fn should_be_armed(mode_enabled: bool, meeting_active: bool, session_active: bool) -> bool {
    mode_enabled && !meeting_active && !session_active
}

/// What to do with the wake stream after OS resume. The capture stream is
/// gone even if `currently_armed` is still true, so a live intent must
/// restart rather than trusting the flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResumeAction {
    /// Disarm (if needed) then arm again — stream may be dead.
    Restart,
    /// Close a leftover armed stream; do not re-open.
    Disarm,
    /// Nothing to do.
    None,
}

fn resume_action(
    mode_enabled: bool,
    meeting_active: bool,
    session_active: bool,
    currently_armed: bool,
) -> ResumeAction {
    let desired = should_be_armed(mode_enabled, meeting_active, session_active);
    if desired {
        ResumeAction::Restart
    } else if currently_armed {
        ResumeAction::Disarm
    } else {
        ResumeAction::None
    }
}

/// OS suspend tears the capture stream down. Do not trust `wake_armed`:
/// restart when the mode still wants the mic, otherwise drop a leftover arm.
pub(crate) fn on_resume(app: &tauri::AppHandle) {
    let Some(state) = app.try_state::<AppState>() else {
        tracing::debug!("OS resume: app state already gone");
        return;
    };
    let mode_enabled = state
        .settings
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .wake_word_enabled;
    let meeting_active = state.meeting_active.load(Ordering::Acquire);
    let session_active = *state.recording.lock().unwrap_or_else(|e| e.into_inner());
    let currently_armed = state.wake_armed.load(Ordering::Acquire);

    match resume_action(
        mode_enabled,
        meeting_active,
        session_active,
        currently_armed,
    ) {
        ResumeAction::Restart => {
            tracing::info!("OS resume: restarting wake stream");
            send_disarm(&state);
            state.wake_armed.store(false, Ordering::Release);
            sync(app);
        }
        ResumeAction::Disarm => {
            tracing::info!("OS resume: disarming wake stream");
            send_disarm(&state);
        }
        ResumeAction::None => {
            tracing::debug!("OS resume: wake stream not needed");
        }
    }
}

/// Fire-and-forget disarm so the capture stream closes before the worker is
/// dropped. Channel-closed is expected if the worker already exited.
pub(crate) fn on_exit(app: &tauri::AppHandle) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    if let Some(audio) = state.audio.get()
        && let Err(e) = audio.send_disarm()
    {
        tracing::debug!(error = %e, "wake disarm on exit skipped");
    }
}

fn send_disarm(state: &AppState) {
    if let Some(audio) = state.audio.get()
        && let Err(e) = audio.send_disarm()
    {
        tracing::error!("Failed to send disarm: {}", e);
    }
}

/// Delay before the next automatic re-arm attempt, or `None` when the exit
/// was expected or the attempts are exhausted.
fn retry_delay(failed: bool, attempts: u32) -> Option<Duration> {
    if !failed {
        return None;
    }
    REARM_BACKOFF.get(attempts as usize).copied()
}

/// Reconcile the worker's armed state with the current intent. Called from
/// settings changes, session end, meeting start/end, resume, and startup.
/// Idempotent: the worker ignores a redundant disarm and rejects a duplicate
/// arm on the new arm's own channel.
pub(crate) fn sync(app: &tauri::AppHandle) {
    let state = app.state::<AppState>();
    let mode_enabled = state
        .settings
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .wake_word_enabled;
    let meeting_active = state.meeting_active.load(Ordering::Acquire);
    let session_active = *state.recording.lock().unwrap_or_else(|e| e.into_inner());

    let desired = should_be_armed(mode_enabled, meeting_active, session_active);
    let worker = WorkerWakeState {
        armed: state.wake_armed.load(Ordering::Acquire),
        arm_pending: state.wake_arm_pending.load(Ordering::Acquire),
    };
    match sync_action(desired, worker) {
        SyncAction::Arm => arm(app, &state),
        SyncAction::Disarm => send_disarm(&state),
        SyncAction::Nothing => {}
    }
}

#[cfg(feature = "wake")]
fn arm(app: &tauri::AppHandle, state: &AppState) {
    use crate::audio_worker::ArmParams;

    // No echo-cancellation setting here: armed capture is always the raw mic,
    // so the arm never spends the AEC probe budget (see `start_passive`).
    let (threshold, audio_device) = {
        let settings = state.settings.lock().unwrap_or_else(|e| e.into_inner());
        (
            settings.wake_word_sensitivity.threshold(),
            settings.audio_device.clone(),
        )
    };
    let scorer = match build_scorer() {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("Wake scorer unavailable: {}", e);
            emit_hotkey_error(app, &format!("Always listening unavailable: {}", e));
            // The mode never half-enables: flip the setting off (and save)
            // so every future sync — session end, meeting end — doesn't
            // retry a build that cannot succeed and re-toast the error.
            disable_wake_setting(app, state);
            return;
        }
    };
    let Some(wake_tx) = state.wake_tx.get() else {
        tracing::error!("Wake event pump not started; cannot arm");
        return;
    };
    let Some(audio) = state.audio.get() else {
        tracing::error!("Audio worker not initialized; cannot arm");
        return;
    };
    // Set before the send so no reconciliation can observe the gap between
    // queueing the arm and the worker answering.
    state.wake_arm_pending.store(true, Ordering::Release);
    if let Err(e) = audio.send_arm(ArmParams {
        audio_device,
        threshold,
        wake_tx: wake_tx.clone(),
        scorer,
    }) {
        state.wake_arm_pending.store(false, Ordering::Release);
        tracing::error!("Failed to send arm: {}", e);
        emit_hotkey_error(app, &format!("Always listening failed to start: {}", e));
    }
}

/// Builds without the `wake` feature cannot construct a scorer; the setting
/// stays visible in the config but arms nothing.
#[cfg(not(feature = "wake"))]
fn arm(_app: &tauri::AppHandle, _state: &AppState) {
    tracing::debug!("wake feature not compiled; always-listening stays off");
}

/// Turn the persisted setting off after an unrecoverable arm failure and
/// tell the frontend so the toggle reflects reality.
#[cfg(feature = "wake")]
fn disable_wake_setting(app: &tauri::AppHandle, state: &AppState) {
    use tauri::Emitter;
    {
        let mut settings = state.settings.lock().unwrap_or_else(|e| e.into_inner());
        settings.wake_word_enabled = false;
        if let Ok(path) = murmur_core::config::Settings::default_path()
            && let Err(e) = settings.save(&path)
        {
            tracing::error!("Failed to save settings after wake disable: {}", e);
        }
    }
    let _ = app.emit("settings-changed", serde_json::json!({}));
    crate::tray::update_menu(app);
}

#[cfg(feature = "wake")]
fn build_scorer() -> anyhow::Result<Box<dyn murmur_core::audio::wake::WakeScorer>> {
    build_scorer_at(&murmur_core::audio::wake::resolved_model_paths()?)
}

/// Runtime check, not a cfg: whether the models exist on *this* machine can
/// only be asked of the filesystem (feature unification once made a
/// compile-time gate lie about runtime capability — see the STT stub test).
#[cfg(feature = "wake")]
fn build_scorer_at(
    paths: &murmur_core::audio::wake::WakeModelPaths,
) -> anyhow::Result<Box<dyn murmur_core::audio::wake::WakeScorer>> {
    use anyhow::Context;
    if !(paths.melspectrogram.exists() && paths.embedding.exists() && paths.head.exists()) {
        anyhow::bail!("wake model files are not downloaded");
    }
    let scorer = murmur_core::audio::wake_onnx::OnnxWakeScorer::new(paths)
        .context("failed to load wake models")?;
    Ok(Box::new(scorer))
}

/// Start the long-lived thread translating worker wake events into UI state,
/// session starts, and bounded re-arm retries. Returns the sender cloned
/// into every `ArmParams`.
pub(crate) fn spawn_event_pump(app: tauri::AppHandle) -> mpsc::Sender<WakeEvent> {
    let (tx, rx) = mpsc::channel::<WakeEvent>();
    std::thread::spawn(move || {
        let mut attempts: u32 = 0;
        while let Ok(event) = rx.recv() {
            let state = app.state::<AppState>();
            match event {
                WakeEvent::Armed => {
                    attempts = 0;
                    state.wake_arm_pending.store(false, Ordering::Release);
                    state.wake_armed.store(true, Ordering::Release);
                    // The intent may have changed while the arm was in flight
                    // (a meeting started, the mode was turned off), so settle
                    // against it now that the worker has answered.
                    sync(&app);
                }
                WakeEvent::Disarmed { reason, failed } => {
                    state.wake_arm_pending.store(false, Ordering::Release);
                    state.wake_armed.store(false, Ordering::Release);
                    tracing::info!(%reason, failed, "wake armed state ended");
                    match retry_delay(failed, attempts) {
                        Some(delay) => {
                            attempts += 1;
                            let app = app.clone();
                            std::thread::spawn(move || {
                                std::thread::sleep(delay);
                                sync(&app);
                            });
                        }
                        None if failed => {
                            emit_hotkey_error(
                                &app,
                                &format!("Always listening stopped: {}", reason),
                            );
                        }
                        // Expected exit: reconcile here rather than on a timer,
                        // so a disarm taken to reload settings re-arms as soon
                        // as the worker is actually idle.
                        None => sync(&app),
                    }
                }
                WakeEvent::Detected { score } => {
                    tracing::info!(score, "wake detection; starting session");
                    begin_wake_session(&app);
                }
            }
        }
        tracing::debug!("wake event pump ended");
    });
    tx
}

/// Start a dictation session in response to a wake detection. The reason
/// routes the worker to its prearmed start: live-buffer rewind past the wake
/// phrase, no calibration pause, and the silent-abort deadline.
fn begin_wake_session(app: &tauri::AppHandle) {
    crate::session::begin_wake_recording(app);
}

/// PBT_APMRESUMEAUTOMATIC / PBT_APMSUSPEND. Numeric so tests compile without
/// the windows crate; must stay equal to the Win32 constants.
///
/// The decision logic below is deliberately platform-neutral so its tests run
/// everywhere, which leaves it unused in a non-Windows lib build.
#[cfg_attr(not(windows), allow(dead_code))]
const POWER_RESUME_AUTOMATIC: u32 = 0x12;
#[cfg(test)]
const POWER_SUSPEND: u32 = 0x04;

/// What the Windows power-notify callback may do before it returns.
/// Resume work (disarm, ONNX load, arm) must not run on the callback stack.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PowerCallbackAction {
    Ignore,
    PostResume,
}

#[cfg_attr(not(windows), allow(dead_code))]
fn power_callback_action(event_type: u32, has_context: bool) -> PowerCallbackAction {
    if event_type == POWER_RESUME_AUTOMATIC && has_context {
        PowerCallbackAction::PostResume
    } else {
        PowerCallbackAction::Ignore
    }
}

/// Tauri's `RunEvent::Resumed` is the event-loop resume (mobile / some
/// desktops). It does not fire on Windows sleep/wake — see
/// <https://github.com/tauri-apps/tauri/issues/10662> and tao's Windows
/// backend, which never dispatches `WM_POWERBROADCAST`. Register a power
/// callback so always-listening re-opens the mic after resume.
#[cfg(windows)]
pub(crate) fn register_windows_resume_listener(app: tauri::AppHandle) {
    use std::ffi::c_void;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Power::{
        DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS, PowerRegisterSuspendResumeNotification,
    };
    use windows::Win32::UI::WindowsAndMessaging::DEVICE_NOTIFY_CALLBACK;

    let app = Box::leak(Box::new(app));
    let params = Box::leak(Box::new(DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS {
        Callback: Some(windows_resume_callback),
        Context: std::ptr::from_mut(app).cast::<c_void>(),
    }));
    let mut registration = std::ptr::null_mut();
    let status = unsafe {
        PowerRegisterSuspendResumeNotification(
            DEVICE_NOTIFY_CALLBACK,
            HANDLE(std::ptr::from_mut(params).cast::<c_void>()),
            &mut registration,
        )
    };
    if status.is_err() {
        tracing::warn!(
            error = status.0,
            "Failed to register Windows suspend/resume listener; always-listening may not recover after sleep"
        );
    }
}

#[cfg(windows)]
unsafe extern "system" fn windows_resume_callback(
    context: *const std::ffi::c_void,
    event_type: u32,
    _setting: *const std::ffi::c_void,
) -> u32 {
    if power_callback_action(event_type, !context.is_null()) == PowerCallbackAction::PostResume {
        // Safety: `context` is the leaked AppHandle from register_windows_resume_listener.
        let app = unsafe { &*context.cast::<tauri::AppHandle>() }.clone();
        std::thread::spawn(move || on_resume(&app));
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn armed_only_when_enabled_and_nothing_owns_the_mic() {
        assert!(should_be_armed(true, false, false));
        assert!(!should_be_armed(true, true, false));
        assert!(!should_be_armed(true, false, true));
        assert!(!should_be_armed(true, true, true));
        assert!(!should_be_armed(false, false, false));
        assert!(!should_be_armed(false, true, false));
        assert!(!should_be_armed(false, false, true));
        assert!(!should_be_armed(false, true, true));
    }

    #[test]
    fn rearm_backoff_is_increasing_and_bounded() {
        assert!(REARM_BACKOFF.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(retry_delay(true, 0), Some(REARM_BACKOFF[0]));
        assert_eq!(retry_delay(true, 2), Some(REARM_BACKOFF[2]));
        assert_eq!(retry_delay(true, 3), None);
    }

    #[test]
    fn expected_exits_never_retry() {
        assert_eq!(retry_delay(false, 0), None);
        assert_eq!(retry_delay(false, 2), None);
    }

    #[test]
    fn resume_mode_on_idle_restarts() {
        assert_eq!(
            resume_action(true, false, false, false),
            ResumeAction::Restart
        );
    }

    #[test]
    fn resume_must_rearm_even_if_flags_say_armed() {
        assert_eq!(
            resume_action(true, false, false, true),
            ResumeAction::Restart
        );
    }

    #[test]
    fn resume_mode_off_armed_disarms() {
        assert_eq!(
            resume_action(false, false, false, true),
            ResumeAction::Disarm
        );
    }

    #[test]
    fn resume_must_not_arm_over_a_meeting() {
        assert_ne!(
            resume_action(true, true, false, false),
            ResumeAction::Restart
        );
        assert_ne!(
            resume_action(true, true, false, true),
            ResumeAction::Restart
        );
        assert_eq!(resume_action(true, true, false, true), ResumeAction::Disarm);
        assert_eq!(resume_action(true, true, false, false), ResumeAction::None);
    }

    #[test]
    fn resume_must_not_arm_over_a_session() {
        assert_ne!(
            resume_action(true, false, true, false),
            ResumeAction::Restart
        );
        assert_ne!(
            resume_action(true, false, true, true),
            ResumeAction::Restart
        );
        assert_eq!(resume_action(true, false, true, true), ResumeAction::Disarm);
        assert_eq!(resume_action(true, false, true, false), ResumeAction::None);
    }

    #[test]
    fn resume_mode_off_idle_is_none() {
        assert_eq!(
            resume_action(false, false, false, false),
            ResumeAction::None
        );
    }

    #[test]
    fn power_resume_is_posted_off_the_callback_stack() {
        assert_eq!(
            power_callback_action(POWER_RESUME_AUTOMATIC, true),
            PowerCallbackAction::PostResume
        );
    }

    #[cfg(windows)]
    #[test]
    fn power_resume_constant_matches_win32() {
        assert_eq!(
            POWER_RESUME_AUTOMATIC,
            windows::Win32::UI::WindowsAndMessaging::PBT_APMRESUMEAUTOMATIC
        );
    }

    #[test]
    fn power_callback_ignores_non_resume_and_null_context() {
        assert_eq!(
            power_callback_action(POWER_SUSPEND, true),
            PowerCallbackAction::Ignore
        );
        assert_eq!(
            power_callback_action(POWER_RESUME_AUTOMATIC, false),
            PowerCallbackAction::Ignore
        );
    }

    /// Arming with absent model files must fail cleanly at the runtime
    /// check, never reach ONNX session construction.
    #[cfg(feature = "wake")]
    #[test]
    fn scorer_build_fails_cleanly_without_model_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = murmur_core::audio::wake::WakeModelPaths {
            melspectrogram: dir.path().join("melspectrogram.onnx"),
            embedding: dir.path().join("embedding_model.onnx"),
            head: dir.path().join("hey_murmur.onnx"),
        };
        let err = match build_scorer_at(&paths) {
            Ok(_) => panic!("must fail without files"),
            Err(e) => e,
        };
        assert!(err.to_string().contains("not downloaded"), "got: {err}");
    }

    #[test]
    fn a_meeting_starting_during_an_in_flight_arm_still_disarms() {
        // The worker has not answered yet, so `armed` is still false. Without
        // counting the pending arm this returns Nothing and the wake stream
        // stays open for the meeting.
        let worker = WorkerWakeState {
            armed: false,
            arm_pending: true,
        };
        let desired = should_be_armed(true, true, false);
        assert!(!desired, "a meeting must not want the wake stream");
        assert_eq!(sync_action(desired, worker), SyncAction::Disarm);
    }

    #[test]
    fn an_in_flight_arm_is_not_armed_twice() {
        let worker = WorkerWakeState {
            armed: false,
            arm_pending: true,
        };
        assert_eq!(sync_action(true, worker), SyncAction::Nothing);
    }

    #[test]
    fn an_idle_worker_arms_and_a_confirmed_one_holds() {
        let idle = WorkerWakeState {
            armed: false,
            arm_pending: false,
        };
        assert_eq!(sync_action(true, idle), SyncAction::Arm);
        assert_eq!(sync_action(false, idle), SyncAction::Nothing);

        let confirmed = WorkerWakeState {
            armed: true,
            arm_pending: false,
        };
        assert_eq!(sync_action(true, confirmed), SyncAction::Nothing);
        assert_eq!(sync_action(false, confirmed), SyncAction::Disarm);
    }
}
