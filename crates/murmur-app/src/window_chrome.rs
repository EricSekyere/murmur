//! Main-window chrome; native menu definitions also drive the custom dropdown.

use tauri::{
    LogicalPosition, Manager, WebviewWindow, Wry,
    menu::{Menu, MenuItem, MenuItemKind, Submenu},
};

#[derive(serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum MenuEntry {
    Separator,
    Command {
        id: String,
        label: String,
        enabled: bool,
        accelerator: Option<&'static str>,
    },
}

#[derive(serde::Serialize)]
pub(crate) struct MenuGroup {
    id: String,
    label: String,
    native: bool,
    items: Vec<MenuEntry>,
}

fn application_menu(window: &WebviewWindow) -> Result<Menu<Wry>, String> {
    window
        .menu()
        .or_else(|| window.app_handle().menu())
        .ok_or_else(|| "Application menu is not ready".into())
}

fn describe_group(submenu: Submenu<Wry>) -> tauri::Result<MenuGroup> {
    let mut group = MenuGroup {
        id: submenu.id().as_ref().to_owned(),
        label: submenu.text()?,
        native: false,
        items: Vec::new(),
    };
    for item in submenu.items()? {
        match item {
            MenuItemKind::MenuItem(item) if item.id().as_ref().starts_with("app:") => {
                group.items.push(MenuEntry::Command {
                    id: item.id().as_ref().to_owned(),
                    label: item.text()?,
                    enabled: item.is_enabled()?,
                    accelerator: crate::menu::accelerator_for(item.id().as_ref()),
                });
            }
            MenuItemKind::Predefined(item) if item.text()?.is_empty() => {
                group.items.push(MenuEntry::Separator)
            }
            // Keep OS editing, Services, and future unsupported item types native.
            _ => group.native = true,
        }
    }
    Ok(group)
}

#[tauri::command]
pub(crate) fn window_menu_groups(window: WebviewWindow) -> Result<Vec<MenuGroup>, String> {
    require_main(window.label())?;
    application_menu(&window)?
        .items()
        .map_err(|error| error.to_string())?
        .into_iter()
        .filter_map(|item| match item {
            MenuItemKind::Submenu(submenu) => Some(describe_group(submenu)),
            _ => None,
        })
        .collect::<tauri::Result<Vec<_>>>()
        .map_err(|error| error.to_string())
}

fn require_main(label: &str) -> Result<(), String> {
    if label == "main" {
        Ok(())
    } else {
        Err("Window chrome is only available in the main window".into())
    }
}

#[tauri::command]
pub(crate) fn initialize_window_chrome(window: WebviewWindow) -> Result<&'static str, String> {
    require_main(window.label())?;
    #[cfg(not(target_os = "macos"))]
    {
        // Hiding retains the native menu's keyboard accelerators.
        window.hide_menu().map_err(|error| error.to_string())?;
        if let Err(error) = window.set_decorations(false) {
            if let Err(restore_error) = window.show_menu() {
                tracing::warn!(%restore_error, "could not restore the native menu");
            }
            return Err(error.to_string());
        }
    }
    Ok(std::env::consts::OS)
}

#[tauri::command]
pub(crate) fn show_window_menu(
    window: WebviewWindow,
    menu_id: Option<String>,
    x: f64,
    y: f64,
) -> Result<(), String> {
    require_main(window.label())?;
    if !x.is_finite() || !y.is_finite() || x < 0.0 || y < 0.0 {
        return Err("Invalid menu position".into());
    }
    let menu = application_menu(&window)?;
    let position = LogicalPosition::new(x, y);
    if let Some(id) = menu_id {
        let submenu = menu
            .items()
            .map_err(|error| error.to_string())?
            .into_iter()
            .find_map(|item| match item {
                MenuItemKind::Submenu(submenu) if submenu.id().as_ref() == id => Some(submenu),
                _ => None,
            })
            .ok_or("Unknown application menu")?;
        window
            .popup_menu_at(&submenu, position)
            .map_err(|error| error.to_string())
    } else {
        window
            .popup_menu_at(&menu, position)
            .map_err(|error| error.to_string())
    }
}

fn find_command(menu: &Menu<Wry>, id: &str) -> Result<MenuItem<Wry>, String> {
    if !id.starts_with("app:") {
        return Err("Unknown application command".into());
    }
    for group in menu.items().map_err(|error| error.to_string())? {
        let MenuItemKind::Submenu(submenu) = group else {
            continue;
        };
        for item in submenu.items().map_err(|error| error.to_string())? {
            let MenuItemKind::MenuItem(item) = item else {
                continue;
            };
            if item.id().as_ref() == id {
                return Ok(item);
            }
        }
    }
    Err("Unknown application command".into())
}

#[tauri::command]
pub(crate) fn invoke_menu_command(window: WebviewWindow, id: String) -> Result<(), String> {
    require_main(window.label())?;
    let item = find_command(&application_menu(&window)?, &id)?;
    if !item.is_enabled().map_err(|error| error.to_string())? {
        return Err("This command is unavailable right now".into());
    }
    crate::menu::handle_event(window.app_handle(), &id);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_main_can_change_chrome() {
        assert!(require_main("main").is_ok());
        for label in ["widget", "caption", "", "other"] {
            assert!(require_main(label).is_err());
        }
    }
}
