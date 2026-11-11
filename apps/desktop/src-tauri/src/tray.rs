//! System tray icon and menu.

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Wry};

use crate::commands::AppState;
use crate::{updater, window};

const ID_OPEN: &str = "open";
const ID_PAUSE: &str = "pause";
const ID_UPDATE: &str = "update";
const ID_QUIT: &str = "quit";

const LABEL_PAUSE: &str = "Pause";
const LABEL_RESUME: &str = "Resume";

/// Label of the pause/resume entry for the given engine state.
pub fn pause_label(running: bool) -> &'static str {
    if running { LABEL_PAUSE } else { LABEL_RESUME }
}

/// Stops the engine, releasing outputs, and exits the application.
pub fn quit(app: &AppHandle) {
    if let Some(state) = app.try_state::<AppState>() {
        state.shutdown();
    }
    app.exit(0);
}

/// Creates the tray icon and returns the pause/resume menu item so its label can follow the
/// engine state.
pub fn build(app: &AppHandle) -> tauri::Result<MenuItem<Wry>> {
    let open = MenuItem::with_id(app, ID_OPEN, "Open Glowdrift", true, None::<&str>)?;
    let pause = MenuItem::with_id(app, ID_PAUSE, LABEL_PAUSE, true, None::<&str>)?;
    let update = MenuItem::with_id(app, ID_UPDATE, "Check for updates", true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, ID_QUIT, "Quit", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&open, &pause, &update, &separator, &quit_item])?;

    let mut builder = TrayIconBuilder::with_id("main")
        .tooltip("Glowdrift")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            ID_OPEN => window::show_main(app),
            ID_PAUSE => {
                if let Some(state) = app.try_state::<AppState>() {
                    state.toggle_running();
                }
            }
            ID_UPDATE => updater::check_now(app.clone()),
            ID_QUIT => quit(app),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                window::toggle_main(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(pause)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pause_label_follows_running_state() {
        assert_eq!(pause_label(true), "Pause");
        assert_eq!(pause_label(false), "Resume");
    }
}
