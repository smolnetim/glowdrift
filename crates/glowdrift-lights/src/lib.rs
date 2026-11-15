//! Lights that receive the smoothed colour.

#![deny(unsafe_code)]

pub mod hue;
pub mod icue;
pub mod null;

pub use null::NullLight;

use std::time::Duration;

use glowdrift_core::{NormRect, Rgb};
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
    /// The Hue light has no paired bridge.
    NotPaired,
    /// The Hue light has no usable area selected.
    NoArea,
}

/// Snapshot of a light for the UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LightStatus {
    pub id: String,
    /// Light family, e.g. `"icue"` or `"hue"`.
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

/// Screen zones a light wants colours for, as fractions of the monitor.
#[derive(Debug, Clone, PartialEq)]
pub struct ZoneLayout {
    /// Changes whenever `zones` changes, so colours computed for an older layout can be told
    /// apart from current ones.
    pub generation: u64,
    pub zones: Vec<NormRect>,
}

/// An ambient-lighting target.
pub trait Light: Send {
    fn id(&self) -> &str;
    fn status(&self) -> LightStatus;
    fn set_enabled(&mut self, enabled: bool);
    /// The shortest useful gap between colours sent to this light, which sets its maximum
    /// smoothing rate: colours arriving faster are dropped or coalesced. `None` means the
    /// light has no limit and can use any rate.
    fn min_send_interval(&self) -> Option<Duration>;
    /// Sends a colour. Must be cheap and non-blocking; implementations own their
    /// reconnect logic and drop colours while disconnected.
    fn set_color(&mut self, color: Rgb) -> Result<(), LightError>;
    /// The zones this light wants colours for; `None` means the light only uses the
    /// whole-region colour.
    fn zone_layout(&self) -> Option<ZoneLayout> {
        None
    }
    /// Sends per-zone colours for the layout `generation`, in layout order, together with the
    /// whole-region colour. Called instead of [`Light::set_color`] when the light has a
    /// layout; must be cheap and non-blocking. A generation that does not match the current
    /// layout falls back to `whole` for every zone. The default sends `whole`.
    fn set_zone_colors(
        &mut self,
        generation: u64,
        whole: Rgb,
        zones: &[Rgb],
    ) -> Result<(), LightError> {
        let _ = (generation, zones);
        self.set_color(whole)
    }
    /// Asks the light to retry connecting now instead of waiting for its next scheduled
    /// attempt, for example after a missing runtime library was installed.
    fn retry(&mut self) {}
    /// Releases devices and stops any background work.
    fn shutdown(&mut self);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Recorder(Vec<Rgb>);

    impl Light for Recorder {
        fn id(&self) -> &'static str {
            "recorder"
        }
        fn status(&self) -> LightStatus {
            unreachable!("not queried")
        }
        fn set_enabled(&mut self, _: bool) {}
        fn min_send_interval(&self) -> Option<Duration> {
            None
        }
        fn set_color(&mut self, color: Rgb) -> Result<(), LightError> {
            self.0.push(color);
            Ok(())
        }
        fn shutdown(&mut self) {}
    }

    #[test]
    fn default_zone_methods_fall_back_to_the_whole_colour() {
        let mut light = Recorder::default();
        assert_eq!(light.zone_layout(), None);
        let whole = Rgb::new(1, 2, 3);
        light
            .set_zone_colors(7, whole, &[Rgb::new(9, 9, 9), Rgb::new(8, 8, 8)])
            .unwrap();
        assert_eq!(light.0, vec![whole]);
    }
}
