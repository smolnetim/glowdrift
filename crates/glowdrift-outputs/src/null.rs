//! An output that records colours and talks to no hardware.

use glowdrift_core::Rgb;

use crate::{Output, OutputError, OutputState, OutputStatus};

/// Output id reported by [`NullOutput`].
pub const NULL_OUTPUT_ID: &str = "null";

/// Accepts colours and remembers the latest one. Useful in tests and as a stand-in sink.
#[derive(Debug)]
pub struct NullOutput {
    enabled: bool,
    last_color: Option<Rgb>,
    color_count: u64,
}

impl NullOutput {
    pub fn new() -> NullOutput {
        NullOutput {
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

impl Default for NullOutput {
    fn default() -> Self {
        Self::new()
    }
}

impl Output for NullOutput {
    fn id(&self) -> &str {
        NULL_OUTPUT_ID
    }

    fn status(&self) -> OutputStatus {
        OutputStatus {
            id: NULL_OUTPUT_ID.to_owned(),
            kind: "null".to_owned(),
            name: "Null output".to_owned(),
            enabled: self.enabled,
            state: if self.enabled {
                OutputState::Connected
            } else {
                OutputState::Disabled
            },
            detail: None,
            device_count: 0,
            code: None,
        }
    }

    fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    fn set_color(&mut self, color: Rgb) -> Result<(), OutputError> {
        if !self.enabled {
            return Err(OutputError::Unavailable("output is disabled".to_owned()));
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
        let mut out = NullOutput::new();
        assert_eq!(out.status().state, OutputState::Connected);
        out.set_color(Rgb::new(1, 2, 3)).unwrap();
        out.set_color(Rgb::new(4, 5, 6)).unwrap();
        assert_eq!(out.last_color(), Some(Rgb::new(4, 5, 6)));
        assert_eq!(out.color_count(), 2);
    }

    #[test]
    fn rejects_colours_when_disabled() {
        let mut out = NullOutput::new();
        out.set_enabled(false);
        assert_eq!(out.status().state, OutputState::Disabled);
        assert!(matches!(
            out.set_color(Rgb::new(1, 1, 1)),
            Err(OutputError::Unavailable(_))
        ));
        assert_eq!(out.last_color(), None);
    }
}
