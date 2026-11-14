//! Lights that receive the smoothed colour.

#![deny(unsafe_code)]

pub mod icue;
pub mod null;

pub use null::NullLight;

use glowdrift_core::Rgb;
use serde::{Deserialize, Serialize};

/// Errors a light can report from [`Light::set_color`].
#[derive(Debug, thiserror::Error)]
pub enum LightError {
    /// The light is not connected; the colour was dropped.
    #[error("light unavailable: {0}")]
    Unavailable(String),
    /// The underlying SDK or device reported a failure.
    #[error("light error: {0}")]
    Failed(String),
}

/// Connection state of a light.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LightState {
    Connected,
    Connecting,
    Unavailable,
    Error,
    Disabled,
}

/// Machine-readable reason behind an [`LightStatus`], for UI branches that must not parse
/// `detail`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LightCode {
    /// The light's runtime library is not installed in any searched location.
    LibraryMissing,
}

/// Snapshot of a light for the UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LightStatus {
    pub id: String,
    /// Light family, e.g. `"icue"`.
    pub kind: String,
    pub name: String,
    pub enabled: bool,
    pub state: LightState,
    pub detail: Option<String>,
    pub device_count: u32,
    /// Set when `detail` has a specific machine-readable cause.
    #[serde(default)]
    pub code: Option<LightCode>,
}

/// An ambient-lighting target.
pub trait Light: Send {
    fn id(&self) -> &str;
    fn status(&self) -> LightStatus;
    fn set_enabled(&mut self, enabled: bool);
    /// Sends a colour. Must be cheap and non-blocking; implementations own their
    /// reconnect logic and drop colours while disconnected.
    fn set_color(&mut self, color: Rgb) -> Result<(), LightError>;
    /// Asks the light to retry connecting now instead of waiting for its next scheduled
    /// attempt, for example after a missing runtime library was installed.
    fn retry(&mut self) {}
    /// Releases devices and stops any background work.
    fn shutdown(&mut self);
}
