//! Hue bridge credentials and the target of the Hue light.
//!
//! The credentials live in the encrypted [`hue_store`] file; the configuration only records
//! which bridge, area, extra lights and zone overrides are selected. [`HueState`] reconciles the
//! credentials and points the [`HueHandle`] of the running light at the result.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use glowdrift_core::NormRect;
use glowdrift_core::config::HueConfig;
use glowdrift_lights::hue::{AreaLightZone, BridgeCredentials, HueError, HueHandle, HueLight};
use serde::{Deserialize, Serialize};

use crate::hue_store;

/// Application name part of the pairing device type (the bridge accepts at most 20 characters).
const APP_NAME: &str = "glowdrift";
/// Longest device name part of the pairing device type the bridge accepts.
const DEVICE_NAME_MAX_CHARS: usize = 19;
/// Device name used when the computer name is unavailable or has no usable characters.
const FALLBACK_DEVICE_NAME: &str = "pc";

/// What is written to the credential store: the bridge credentials and its display name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredBridge {
    #[serde(flatten)]
    pub credentials: BridgeCredentials,
    #[serde(default)]
    pub name: Option<String>,
}

/// The paired bridge as shown to the UI. Carries no secrets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HueBridge {
    pub id: String,
    pub address: String,
    pub name: Option<String>,
}

impl From<&StoredBridge> for HueBridge {
    fn from(stored: &StoredBridge) -> Self {
        Self {
            id: stored.credentials.bridge_id.clone(),
            address: stored.credentials.address.clone(),
            name: stored.name.clone(),
        }
    }
}

/// Returns the stored bridge only when the configuration selects that same bridge.
pub fn reconcile(
    config_bridge_id: Option<&str>,
    stored: Option<StoredBridge>,
) -> Option<StoredBridge> {
    let stored = stored?;
    (config_bridge_id == Some(stored.credentials.bridge_id.as_str())).then_some(stored)
}

/// The `devicetype` sent when pairing: `glowdrift#<computer name>`, with the computer name
/// reduced to what the bridge accepts (no `#`, no control characters, at most 19 characters).
pub fn device_type(computer_name: &str) -> String {
    let device: String = computer_name
        .chars()
        .filter(|c| *c != '#' && !c.is_control())
        .take(DEVICE_NAME_MAX_CHARS)
        .collect();
    let device = device.trim();
    let device = if device.is_empty() {
        FALLBACK_DEVICE_NAME
    } else {
        device
    };
    format!("{APP_NAME}#{device}")
}

/// The `devicetype` for this computer.
pub fn local_device_type() -> String {
    device_type(&std::env::var("COMPUTERNAME").unwrap_or_default())
}

/// A message for the UI: the error's text as a sentence.
pub fn describe(error: &HueError) -> String {
    let text = error.to_string();
    let mut chars = text.chars();
    let mut sentence: String = chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default();
    if !sentence.ends_with('.') {
        sentence.push('.');
    }
    sentence
}

/// Reads the stored bridge. A missing store yields `None`; a store that cannot be decrypted or
/// parsed is logged and also yields `None`.
pub fn load_stored(dir: &Path) -> Option<StoredBridge> {
    let bytes = match hue_store::load(dir) {
        Ok(Some(bytes)) => bytes,
        Ok(None) => return None,
        Err(e) => {
            log::warn!(
                "the Hue credentials cannot be read; treating the bridge as not paired: {e}"
            );
            return None;
        }
    };
    match serde_json::from_slice::<StoredBridge>(&bytes) {
        Ok(stored) => Some(stored),
        Err(e) => {
            log::warn!(
                "the stored Hue credentials are invalid ({:?}); treating the bridge as not paired",
                e.classify()
            );
            None
        }
    }
}

/// The target the light should have for the stored bridge and the configuration: the bridge's
/// credentials when the configuration selects it, and the selected area only with a bridge.
pub fn target_for(stored: Option<StoredBridge>, config: &HueConfig) -> Target {
    let credentials =
        reconcile(config.bridge_id.as_deref(), stored).map(|stored| stored.credentials);
    let area = credentials.as_ref().and(config.area_id.clone());
    (credentials, area)
}

/// Credentials and area the light works with.
pub type Target = (Option<BridgeCredentials>, Option<String>);

struct Inner {
    stored: Option<StoredBridge>,
    applied: Target,
    applied_zones: BTreeMap<String, NormRect>,
    applied_extra: Vec<String>,
}

/// The Hue light's handle, the credential store location and the bridge read from it.
pub struct HueState {
    handle: HueHandle,
    dir: PathBuf,
    inner: Mutex<Inner>,
}

impl std::fmt::Debug for HueState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HueState").finish_non_exhaustive()
    }
}

impl HueState {
    /// Reads the credential store in `dir` and creates the Hue light aimed at the bridge and
    /// area `config` selects, together with the state that keeps it aimed there.
    pub fn start(dir: PathBuf, config: &HueConfig) -> (HueLight, Self) {
        let stored = load_stored(&dir);
        let target = target_for(stored.clone(), config);
        if config.bridge_id.is_some() && target.0.is_none() {
            log::info!("the selected Hue bridge has no matching stored credentials; not paired");
        }
        let (light, handle) = HueLight::new(target.0.clone(), target.1.clone());
        handle.set_zone_overrides(config.zones.clone());
        handle.set_extra_lights(config.extra_lights.clone());
        let state = Self {
            handle,
            dir,
            inner: Mutex::new(Inner {
                stored,
                applied: target,
                applied_zones: config.zones.clone(),
                applied_extra: config.extra_lights.clone(),
            }),
        };
        (light, state)
    }

    fn inner(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The paired bridge, or `None` when nothing is stored or the configuration selects another
    /// bridge.
    pub fn bridge(&self, config: &HueConfig) -> Option<StoredBridge> {
        reconcile(config.bridge_id.as_deref(), self.inner().stored.clone())
    }

    /// Encrypts and writes `bridge` to the store and makes it the stored bridge. The light is
    /// retargeted by the next [`sync`](Self::sync).
    pub fn save(&self, bridge: StoredBridge) -> Result<(), String> {
        let bytes = serde_json::to_vec(&bridge).map_err(|e| e.to_string())?;
        hue_store::save(&self.dir, &bytes)
            .map_err(|e| format!("The Hue credentials could not be saved: {e}"))?;
        self.inner().stored = Some(bridge);
        Ok(())
    }

    /// Deletes the store and forgets the stored bridge.
    pub fn clear(&self) -> Result<(), String> {
        hue_store::clear(&self.dir)
            .map_err(|e| format!("The Hue credentials could not be removed: {e}"))?;
        self.inner().stored = None;
        Ok(())
    }

    /// Points the light at the bridge and area `config` selects and hands it the zone overrides
    /// and extra lights, each only when it differs from what the light already has.
    pub fn sync(&self, config: &HueConfig) {
        let mut inner = self.inner();
        let target = target_for(inner.stored.clone(), config);
        if inner.applied != target {
            self.handle.set_target(target.0.clone(), target.1.clone());
            inner.applied = target;
        }
        if inner.applied_zones != config.zones {
            self.handle.set_zone_overrides(config.zones.clone());
            inner.applied_zones.clone_from(&config.zones);
        }
        if inner.applied_extra != config.extra_lights {
            self.handle.set_extra_lights(config.extra_lights.clone());
            inner.applied_extra.clone_from(&config.extra_lights);
        }
    }

    /// The lights the Hue light drives with the zone each samples; empty until the area is
    /// resolved. Performs no I/O.
    pub fn area_lights(&self) -> Vec<AreaLightZone> {
        self.handle.area_lights().unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stored(id: &str) -> StoredBridge {
        StoredBridge {
            credentials: BridgeCredentials {
                bridge_id: id.to_owned(),
                address: "10.0.0.2".to_owned(),
                app_key: "key".to_owned(),
                client_key: "client".to_owned(),
                cert_sha256: "ab".to_owned(),
            },
            name: Some("Hue".to_owned()),
        }
    }

    #[test]
    fn area_lights_are_empty_until_the_area_is_resolved() {
        let dir = std::env::temp_dir().join("glowdrift-test-no-hue-store");
        let config = HueConfig::default();
        let (_light, state) = HueState::start(dir, &config);
        assert_eq!(state.area_lights(), Vec::new());
        let changed = HueConfig {
            extra_lights: vec!["a".to_owned()],
            ..config
        };
        state.sync(&changed);
        state.sync(&changed);
        assert_eq!(state.inner().applied_extra, vec!["a".to_owned()]);
    }

    #[test]
    fn reconcile_requires_matching_bridge_id() {
        assert!(reconcile(Some("A"), Some(stored("A"))).is_some());
        assert!(reconcile(Some("B"), Some(stored("A"))).is_none());
        assert!(reconcile(None, Some(stored("A"))).is_none());
        assert!(reconcile(Some("A"), None).is_none());
    }

    #[test]
    fn target_needs_a_matching_bridge_and_keeps_the_area_only_with_it() {
        let config = HueConfig {
            enabled: true,
            bridge_id: Some("A".to_owned()),
            area_id: Some("room".to_owned()),
            ..HueConfig::default()
        };
        let (creds, area) = target_for(Some(stored("A")), &config);
        assert_eq!(creds, Some(stored("A").credentials));
        assert_eq!(area.as_deref(), Some("room"));

        assert_eq!(target_for(Some(stored("B")), &config), (None, None));
        assert_eq!(target_for(None, &config), (None, None));
        let unselected = HueConfig {
            bridge_id: None,
            ..config
        };
        assert_eq!(target_for(Some(stored("A")), &unselected), (None, None));
    }

    #[test]
    fn stored_bridge_round_trips_and_reads_plain_credentials() {
        let original = stored("A");
        let json = serde_json::to_vec(&original).unwrap();
        assert_eq!(
            serde_json::from_slice::<StoredBridge>(&json).unwrap(),
            original
        );

        let plain = serde_json::to_vec(&original.credentials).unwrap();
        let read: StoredBridge = serde_json::from_slice(&plain).unwrap();
        assert_eq!(read.credentials, original.credentials);
        assert_eq!(read.name, None);
    }

    #[test]
    fn bridge_view_carries_no_secrets() {
        let json = serde_json::to_string(&HueBridge::from(&stored("A"))).unwrap();
        assert_eq!(json, r#"{"id":"A","address":"10.0.0.2","name":"Hue"}"#);
    }

    #[test]
    fn device_type_stays_within_bridge_limits() {
        assert_eq!(device_type("DESKTOP-1"), "glowdrift#DESKTOP-1");
        let long = device_type("A-VERY-LONG-COMPUTER-NAME-OF-MANY-CHARACTERS");
        assert_eq!(long, "glowdrift#A-VERY-LONG-COMPUTE");
        let (app, device) = long.split_once('#').unwrap();
        assert!(app.chars().count() <= 20);
        assert!(device.chars().count() <= 19);
    }

    #[test]
    fn device_type_sanitises_the_name() {
        assert_eq!(device_type("a#b"), "glowdrift#ab");
        assert_eq!(device_type(""), "glowdrift#pc");
        assert_eq!(device_type("  \t#"), "glowdrift#pc");
        let wide = device_type(&"\u{e4}".repeat(30));
        assert_eq!(wide.split_once('#').unwrap().1.chars().count(), 19);
    }

    #[test]
    fn errors_are_described_as_sentences() {
        assert_eq!(
            describe(&HueError::Unauthorized),
            "The Hue bridge rejected the app key."
        );
        assert_eq!(
            describe(&HueError::AreaNotFound),
            "The Hue area was not found or has no lights."
        );
    }
}
