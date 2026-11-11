//! Main window visibility helpers.

use tauri::{AppHandle, Manager};

use crate::commands::AppState;

pub const MAIN_LABEL: &str = "main";

/// True when the main window is on screen (shown and not minimised).
pub fn is_main_visible(app: &AppHandle) -> bool {
    app.get_webview_window(MAIN_LABEL)
        .is_some_and(|w| w.is_visible().unwrap_or(false) && !w.is_minimized().unwrap_or(false))
}

/// Tells the engine whether colour previews are currently worth emitting.
pub fn sync_preview(app: &AppHandle) {
    if let Some(state) = app.try_state::<AppState>() {
        state.engine.set_preview_enabled(is_main_visible(app));
    }
}

pub fn show_main(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN_LABEL) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
    sync_preview(app);
}

pub fn hide_main(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN_LABEL) {
        let _ = window.hide();
    }
    sync_preview(app);
}

pub fn toggle_main(app: &AppHandle) {
    if is_main_visible(app) {
        hide_main(app);
    } else {
        show_main(app);
    }
}
