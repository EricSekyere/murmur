//! System tray: menu construction, actions, and state-driven item updates.

use tauri::{
    AppHandle, Manager, Wry,
    image::Image,
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};

#[cfg(feature = "wake")]
use tauri::menu::CheckMenuItem;

use crate::state::AppState;

/// Handles to the state-dependent menu items, kept in Tauri state so
/// [`update_menu`] can reach them from any thread (Tauri proxies menu
/// mutations to the main thread internally).
pub(crate) struct TrayMenu {
    dictate: MenuItem<Wry>,
    copy_last: MenuItem<Wry>,
    #[cfg(feature = "wake")]
    always_listening: CheckMenuItem<Wry>,
}

/// What the dictation item should show for a given app state.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct DictationItem {
    pub label: &'static str,
    pub enabled: bool,
}

/// Pure mapping from app state to the dictation item. A meeting takes
/// precedence: it owns the mic and the STT engine, and `start_session`
/// refuses dictation while one runs, so the item explains why instead of
/// offering an action that would only produce an error.
pub(crate) fn dictation_item(recording: bool, meeting_active: bool) -> DictationItem {
    if meeting_active {
        DictationItem {
            label: "Dictation paused (meeting active)",
            enabled: false,
        }
    } else if recording {
        DictationItem {
            label: "Stop dictation",
            enabled: true,
        }
    } else {
        DictationItem {
            label: "Start dictation",
            enabled: true,
        }
    }
}

pub(crate) fn build(app: &mut tauri::App) -> tauri::Result<()> {
    // Runtime package info: release CI stamps it from the git tag, so unlike
    // CARGO_PKG_VERSION it is meaningful outside dev builds (see about.rs).
    let version_i = MenuItem::with_id(
        app,
        "version",
        format!(
            "{} v{}",
            crate::about::display_name(app.handle()),
            app.package_info().version
        ),
        false,
        None::<&str>,
    )?;
    let initial = dictation_item(false, false);
    let dictate_i =
        MenuItem::with_id(app, "dictate", initial.label, initial.enabled, None::<&str>)?;
    // Disabled until update_menu below sees stored history.
    let copy_i = MenuItem::with_id(
        app,
        "copy_last",
        "Copy Last Transcript",
        false,
        None::<&str>,
    )?;
    #[cfg(feature = "wake")]
    let always_listening_i = {
        let checked = app
            .try_state::<AppState>()
            .map(|s| {
                s.settings
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .wake_word_enabled
            })
            .unwrap_or(false);
        CheckMenuItem::with_id(
            app,
            "always_listening",
            "Always listening",
            true,
            checked,
            None::<&str>,
        )?
    };
    let show_i = MenuItem::with_id(app, "show", "Show", true, None::<&str>)?;
    let settings_i = MenuItem::with_id(app, "settings", "Settings", true, None::<&str>)?;
    let widget_i = MenuItem::with_id(app, "toggle_widget", "Toggle Widget", true, None::<&str>)?;
    let quit_i = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let sep_top = PredefinedMenuItem::separator(app)?;
    let sep_mid = PredefinedMenuItem::separator(app)?;
    let sep_bot = PredefinedMenuItem::separator(app)?;
    #[cfg(feature = "wake")]
    let menu = Menu::with_items(
        app,
        &[
            &version_i,
            &sep_top,
            &dictate_i,
            &copy_i,
            &always_listening_i,
            &sep_mid,
            &show_i,
            &settings_i,
            &widget_i,
            &sep_bot,
            &quit_i,
        ],
    )?;
    #[cfg(not(feature = "wake"))]
    let menu = Menu::with_items(
        app,
        &[
            &version_i,
            &sep_top,
            &dictate_i,
            &copy_i,
            &sep_mid,
            &show_i,
            &settings_i,
            &widget_i,
            &sep_bot,
            &quit_i,
        ],
    )?;

    let icon = app.default_window_icon().cloned().unwrap_or_else(|| {
        tracing::warn!("No default window icon found, using fallback");
        Image::new_owned(vec![0, 0, 0, 0], 1, 1)
    });

    TrayIconBuilder::new()
        .icon(icon)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .tooltip(format!(
            "{} - Voice to Text",
            crate::about::display_name(app.handle())
        ))
        .on_menu_event(|app, event| match event.id.as_ref() {
            // exit(0) routes through RunEvent::Exit, where an active meeting
            // is stopped and its spool deleted; never bypass it.
            "quit" => app.exit(0),
            "show" => crate::show_main_window(app),
            "settings" => open_settings(app),
            "dictate" => crate::session::handle_toggle(app),
            "copy_last" => copy_last_transcript(app),
            "toggle_widget" => crate::toggle_widget(app),
            #[cfg(feature = "wake")]
            "always_listening" => toggle_always_listening(app),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                crate::show_main_window(tray.app_handle());
            }
        })
        .build(app)?;

    app.manage(TrayMenu {
        dictate: dictate_i,
        copy_last: copy_i,
        #[cfg(feature = "wake")]
        always_listening: always_listening_i,
    });

    // History loaded at startup may already have entries; sync item states.
    update_menu(app.handle());
    Ok(())
}

/// Re-derive the state-dependent items from `AppState`. Hooked into the
/// existing broadcast points (`emit_recording_state`, the meeting worker's
/// `meeting-state` emits, history clearing) rather than polling.
pub(crate) fn update_menu(app: &AppHandle) {
    let (Some(items), Some(state)) = (app.try_state::<TrayMenu>(), app.try_state::<AppState>())
    else {
        return;
    };
    let recording = *state.recording.lock().unwrap_or_else(|e| e.into_inner());
    let meeting = state
        .meeting_active
        .load(std::sync::atomic::Ordering::Acquire);
    let item = dictation_item(recording, meeting);
    // Cosmetic updates; the menu can already be torn down mid-exit, so a
    // failure here is safe to drop.
    let _ = items.dictate.set_text(item.label);
    let _ = items.dictate.set_enabled(item.enabled);
    let has_history = !state
        .history
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .entries()
        .is_empty();
    let _ = items.copy_last.set_enabled(has_history);
    #[cfg(feature = "wake")]
    {
        let checked = state
            .settings
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .wake_word_enabled;
        let _ = items.always_listening.set_checked(checked);
    }
}

/// Copy the newest history entry to the clipboard: the recovery move when
/// dictation landed in the wrong window. The transcript text is never logged.
/// Shared with the application menu's Copy Last Transcript item.
pub(crate) fn copy_last_transcript(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let text = {
        let history = state.history.lock().unwrap_or_else(|e| e.into_inner());
        history.entries().first().map(|entry| entry.text.clone())
    };
    // The item is disabled while history is empty, but history can be cleared
    // between the menu opening and the click.
    let Some(text) = text else {
        crate::state::emit_hotkey_error(app, "No transcripts yet: dictate something first");
        return;
    };
    let result = murmur_core::output::clipboard::ClipboardOutput::new()
        .and_then(|mut clipboard| clipboard.copy(&text));
    match result {
        Ok(()) => tracing::info!("Copied last transcript to clipboard"),
        Err(e) => {
            tracing::warn!("Copy last transcript failed: {e}");
            crate::state::emit_hotkey_error(app, "Could not copy the transcript to the clipboard");
        }
    }
}

/// Show the main window on its Settings view.
fn open_settings(app: &AppHandle) {
    crate::menu::navigate(app, "settings");
}

/// Toggle through the same `update_settings { wake_word_enabled }` path as
/// the settings page. The OS check-item flips immediately; we restore from
/// the persisted setting until that save completes (never optimistic).
#[cfg(feature = "wake")]
fn toggle_always_listening(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let current = state
        .settings
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .wake_word_enabled;
    let desired = !current;
    update_menu(app);

    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if desired
            && !crate::commands::wake_models_ready()
            && let Err(e) = crate::commands::download_wake_models(app.clone()).await
        {
            crate::state::emit_hotkey_error(&app, &e);
            update_menu(&app);
            return;
        }
        if let Err(e) = crate::commands::set_wake_word_enabled(app.clone(), desired) {
            crate::state::emit_hotkey_error(&app, &e);
        }
        update_menu(&app);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_offers_start() {
        let item = dictation_item(false, false);
        assert_eq!(item.label, "Start dictation");
        assert!(item.enabled);
    }

    #[test]
    fn recording_offers_stop() {
        let item = dictation_item(true, false);
        assert_eq!(item.label, "Stop dictation");
        assert!(item.enabled);
    }

    #[test]
    fn meeting_disables_dictation() {
        let item = dictation_item(false, true);
        assert_eq!(item.label, "Dictation paused (meeting active)");
        assert!(!item.enabled);
    }

    #[test]
    fn meeting_wins_over_stale_recording_flag() {
        // The modes are mutually exclusive by construction; if the flags ever
        // disagree, the safe rendering is the disabled meeting label.
        let item = dictation_item(true, true);
        assert!(!item.enabled);
    }
}
