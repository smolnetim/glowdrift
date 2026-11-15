//! A light that records colours and talks to no hardware.

use std::time::Duration;

use glowdrift_core::Rgb;

use crate::{Light, LightError, LightState, LightStatus};

/// Light id reported by [`NullLight`].
pub const NULL_LIGHT_ID: &str = "null";

/// Accepts colours and remembers the latest one. Useful in tests and as a stand-in light.
#[derive(Debug)]
pub struct NullLight {
    enabled: bool,
    last_color: Option<Rgb>,
    color_count: u64,
}

impl NullLight {
    pub fn new() -> NullLight {
        NullLight {
            enabled: true,
            last_color: None,
            color_count: 0,
        }
    }

    /// The most recent colour received while enabled.
    pub fn last_color(&self) -> Option<Rgb> {
        self.last_color
    }

    /// Number of colours received while enabled.
    pub fn color_count(&self) -> u64 {
        self.color_count
    }
}

impl Default for NullLight {
    fn default() -> Self {
        Self::new()
    }
}

impl Light for NullLight {
    fn id(&self) -> &str {
        NULL_LIGHT_ID
    }

    fn status(&self) -> LightStatus {
        LightStatus {
            id: NULL_LIGHT_ID.to_owned(),
            kind: "null".to_owned(),
            name: "Null light".to_owned(),
            enabled: self.enabled,
            state: if self.enabled {
                LightState::Connected
            } else {
                LightState::Disabled
            },
            detail: None,
            device_count: 0,
            code: None,
        }
    }

    fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    fn min_send_interval(&self) -> Option<Duration> {
        None
    }

    fn set_color(&mut self, color: Rgb) -> Result<(), LightError> {
        if !self.enabled {
            return Err(LightError::Unavailable("light is disabled".to_owned()));
        }
        self.last_color = Some(color);
        self.color_count += 1;
        Ok(())
    }

    fn shutdown(&mut self) {
        self.enabled = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_colours_while_enabled() {
        let mut out = NullLight::new();
        assert_eq!(out.status().state, LightState::Connected);
        out.set_color(Rgb::new(1, 2, 3)).unwrap();
        out.set_color(Rgb::new(4, 5, 6)).unwrap();
        assert_eq!(out.last_color(), Some(Rgb::new(4, 5, 6)));
        assert_eq!(out.color_count(), 2);
    }

    #[test]
    fn declares_no_rate_limit() {
        assert_eq!(NullLight::new().min_send_interval(), None);
    }

    #[test]
    fn rejects_colours_when_disabled() {
        let mut out = NullLight::new();
        out.set_enabled(false);
        assert_eq!(out.status().state, LightState::Disabled);
        assert!(matches!(
            out.set_color(Rgb::new(1, 1, 1)),
            Err(LightError::Unavailable(_))
        ));
        assert_eq!(out.last_color(), None);
    }
}
