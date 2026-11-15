//! Philips Hue light over the bridge's CLIP v2 REST API.
//!
//! [`discover`] and [`probe`] find a bridge, [`pair`] registers this app (one attempt per
//! call; the caller polls while the user presses the link button), [`list_areas`] returns the
//! entertainment areas a user can pick and [`list_lights`] the single lights that can be added.
//! [`HueLight`] then sends colours to the lights of the chosen area plus any extra lights with
//! `PUT /clip/v2/resource/light/{id}`.
//!
//! # Trust
//!
//! Bridges use certificates no public CA signs. [`pair`] records the SHA-256 of the bridge's
//! leaf certificate; every later connection requires an exact match (trust on first use), and
//! a changed certificate is reported as [`HueError::CertificateMismatch`].
//!
//! # Output
//!
//! At most 10 PUTs per second go out in total, one light per slot, round-robin over the lights
//! whose colour changed. With `n` lights a light is therefore refreshed at most every
//! `n * 100 ms`, which is also the transition duration the bridge fades over and the value of
//! [`Light::min_send_interval`]. Colours are coalesced: only the latest is ever sent, nothing
//! queues. Near-black switches a light off; see [`color`] for the colour and brightness
//! mapping.
//!
//! # Zones
//!
//! The lights driven are the lights of the entertainment area in channel order followed by the
//! extra lights. Each has a zone of the monitor: an override from
//! [`HueHandle::set_zone_overrides`], else the default for its channel position (a light in
//! several channels gets the average position) or, for extra lights, a centred box. The
//! layout is available once the lights are resolved ([`Light::zone_layout`]); its generation
//! changes when the lights or a zone change. [`Light::set_zone_colors`] gives each light its
//! own colour; a colour computed for another generation is replaced by the whole-region colour
//! on every light.
//!
//! [`HueHandle::refresh`] re-reads the bridge inventory over the open connection, so moved
//! lights, renamed lights and changed area members show up without reconnecting; kept light
//! states and colours stay, and the generation changes only if a light or an effective zone
//! did.
//!
//! # Restore
//!
//! Just before the first colour is sent to a light, its state (on/off, brightness, colour or
//! colour temperature) is read. It is put back, fading over 400 ms, when the light is
//! disabled, the bridge is forgotten, the target becomes empty or the light shuts down, and
//! for lights that stop being driven when the area or extra lights change. A state is restored
//! once; lights that cannot be reached are logged and skipped. The state is read again for
//! lights that start being driven later.
//!
//! # Threading
//!
//! The free functions block on network I/O with short timeouts and must not be called from an
//! async runtime thread (the blocking HTTP client panics there); use a blocking thread.
//! [`HueLight`] and [`HueHandle`] never block the caller.

mod api;
mod client;
pub mod color;
mod discovery;
mod light;
mod pacer;
mod worker;

#[cfg(test)]
mod tests;

use serde::{Deserialize, Serialize};
use serde_json::json;

use self::api::{ConfigReply, Inventory, PairReply, normalize_bridge_id, parse_pair_reply};
use self::client::{Bridge, REQUEST_TIMEOUT};

pub use self::discovery::{discover, probe};
pub use self::light::{AreaLightZone, HueHandle, HueLight, ZoneSource};

/// Light id reported by [`HueLight`].
pub const HUE_LIGHT_ID: &str = "hue";

/// A bridge found on the network.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeCandidate {
    /// Bridge id, 16 uppercase hex digits.
    pub id: String,
    /// IP address or host name, with `:port` if not 443.
    pub address: String,
    pub name: Option<String>,
}

/// Everything needed to talk to a paired bridge. Secrets are redacted from `Debug`.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeCredentials {
    pub bridge_id: String,
    pub address: String,
    pub app_key: String,
    /// Key for entertainment streaming; empty if the bridge did not issue one.
    pub client_key: String,
    /// Lowercase hex SHA-256 of the bridge's leaf certificate.
    pub cert_sha256: String,
}

impl std::fmt::Debug for BridgeCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BridgeCredentials")
            .field("bridge_id", &self.bridge_id)
            .field("address", &self.address)
            .field("app_key", &"<redacted>")
            .field("client_key", &"<redacted>")
            .field("cert_sha256", &self.cert_sha256)
            .finish()
    }
}

/// Result of one pairing attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairOutcome {
    Paired(BridgeCredentials),
    /// The link button has not been pressed; try again shortly.
    LinkButtonNotPressed,
}

/// Kind of a [`HueArea`]. Only entertainment areas are listed; the other kinds remain for
/// consumers of the serialised form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AreaKind {
    Room,
    Zone,
    Entertainment,
}

/// A group of lights the user can pick as the output target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HueArea {
    pub id: String,
    pub kind: AreaKind,
    pub name: String,
    pub light_count: u32,
}

/// A single light of a bridge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeLight {
    pub id: String,
    pub name: String,
}

/// Errors from talking to a bridge.
#[derive(Debug, thiserror::Error)]
pub enum HueError {
    /// The bridge could not be reached or the connection failed.
    #[error("could not reach the Hue bridge: {0}")]
    Network(String),
    /// The bridge presented a certificate other than the one recorded when pairing.
    #[error("the Hue bridge's certificate does not match the one recorded when pairing")]
    CertificateMismatch,
    /// The bridge rejected the app key.
    #[error("the Hue bridge rejected the app key")]
    Unauthorized,
    /// The bridge answered with an error.
    #[error("Hue bridge error: {0}")]
    Bridge(String),
    /// The bridge's answer was not in the expected format.
    #[error("unexpected Hue bridge reply: {0}")]
    Parse(String),
    /// The address is not a plain host name or IP address with an optional port.
    #[error("invalid Hue bridge address: {0}")]
    InvalidAddress(String),
    /// The stored credentials are incomplete.
    #[error("invalid Hue credentials: {0}")]
    InvalidCredentials(String),
    /// Neither the chosen area nor any extra light exists on the bridge.
    #[error("the Hue area was not found or has no lights")]
    AreaNotFound,
}

/// Installs the ring crypto provider as the process default if none is set, for the plain
/// (not pinned) HTTPS client used by cloud discovery.
fn ensure_tls_provider() {
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        // Fails only when another provider was installed concurrently, which is equally fine.
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
}

/// One pairing attempt with the bridge at `address`: `POST /api` with a new device type.
///
/// Returns [`PairOutcome::LinkButtonNotPressed`] until the user presses the bridge's link
/// button; call again every second or two for about 30 seconds. On success the bridge's
/// certificate fingerprint seen during this attempt is pinned in the credentials.
pub fn pair(address: &str, device_type: &str) -> Result<PairOutcome, HueError> {
    let bridge = Bridge::new(address, None, None, REQUEST_TIMEOUT)?;
    let config: ConfigReply = bridge.get_v1("/api/0/config")?;
    let bridge_id = normalize_bridge_id(&config.bridgeid);
    if bridge_id.is_empty() {
        return Err(HueError::Parse(
            "the device did not report a bridge id".to_owned(),
        ));
    }
    let reply = bridge.post_v1(
        "/api",
        &json!({ "devicetype": device_type, "generateclientkey": true }),
    )?;
    match parse_pair_reply(&reply)? {
        PairReply::LinkButtonNotPressed => Ok(PairOutcome::LinkButtonNotPressed),
        PairReply::Success {
            username,
            client_key,
        } => {
            let cert_sha256 = bridge
                .pin()
                .ok_or_else(|| HueError::Parse("no certificate was presented".to_owned()))?;
            Ok(PairOutcome::Paired(BridgeCredentials {
                bridge_id,
                address: address.trim().to_owned(),
                app_key: username,
                client_key,
                cert_sha256,
            }))
        }
    }
}

/// Lists the entertainment areas of a paired bridge, sorted by name. Rooms and zones are not
/// offered.
pub fn list_areas(creds: &BridgeCredentials) -> Result<Vec<HueArea>, HueError> {
    let bridge = pinned_bridge(creds)?;
    Ok(Inventory::fetch(&bridge)?
        .areas()
        .into_iter()
        .map(|(area, _)| area)
        .collect())
}

/// Lists the lights of a paired bridge that can be added as extra lights: colour-capable
/// lights first, each group sorted by name. With `area_id`, the lights of that entertainment
/// area are left out, since the area already drives them.
pub fn list_lights(
    creds: &BridgeCredentials,
    area_id: Option<&str>,
) -> Result<Vec<BridgeLight>, HueError> {
    let bridge = pinned_bridge(creds)?;
    Ok(match area_id {
        Some(area_id) => Inventory::fetch(&bridge)?.bridge_lights_outside(area_id),
        None => Inventory::fetch_lights(&bridge)?.bridge_lights(),
    })
}

fn pinned_bridge(creds: &BridgeCredentials) -> Result<Bridge, HueError> {
    if creds.cert_sha256.is_empty() {
        return Err(HueError::InvalidCredentials(
            "no certificate pin recorded".to_owned(),
        ));
    }
    Bridge::new(
        &creds.address,
        Some(&creds.cert_sha256),
        Some(&creds.app_key),
        REQUEST_TIMEOUT,
    )
}
