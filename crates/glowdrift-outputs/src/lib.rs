//! Output sinks that receive the smoothed colour.

#![deny(unsafe_code)]

pub mod icue;
pub mod null;

pub use null::NullOutput;

use glowdrift_core::Rgb;
use serde::{Deserialize, Serialize};

/// Errors an output can report from [`Output::set_color`].
#[derive(Debug, thiserror::Error)]
pub enum OutputError {
    /// The output is not connected; the colour was dropped.
    #[error("output unavailable: {0}")]
    Unavailable(String),
    /// The underlying SDK or device reported a failure.
    #[error("output error: {0}")]
    Failed(String),
}

/// Connection state of an output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputState {
    Connected,
    Connecting,
    Unavailable,
    Error,
    Disabled,
}

/// Snapshot of an output for the UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputStatus {
    pub id: String,
    /// Output family, e.g. `"icue"`.
    pub kind: String,
    pub name: String,
    pub enabled: bool,
    pub state: OutputState,
    pub detail: Option<String>,
    pub device_count: u32,
}

/// An ambient-lighting target.
pub trait Output: Send {
    fn id(&self) -> &str;
    fn status(&self) -> OutputStatus;
    fn set_enabled(&mut self, enabled: bool);
    /// Sends a colour. Must be cheap and non-blocking; implementations own their
    /// reconnect logic and drop colours while disconnected.
    fn set_color(&mut self, color: Rgb) -> Result<(), OutputError>;
    /// Releases devices and stops any background work.
    fn shutdown(&mut self);
}
