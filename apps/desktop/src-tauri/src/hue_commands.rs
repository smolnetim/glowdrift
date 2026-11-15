//! IPC commands for the Hue bridge: discovery, pairing and choosing the area to drive.
//!
//! The library calls block on the network, so each runs on a blocking thread.

use std::time::Duration;

use glowdrift_core::Config;
use glowdrift_lights::hue::{
    self, AreaLightZone, BridgeCandidate, BridgeCredentials, BridgeLight, HueArea, HueError,
    PairOutcome,
};
use tauri::State;

use crate::commands::{AppState, update_config};
use crate::hue::{self as hue_state, HueBridge, StoredBridge};

const DISCOVER_TIMEOUT: Duration = Duration::from_secs(4);
const PAIR_INTERVAL: Duration = Duration::from_secs(2);
const PAIR_TIMEOUT: Duration = Duration::from_secs(30);
const REFRESH_TIMEOUT: Duration = Duration::from_secs(5);
const NOT_PAIRED: &str = "No Hue bridge is paired.";
const LINK_BUTTON_TIMEOUT: &str = "The link button was not pressed in time.";

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|e| format!("The Hue task failed: {e}"))?
}

/// Calls `attempt` every `interval` until it pairs, fails, or `timeout` has passed.
fn poll_pairing(
    mut attempt: impl FnMut() -> Result<PairOutcome, HueError>,
    mut sleep: impl FnMut(Duration),
    interval: Duration,
    timeout: Duration,
) -> Result<BridgeCredentials, String> {
    let mut waited = Duration::ZERO;
    loop {
        match attempt().map_err(|e| hue_state::describe(&e))? {
            PairOutcome::Paired(credentials) => return Ok(credentials),
            PairOutcome::LinkButtonNotPressed if waited >= timeout => {
                return Err(LINK_BUTTON_TIMEOUT.to_owned());
            }
            PairOutcome::LinkButtonNotPressed => {
                sleep(interval);
                waited += interval;
            }
        }
    }
}

#[tauri::command]
pub async fn hue_discover() -> Result<Vec<BridgeCandidate>, String> {
    tauri::async_runtime::spawn_blocking(|| hue::discover(DISCOVER_TIMEOUT))
        .await
        .map_err(|e| format!("The Hue task failed: {e}"))
}

#[tauri::command]
pub async fn hue_probe(address: String) -> Result<BridgeCandidate, String> {
    blocking(move || hue::probe(&address).map_err(|e| hue_state::describe(&e))).await
}

/// Pairs with the bridge at `address`, polling until the link button is pressed (up to about
/// 30 seconds), then stores the credentials and selects the bridge in the configuration.
#[tauri::command]
pub async fn hue_pair(address: String, state: State<'_, AppState>) -> Result<HueBridge, String> {
    let stored = blocking(move || {
        let name = hue::probe(&address)
            .map_err(|e| hue_state::describe(&e))?
            .name;
        let device_type = hue_state::local_device_type();
        let credentials = poll_pairing(
            || hue::pair(&address, &device_type),
            std::thread::sleep,
            PAIR_INTERVAL,
            PAIR_TIMEOUT,
        )?;
        Ok(StoredBridge { credentials, name })
    })
    .await?;
    let bridge = HueBridge::from(&stored);
    state.hue.save(stored)?;
    update_config(&state, |config| {
        let hue = &mut config.lights.hue;
        if hue.bridge_id.as_deref() != Some(bridge.id.as_str()) {
            hue.area_id = None;
        }
        hue.bridge_id = Some(bridge.id.clone());
    })?;
    Ok(bridge)
}

#[tauri::command]
pub async fn hue_bridge(state: State<'_, AppState>) -> Result<Option<HueBridge>, String> {
    Ok(state
        .hue
        .bridge(&state.hue_config())
        .as_ref()
        .map(HueBridge::from))
}

#[tauri::command]
pub async fn hue_list_areas(state: State<'_, AppState>) -> Result<Vec<HueArea>, String> {
    let credentials = state
        .hue
        .bridge(&state.hue_config())
        .ok_or_else(|| NOT_PAIRED.to_owned())?
        .credentials;
    blocking(move || hue::list_areas(&credentials).map_err(|e| hue_state::describe(&e))).await
}

/// The lights of the paired bridge that can be added as extra lights: every light except those of
/// the selected entertainment area.
#[tauri::command]
pub async fn hue_list_lights(state: State<'_, AppState>) -> Result<Vec<BridgeLight>, String> {
    let config = state.hue_config();
    let credentials = state
        .hue
        .bridge(&config)
        .ok_or_else(|| NOT_PAIRED.to_owned())?
        .credentials;
    let area_id = config.area_id;
    blocking(move || {
        hue::list_lights(&credentials, area_id.as_deref()).map_err(|e| hue_state::describe(&e))
    })
    .await
}

/// The lights the Hue light drives with the zone each samples; empty until the area is
/// resolved. Performs no I/O.
#[tauri::command]
pub fn hue_area_lights(state: State<'_, AppState>) -> Vec<AreaLightZone> {
    state.hue.area_lights()
}

/// Makes the Hue light read the bridge inventory again, waits until it has (at most about 5
/// seconds), and returns the lights as [`hue_area_lights`] does. Rejects when no bridge is
/// paired. The light reads the bridge on its own thread, at most once per two seconds; calls
/// made meanwhile share that read.
#[tauri::command]
pub async fn hue_refresh_area_lights(
    state: State<'_, AppState>,
) -> Result<Vec<AreaLightZone>, String> {
    if state.hue.bridge(&state.hue_config()).is_none() {
        return Err(NOT_PAIRED.to_owned());
    }
    let handle = state.hue.handle();
    blocking(move || {
        if !handle.refresh_and_wait(REFRESH_TIMEOUT) {
            log::debug!("the Hue light did not finish refreshing in time");
        }
        Ok(handle.area_lights().unwrap_or_default())
    })
    .await
}

#[tauri::command]
pub async fn hue_select_area(
    area_id: String,
    state: State<'_, AppState>,
) -> Result<Config, String> {
    let area_id = area_id.trim().to_owned();
    if area_id.is_empty() {
        return Err("Choose a Hue area.".to_owned());
    }
    if state.hue.bridge(&state.hue_config()).is_none() {
        return Err(NOT_PAIRED.to_owned());
    }
    update_config(&state, |config| config.lights.hue.area_id = Some(area_id))
}

#[tauri::command]
pub async fn hue_forget(state: State<'_, AppState>) -> Result<Config, String> {
    state.hue.clear()?;
    update_config(&state, |config| {
        config.lights.hue.bridge_id = None;
        config.lights.hue.area_id = None;
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn credentials() -> BridgeCredentials {
        BridgeCredentials {
            bridge_id: "A".to_owned(),
            address: "10.0.0.2".to_owned(),
            app_key: "key".to_owned(),
            client_key: String::new(),
            cert_sha256: "ab".to_owned(),
        }
    }

    #[test]
    fn pairing_succeeds_after_the_button_is_pressed() {
        let mut attempts = 0;
        let mut sleeps = Vec::new();
        let result = poll_pairing(
            || {
                attempts += 1;
                Ok(if attempts < 3 {
                    PairOutcome::LinkButtonNotPressed
                } else {
                    PairOutcome::Paired(credentials())
                })
            },
            |d| sleeps.push(d),
            PAIR_INTERVAL,
            PAIR_TIMEOUT,
        );
        assert_eq!(result.unwrap(), credentials());
        assert_eq!(sleeps, vec![PAIR_INTERVAL; 2]);
    }

    #[test]
    fn pairing_times_out_after_the_budget() {
        let mut attempts = 0;
        let result = poll_pairing(
            || {
                attempts += 1;
                Ok(PairOutcome::LinkButtonNotPressed)
            },
            |_| {},
            PAIR_INTERVAL,
            PAIR_TIMEOUT,
        );
        assert_eq!(result.unwrap_err(), LINK_BUTTON_TIMEOUT);
        assert_eq!(attempts, 16);
    }

    #[test]
    fn pairing_stops_on_a_bridge_error() {
        let mut attempts = 0;
        let result = poll_pairing(
            || {
                attempts += 1;
                Err(HueError::Network("down".to_owned()))
            },
            |_| panic!("must not wait after an error"),
            PAIR_INTERVAL,
            PAIR_TIMEOUT,
        );
        assert!(result.unwrap_err().starts_with("Could not reach"));
        assert_eq!(attempts, 1);
    }
}
