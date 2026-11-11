//! Background update checks. Updates are downloaded and verified in the background, then
//! staged; installing is an explicit action because the Windows installer ends the process.

use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::{Update, UpdaterExt};

use crate::commands::{AppState, events};

const FIRST_CHECK_DELAY: Duration = Duration::from_secs(10);
const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

/// A downloaded, signature-verified update waiting to be installed.
pub struct PendingUpdate {
    update: Update,
    bytes: Vec<u8>,
}

impl std::fmt::Debug for PendingUpdate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingUpdate")
            .field("version", &self.update.version)
            .finish_non_exhaustive()
    }
}

/// Payload of the `updater://ready` event.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateReady {
    pub version: String,
}

/// Checks 10 s after start and every 6 h afterwards, while auto-update is enabled.
pub fn spawn_scheduler(app: AppHandle) {
    let spawned = std::thread::Builder::new()
        .name("glowdrift-updater".to_owned())
        .spawn(move || {
            std::thread::sleep(FIRST_CHECK_DELAY);
            loop {
                let enabled = app
                    .try_state::<AppState>()
                    .is_some_and(|state| state.auto_update_enabled());
                if enabled {
                    check_and_stage(&app);
                }
                std::thread::sleep(CHECK_INTERVAL);
            }
        });
    if let Err(e) = spawned {
        log::warn!("cannot start the update scheduler: {e}");
    }
}

/// Runs one check on a worker thread (used by the tray menu).
pub fn check_now(app: AppHandle) {
    let spawned = std::thread::Builder::new()
        .name("glowdrift-update-check".to_owned())
        .spawn(move || check_and_stage(&app));
    if let Err(e) = spawned {
        log::warn!("cannot start the update check: {e}");
    }
}

/// Failures are expected (no release yet, offline) and only logged at debug level.
fn check_and_stage(app: &AppHandle) {
    if let Err(e) = try_check_and_stage(app) {
        log::debug!("update check failed: {e}");
    }
}

fn try_check_and_stage(app: &AppHandle) -> Result<(), tauri_plugin_updater::Error> {
    let hook_app = app.clone();
    let updater = app
        .updater_builder()
        .on_before_exit(move || {
            if let Some(state) = hook_app.try_state::<AppState>() {
                state.shutdown();
            }
        })
        .build()?;
    let Some(update) = tauri::async_runtime::block_on(updater.check())? else {
        log::debug!("no update available");
        return Ok(());
    };
    let Some(state) = app.try_state::<AppState>() else {
        return Ok(());
    };
    let already_staged = state.pending_update.lock().is_ok_and(|p| {
        p.as_ref()
            .is_some_and(|p| p.update.version == update.version)
    });
    if already_staged {
        return Ok(());
    }
    let bytes = tauri::async_runtime::block_on(update.download(|_, _| {}, || {}))?;
    let version = update.version.clone();
    if let Ok(mut pending) = state.pending_update.lock() {
        *pending = Some(PendingUpdate { update, bytes });
    }
    log::info!("update {version} downloaded");
    let _ = app.emit(events::UPDATER_READY, UpdateReady { version });
    Ok(())
}

/// Installs the staged update. The engine is stopped first; on Windows a successful install
/// exits the process, so `Ok(true)` is only seen on platforms where it returns.
pub fn install_pending(app: &AppHandle) -> Result<bool, String> {
    let Some(state) = app.try_state::<AppState>() else {
        return Ok(false);
    };
    let pending = state
        .pending_update
        .lock()
        .map_err(|_| "Update state is unavailable.".to_owned())?
        .take();
    let Some(pending) = pending else {
        return Ok(false);
    };
    pending
        .update
        .install(&pending.bytes)
        .map_err(|e| format!("The update could not be installed: {e}"))?;
    Ok(true)
}
