//! Background update checks. Updates are downloaded and verified in the background, then
//! staged; installing is an explicit action because the Windows installer ends the process.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::{Error as UpdaterError, Update, UpdaterExt};

use crate::commands::{AppState, events};

const FIRST_CHECK_DELAY: Duration = Duration::from_secs(10);
const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
/// After consecutive failed checks the interval doubles, this many times at most.
const MAX_BACKOFF_DOUBLINGS: u32 = 3;

/// A downloaded, signature-verified update waiting to be installed.
pub struct PendingUpdate {
    update: Update,
    bytes: Vec<u8>,
    /// Set by the updater's before-exit hook once the engine and outputs have been stopped.
    engine_stopped: Arc<AtomicBool>,
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

/// Time until the next scheduled check: the normal interval, doubled for each consecutive
/// failure up to [`MAX_BACKOFF_DOUBLINGS`] times.
fn next_check_delay(consecutive_failures: u32) -> Duration {
    CHECK_INTERVAL * 2_u32.pow(consecutive_failures.min(MAX_BACKOFF_DOUBLINGS))
}

/// Checks 10 s after start and every 6 h afterwards, while auto-update is enabled. A failed
/// check (no release published yet, offline) lengthens the following interval.
pub fn spawn_scheduler(app: AppHandle) {
    let spawned = std::thread::Builder::new()
        .name("glowdrift-updater".to_owned())
        .spawn(move || {
            std::thread::sleep(FIRST_CHECK_DELAY);
            let mut failures = 0_u32;
            loop {
                let enabled = app
                    .try_state::<AppState>()
                    .is_some_and(|state| state.auto_update_enabled());
                if enabled {
                    failures = if check_and_stage(&app) {
                        0
                    } else {
                        failures.saturating_add(1)
                    };
                }
                std::thread::sleep(next_check_delay(failures));
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
        .spawn(move || {
            check_and_stage(&app);
        });
    if let Err(e) = spawned {
        log::warn!("cannot start the update check: {e}");
    }
}

/// True for failures that are normal operating conditions (no release published yet, no
/// network): they are logged at debug level; anything else is a warning.
fn is_expected_failure(error: &UpdaterError) -> bool {
    matches!(
        error,
        UpdaterError::ReleaseNotFound | UpdaterError::Reqwest(_) | UpdaterError::Network(_)
    )
}

/// Runs one check and stages a found update. Returns false when the check failed.
fn check_and_stage(app: &AppHandle) -> bool {
    match try_check_and_stage(app) {
        Ok(_) => true,
        Err(e) => {
            if is_expected_failure(&e) {
                log::debug!("update check failed: {e}");
            } else {
                log::warn!("update check failed: {e}");
            }
            false
        }
    }
}

/// Checks for an update and stages it when found, on the calling thread. Returns the staged
/// version, or `None` when the app is up to date.
pub fn stage_now(app: &AppHandle) -> Result<Option<String>, UpdaterError> {
    try_check_and_stage(app)
}

fn try_check_and_stage(app: &AppHandle) -> Result<Option<String>, UpdaterError> {
    let hook_app = app.clone();
    let engine_stopped = Arc::new(AtomicBool::new(false));
    let hook_stopped = Arc::clone(&engine_stopped);
    let updater = app
        .updater_builder()
        .on_before_exit(move || {
            if let Some(state) = hook_app.try_state::<AppState>() {
                state.shutdown();
                hook_stopped.store(true, Ordering::Release);
            }
        })
        .build()?;
    let Some(update) = tauri::async_runtime::block_on(updater.check())? else {
        log::debug!("no update available");
        return Ok(None);
    };
    let Some(state) = app.try_state::<AppState>() else {
        return Ok(None);
    };
    let already_staged = state.pending_update.lock().is_ok_and(|p| {
        p.as_ref()
            .is_some_and(|p| p.update.version == update.version)
    });
    if already_staged {
        return Ok(Some(update.version));
    }
    let bytes = tauri::async_runtime::block_on(update.download(|_, _| {}, || {}))?;
    let version = update.version.clone();
    if let Ok(mut pending) = state.pending_update.lock() {
        *pending = Some(PendingUpdate {
            update,
            bytes,
            engine_stopped,
        });
    }
    log::info!("update {version} downloaded");
    let _ = app.emit(
        events::UPDATER_READY,
        UpdateReady {
            version: version.clone(),
        },
    );
    Ok(Some(version))
}

/// Installs the staged update. The engine is stopped first; on Windows a successful install
/// exits the process, so `Ok(true)` is only seen on platforms where it returns. A failed
/// install leaves the update staged so it can be retried.
pub fn install_pending(app: &AppHandle) -> Result<bool, String> {
    let Some(state) = app.try_state::<AppState>() else {
        return Ok(false);
    };
    install_staged(&state.pending_update, |pending| {
        pending.update.install(&pending.bytes).map_err(|e| {
            let mut message = format!("The update could not be installed: {e}.");
            if pending.engine_stopped.load(Ordering::Acquire) {
                message.push_str(" The lights are off until Glowdrift is restarted.");
            }
            message
        })
    })
}

/// Runs `install` on the staged value and clears the slot only when it succeeds.
fn install_staged<T>(
    slot: &Mutex<Option<T>>,
    install: impl FnOnce(&T) -> Result<(), String>,
) -> Result<bool, String> {
    let mut staged = slot
        .lock()
        .map_err(|_| "Update state is unavailable.".to_owned())?;
    let Some(value) = staged.as_ref() else {
        return Ok(false);
    };
    install(value)?;
    *staged = None;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_interval_doubles_per_failure_and_is_capped() {
        let hours: Vec<_> = [0, 1, 2, 3, 4, 100]
            .into_iter()
            .map(|n| next_check_delay(n).as_secs() / 3600)
            .collect();
        assert_eq!(hours, vec![6, 12, 24, 48, 48, 48]);
    }

    #[test]
    fn failed_install_keeps_the_update_staged() {
        let slot = Mutex::new(Some(7_u32));
        let failed = install_staged(&slot, |_| Err("disk full".to_owned()));
        assert_eq!(failed, Err("disk full".to_owned()));
        assert_eq!(*slot.lock().unwrap(), Some(7));
        assert_eq!(install_staged(&slot, |_| Ok(())), Ok(true));
        assert_eq!(*slot.lock().unwrap(), None);
        assert_eq!(install_staged(&slot, |_| Ok(())), Ok(false));
    }

    #[test]
    fn missing_release_and_network_errors_are_expected() {
        assert!(is_expected_failure(&UpdaterError::ReleaseNotFound));
        assert!(is_expected_failure(&UpdaterError::Network(
            "down".to_owned()
        )));
        assert!(!is_expected_failure(&UpdaterError::EmptyEndpoints));
    }
}
