//! Where the engine gets its frames from: the desktop in the app, a scripted source in tests.

use std::time::Duration;

use glowdrift_capture::{CaptureError, Capturer, GrabTimings};
use glowdrift_core::{FrameView, NormRect};

/// A session that delivers downscaled frames of a screen region.
pub trait FrameSource: Send {
    /// Waits up to `timeout` for a frame of `region`; `Ok(None)` means nothing changed. The frame
    /// borrows a buffer owned by the source and is valid until the next call.
    /// [`CaptureError::AccessLost`] means the session must be reopened.
    fn grab(
        &mut self,
        region: NormRect,
        timeout: Duration,
    ) -> Result<Option<FrameView<'_>>, CaptureError>;

    /// Where the time of the most recent grab went, for sources that measure it.
    fn last_timings(&self) -> Option<GrabTimings> {
        None
    }
}

/// Opens a [`FrameSource`] for a monitor id (`None` is the primary monitor) and the longest side,
/// in pixels, of the frames it should deliver.
pub type OpenSource =
    Box<dyn FnMut(Option<&str>, u32) -> Result<Box<dyn FrameSource>, CaptureError> + Send>;

impl FrameSource for Capturer {
    fn grab(
        &mut self,
        region: NormRect,
        timeout: Duration,
    ) -> Result<Option<FrameView<'_>>, CaptureError> {
        Capturer::grab(self, region, timeout)
    }

    #[cfg(windows)]
    fn last_timings(&self) -> Option<GrabTimings> {
        Some(Capturer::last_timings(self))
    }
}

/// Opens desktop capture sessions.
pub fn open_desktop() -> OpenSource {
    Box::new(|monitor_id, max_dimension| {
        Capturer::new(monitor_id, max_dimension)
            .map(|capturer| Box::new(capturer) as Box<dyn FrameSource>)
    })
}
