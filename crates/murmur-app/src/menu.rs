//! Native application menu for the main window: construction, event routing,
//! and state-driven item updates (mirroring `tray.rs`).
//!
//! Windows/Linux attach the menu to the main window only, so the pill and
//! caption windows never grow a menu bar. macOS sets it app-wide (menus live
//! in the system bar there) and adds the conventional application menu.

use tauri::{
    AppHandle, Emitter, Manager, Wry,
    menu::{Menu, MenuItem, PredefinedMenuItem, Submenu},
};

use crate::state::AppState;
use crate::tray;

// Menu item ids carry an `app:` prefix so they can never collide with the
// tray menu's ids in the shared menu-event stream.
const ID_SETTINGS: &str = "app:settings";
const ID_QUIT: &str = "app:quit";
const ID_DICTATE: &str = "app:dictate";
const ID_COMMAND_MODE: &str = "app:command-mode";
const ID_COPY_LAST: &str = "app:copy-last";
const ID_MEETING: &str = "app:meeting";
const ID_PILL: &str = "app:pill";
const ID_CHECK_UPDATES: &str = "app:check-updates";
const ID_HELP_CENTER: &str = "app:help-center";
const ID_REPORT: &str = "app:report";
const ID_PROJECT: &str = "app:project";
#[cfg(not(target_os = "macos"))]
const ID_ABOUT: &str = "app:about";

/// The View menu, one entry per dashboard view: (id, label, accelerator,
/// `navigate-view` payload). The payload names must match `viewEls` in
/// `frontend/js/dashboard.js`.
const VIEW_ITEMS: &[(&str, &str, &str, &str)] = &[
    ("app:view:home", "Home", "CmdOrCtrl+1", "home"),
    (
        "app:view:analytics",
        "Analytics",
        "CmdOrCtrl+2",
        "analytics",
    ),
    ("app:view:settings", "Settings", "CmdOrCtrl+3", "settings"),
    (
        "app:view:diagnostics",
        "Diagnostics",
        "CmdOrCtrl+4",
        "diagnostics",
    ),
    ("app:view:help", "Help", "CmdOrCtrl+5", "help"),
];

/// Handles to the state-dependent items, kept in Tauri state so
/// [`update_menu`] can reach them from any thread.
pub(crate) struct AppMenu {
    dictate: MenuItem<Wry>,
    meeting: MenuItem<Wry>,
    copy_last: MenuItem<Wry>,
}

/// What the meeting item should show for a given app state. Dictation in
/// progress disables the start (the modes fight over the mic and the STT
/// engine, and `start_meeting` refuses); an engine still loading is left to
/// `start_meeting`'s own error, which reaches the user as a toast, because
/// `engine_loaded` flips in places that never refresh menus.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct MeetingItem {
    pub label: &'static str,
    pub enabled: bool,
}

pub(crate) fn meeting_item(recording: bool, meeting_active: bool) -> MeetingItem {
    if meeting_active {
        MeetingItem {
            label: "Stop Meeting",
            enabled: true,
        }
    } else if recording {
        MeetingItem {
            label: "Meeting unavailable (dictating)",
            enabled: false,
        }
    } else {
        MeetingItem {
            label: "Start Meeting",
            enabled: true,
        }
    }
}

/// The dashboard view a menu id navigates to, if it is a View entry.
fn view_for_id(id: &str) -> Option<&'static str> {
    VIEW_ITEMS
        .iter()
        .find(|(item_id, ..)| *item_id == id)
        .map(|(.., view)| *view)
}

pub(crate) fn build(app: &mut tauri::App) -> tauri::Result<()> {
    let dictation_menu = build_dictation_menu(app)?;
    let edit_menu = build_edit_menu(app)?;
    let view_menu = build_view_menu(app)?;
    let help_menu = build_help_menu(app)?;

    #[cfg(target_os = "macos")]
    {
        let menu = Menu::with_items(
            app,
            &[
                &build_mac_app_menu(app)?,
                &dictation_menu,
                &edit_menu,
                &view_menu,
                &build_mac_window_menu(app)?,
                &help_menu,
            ],
        )?;
        // App-wide: on macOS the menu lives in the system bar, not a window.
        app.set_menu(menu)?;
    }
    #[cfg(not(target_os = "macos"))]
    {
        let menu = Menu::with_items(
            app,
            &[
                &build_file_menu(app)?,
                &dictation_menu,
                &edit_menu,
                &view_menu,
                &help_menu,
            ],
        )?;
        // Main window only: the pill and caption must stay chromeless.
        if let Some(main) = app.get_webview_window("main") {
            main.set_menu(menu)?;
        }
    }

    app.on_menu_event(|app, event| handle_event(app, event.id.as_ref()));
    update_menu(app.handle());
    Ok(())
}

fn build_dictation_menu(app: &tauri::App) -> tauri::Result<Submenu<Wry>> {
    let initial = tray::dictation_item(false, false);
    let dictate = MenuItem::with_id(
        app,
        ID_DICTATE,
        initial.label,
        initial.enabled,
        Some("CmdOrCtrl+D"),
    )?;
    let command_mode = MenuItem::with_id(app, ID_COMMAND_MODE, "Command Mode", true, None::<&str>)?;
    // Disabled until update_menu sees stored history, like the tray item.
    let copy_last = MenuItem::with_id(
        app,
        ID_COPY_LAST,
        "Copy Last Transcript",
        false,
        Some("CmdOrCtrl+Shift+C"),
    )?;
    let initial_meeting = meeting_item(false, false);
    let meeting = MenuItem::with_id(
        app,
        ID_MEETING,
        initial_meeting.label,
        initial_meeting.enabled,
        None::<&str>,
    )?;
    let menu = Submenu::with_items(
        app,
        "Dictation",
        true,
        &[
            &dictate,
            &command_mode,
            &copy_last,
            &PredefinedMenuItem::separator(app)?,
            &meeting,
        ],
    )?;
    app.manage(AppMenu {
        dictate,
        meeting,
        copy_last,
    });
    Ok(menu)
}

/// Predefined edit items so the OS-standard clipboard shortcuts work in the
/// Settings text areas (dictionary, snippets, profiles); on macOS those
/// shortcuts only function when these menu items exist.
fn build_edit_menu(app: &tauri::App) -> tauri::Result<Submenu<Wry>> {
    Submenu::with_items(
        app,
        "Edit",
        true,
        &[
            &PredefinedMenuItem::undo(app, None)?,
            &PredefinedMenuItem::redo(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::cut(app, None)?,
            &PredefinedMenuItem::copy(app, None)?,
            &PredefinedMenuItem::paste(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::select_all(app, None)?,
        ],
    )
}

fn build_view_menu(app: &tauri::App) -> tauri::Result<Submenu<Wry>> {
    let menu = Submenu::new(app, "View", true)?;
    for (id, label, accel, _) in VIEW_ITEMS {
        menu.append(&MenuItem::with_id(app, *id, *label, true, Some(*accel))?)?;
    }
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&MenuItem::with_id(
        app,
        ID_PILL,
        "Toggle Floating Pill",
        true,
        None::<&str>,
    )?)?;
    Ok(menu)
}

fn build_help_menu(app: &tauri::App) -> tauri::Result<Submenu<Wry>> {
    let help_center = MenuItem::with_id(app, ID_HELP_CENTER, "Help Center", true, Some("F1"))?;
    let report = MenuItem::with_id(app, ID_REPORT, "Report a Problem", true, None::<&str>)?;
    let project = MenuItem::with_id(app, ID_PROJECT, "Project Page", true, None::<&str>)?;
    // macOS puts this in the application menu, next to About.
    #[cfg(not(target_os = "macos"))]
    let check_updates = MenuItem::with_id(
        app,
        ID_CHECK_UPDATES,
        "Check for Updates\u{2026}",
        true,
        None::<&str>,
    )?;
    #[cfg(not(target_os = "macos"))]
    {
        // The About section lives in Settings; macOS gets the predefined
        // About item in the application menu instead.
        let about = MenuItem::with_id(app, ID_ABOUT, "About Murmur", true, None::<&str>)?;
        Submenu::with_items(
            app,
            "Help",
            true,
            &[
                &help_center,
                &PredefinedMenuItem::separator(app)?,
                &report,
                &project,
                &PredefinedMenuItem::separator(app)?,
                &check_updates,
                &about,
            ],
        )
    }
    #[cfg(target_os = "macos")]
    Submenu::with_items(
        app,
        "Help",
        true,
        &[
            &help_center,
            &PredefinedMenuItem::separator(app)?,
            &report,
            &project,
        ],
    )
}

#[cfg(target_os = "macos")]
fn build_mac_app_menu(app: &tauri::App) -> tauri::Result<Submenu<Wry>> {
    let metadata = tauri::menu::AboutMetadata {
        name: Some("Murmur".into()),
        version: Some(app.package_info().version.to_string()),
        ..Default::default()
    };
    let settings = MenuItem::with_id(
        app,
        ID_SETTINGS,
        "Settings\u{2026}",
        true,
        Some("CmdOrCtrl+Comma"),
    )?;
    let check_updates = MenuItem::with_id(
        app,
        ID_CHECK_UPDATES,
        "Check for Updates\u{2026}",
        true,
        None::<&str>,
    )?;
    Submenu::with_items(
        app,
        "Murmur",
        true,
        &[
            &PredefinedMenuItem::about(app, Some("About Murmur"), Some(metadata))?,
            &check_updates,
            &PredefinedMenuItem::separator(app)?,
            &settings,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::services(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::hide(app, None)?,
            &PredefinedMenuItem::hide_others(app, None)?,
            &PredefinedMenuItem::show_all(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::quit(app, Some("Quit Murmur"))?,
        ],
    )
}

/// Replacing Tauri's default macOS menu loses its Window menu; restore the
/// standard entries so minimize/zoom keep their conventional home.
#[cfg(target_os = "macos")]
fn build_mac_window_menu(app: &tauri::App) -> tauri::Result<Submenu<Wry>> {
    Submenu::with_items(
        app,
        "Window",
        true,
        &[
            &PredefinedMenuItem::minimize(app, None)?,
            &PredefinedMenuItem::maximize(app, None)?,
        ],
    )
}

#[cfg(not(target_os = "macos"))]
fn build_file_menu(app: &tauri::App) -> tauri::Result<Submenu<Wry>> {
    let settings = MenuItem::with_id(app, ID_SETTINGS, "Settings", true, Some("CmdOrCtrl+Comma"))?;
    // Routes through app.exit(0) in the handler so RunEvent::Exit still runs
    // (stops an active meeting and deletes its audio spool).
    let quit = MenuItem::with_id(app, ID_QUIT, "Quit", true, Some("CmdOrCtrl+Q"))?;
    Submenu::with_items(
        app,
        "File",
        true,
        &[&settings, &PredefinedMenuItem::separator(app)?, &quit],
    )
}

fn handle_event(app: &AppHandle, id: &str) {
    if let Some(view) = view_for_id(id) {
        navigate(app, view);
        return;
    }
    match id {
        ID_SETTINGS => navigate(app, "settings"),
        ID_QUIT => app.exit(0),
        ID_DICTATE => crate::session::handle_toggle(app),
        ID_COMMAND_MODE => crate::command_mode::toggle_mode(app),
        ID_COPY_LAST => tray::copy_last_transcript(app),
        ID_MEETING => toggle_meeting(app),
        ID_PILL => crate::toggle_widget(app),
        ID_CHECK_UPDATES => {
            crate::updater::spawn_check(app.clone(), crate::updater::CheckKind::Requested)
        }
        ID_HELP_CENTER => navigate(app, "help"),
        ID_REPORT => open_link(app, "issues"),
        ID_PROJECT => open_link(app, "repo"),
        #[cfg(not(target_os = "macos"))]
        ID_ABOUT => {
            crate::show_main_window(app);
            let _ = app.emit("show-about", ());
        }
        _ => {}
    }
}

/// Show the main window on one of the dashboard views. Delivery is best
/// effort: if the webview is still booting, the user lands on the default
/// view (same trade-off as the tray's Settings item).
pub(crate) fn navigate(app: &AppHandle, view: &str) {
    crate::show_main_window(app);
    let _ = app.emit("navigate-view", serde_json::json!({ "view": view }));
}

/// Open one of the allowlisted About links; the id must exist in
/// `about::ABOUT_LINKS`, so no arbitrary URL can ever come through here.
fn open_link(app: &AppHandle, id: &str) {
    if let Err(e) = crate::about::open_about_link(id.to_string()) {
        tracing::warn!(link = id, "Could not open link from the menu: {e}");
        crate::state::emit_hotkey_error(app, "Could not open the link in your browser");
    }
}

/// Start or stop a meeting from the menu, reusing the same worker paths as
/// the Meetings UI. Stopping waits for the final chunk off the main thread;
/// the worker's own `meeting-state` broadcast refreshes menus and frontend.
fn toggle_meeting(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let handle = state
        .meeting
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take();
    if let Some(handle) = handle {
        // Final-chunk inference can take seconds; never block the UI thread.
        tauri::async_runtime::spawn_blocking(move || handle.stop_and_join());
    } else if let Err(e) = crate::meeting_commands::start_meeting(app.clone(), app.state()) {
        // Same blockers the Meetings UI reports (engine loading, dictating).
        crate::state::emit_hotkey_error(app, &e);
    }
}

/// Re-derive the state-dependent items from `AppState`; called from the same
/// broadcast points as `tray::update_menu`.
pub(crate) fn update_menu(app: &AppHandle) {
    let (Some(items), Some(state)) = (app.try_state::<AppMenu>(), app.try_state::<AppState>())
    else {
        return;
    };
    let recording = *state.recording.lock().unwrap_or_else(|e| e.into_inner());
    let meeting_active = state
        .meeting_active
        .load(std::sync::atomic::Ordering::Acquire);
    let dictate = tray::dictation_item(recording, meeting_active);
    let meeting = meeting_item(recording, meeting_active);
    let has_history = !state
        .history
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .entries()
        .is_empty();
    // Cosmetic updates; the menu can already be torn down mid-exit, so a
    // failure here is safe to drop.
    let _ = items.dictate.set_text(dictate.label);
    let _ = items.dictate.set_enabled(dictate.enabled);
    let _ = items.meeting.set_text(meeting.label);
    let _ = items.meeting.set_enabled(meeting.enabled);
    let _ = items.copy_last.set_enabled(has_history);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_offers_meeting_start() {
        let item = meeting_item(false, false);
        assert_eq!(item.label, "Start Meeting");
        assert!(item.enabled);
    }

    #[test]
    fn active_meeting_offers_stop() {
        let item = meeting_item(false, true);
        assert_eq!(item.label, "Stop Meeting");
        assert!(item.enabled);
    }

    #[test]
    fn dictation_blocks_meeting_start() {
        let item = meeting_item(true, false);
        assert!(!item.enabled);
        assert_eq!(item.label, "Meeting unavailable (dictating)");
    }

    #[test]
    fn active_meeting_wins_over_stale_recording_flag() {
        // Mutually exclusive by construction; if the flags ever disagree the
        // safe rendering is the one that lets the user stop the meeting.
        let item = meeting_item(true, true);
        assert_eq!(item.label, "Stop Meeting");
        assert!(item.enabled);
    }

    #[test]
    fn every_view_item_maps_to_a_dashboard_view() {
        let views: Vec<_> = VIEW_ITEMS.iter().map(|(id, ..)| view_for_id(id)).collect();
        assert_eq!(
            views,
            ["home", "analytics", "settings", "diagnostics", "help"]
                .map(Some)
                .to_vec()
        );
    }

    #[test]
    fn non_view_ids_do_not_navigate() {
        assert_eq!(view_for_id(ID_SETTINGS), None);
        assert_eq!(view_for_id("view:home"), None);
        assert_eq!(view_for_id(""), None);
    }

    #[test]
    fn view_accelerators_match_sidebar_order() {
        for (index, (_, _, accel, _)) in VIEW_ITEMS.iter().enumerate() {
            assert_eq!(*accel, format!("CmdOrCtrl+{}", index + 1));
        }
    }

    #[test]
    fn menu_ids_are_unique_and_prefixed() {
        let mut ids = vec![
            ID_SETTINGS,
            ID_QUIT,
            ID_DICTATE,
            ID_COMMAND_MODE,
            ID_COPY_LAST,
            ID_MEETING,
            ID_PILL,
            ID_HELP_CENTER,
            ID_REPORT,
            ID_PROJECT,
        ];
        ids.extend(VIEW_ITEMS.iter().map(|(id, ..)| *id));
        let unique: std::collections::HashSet<_> = ids.iter().copied().collect();
        assert_eq!(unique.len(), ids.len());
        // The prefix is what keeps app-menu ids out of the tray handler.
        assert!(ids.iter().all(|id| id.starts_with("app:")));
    }
}
