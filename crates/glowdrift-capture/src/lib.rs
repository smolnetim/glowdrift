//! Windows monitor enumeration and DXGI Desktop Duplication capture.
//!
//! Frames are downscaled on the GPU so the CPU only ever sees a small BGRA8 buffer.
//!
//! # Behaviour notes
//!
//! - Capture uses DXGI Desktop Duplication on a D3D11 device created on the adapter that owns
//!   the monitor. The desktop image is downscaled with a mip chain (sRGB format when supported,
//!   so averaging happens in linear light) and only the chosen small mip is read back.
//! - On HDR desktops `DuplicateOutput` yields an SDR-mapped BGRA8 image, which is what is
//!   delivered; HDR values are not preserved.
//! - On rotated outputs the region is mapped onto the unrotated surface for 90 and 270 degrees
//!   when the surface is delivered unrotated. 180 degree rotation and the orientation of the
//!   returned pixels are not corrected (irrelevant for colour reduction); this is untested on
//!   real rotated hardware.
//! - Pixel coordinates and the GDI fallback assume a per-monitor DPI-aware process.
//! - Exclusive-fullscreen games, protected content, the lock screen and the UAC secure desktop
//!   can make duplication fail or return black; those surface as [`CaptureError::AccessLost`].

mod math;
#[cfg(windows)]
mod win;

use std::time::Duration;

use glowdrift_core::{FrameView, NormRect};
use serde::{Deserialize, Serialize};

/// Errors produced by capture operations.
#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    /// No monitor matches the requested id (or no monitor exists).
    #[error("monitor not found")]
    MonitorNotFound,
    /// The duplication session was lost (mode change, secure desktop, GPU reset).
    /// Recoverable: the caller recreates the [`Capturer`].
    #[error("desktop duplication access lost")]
    AccessLost,
    /// Capture is not available on this platform or configuration.
    #[error("capture is not supported here")]
    Unsupported,
    /// Any other operating-system failure.
    #[error("os error: {0}")]
    Os(String),
}

/// Where the time of the most recent [`Capturer::grab`] went.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GrabTimings {
    /// Inside `AcquireNextFrame`, including the wait for a desktop update.
    pub acquire: Duration,
    /// Copying the desktop image into the cache and releasing the duplication frame.
    pub store: Duration,
    /// Recording the downscale and readback copies on the immediate context.
    pub submit: Duration,
    /// Blocked in `Map` waiting for the GPU.
    pub map: Duration,
    /// Time between the desktop image shown in the returned frame being presented and the frame
    /// being returned; `None` when no frame was returned or the presentation time is unknown.
    pub latency: Option<Duration>,
}

/// A display attached to the desktop.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitorInfo {
    /// Stable identifier, usable as `monitorId` in the config.
    pub id: String,
    pub name: String,
    /// Left edge in virtual-desktop coordinates (may be negative).
    pub x: i32,
    /// Top edge in virtual-desktop coordinates (may be negative).
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub primary: bool,
    pub scale_factor: f32,
}

/// An owned BGRA8, sRGB-encoded frame.
#[derive(Debug, Clone)]
pub struct Frame {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// Bytes between the starts of consecutive rows.
    pub stride: usize,
}

impl Frame {
    /// Borrows the frame for reduction.
    pub fn view(&self) -> FrameView<'_> {
        FrameView::new(&self.data, self.width, self.height, self.stride)
    }
}

/// Lists all attached monitors, primary first, then left to right.
pub fn list_monitors() -> Result<Vec<MonitorInfo>, CaptureError> {
    #[cfg(windows)]
    {
        win::list_monitors()
    }
    #[cfg(not(windows))]
    {
        Ok(Vec::new())
    }
}

/// Captures one downscaled frame of an entire monitor, at most `max_width` pixels wide.
/// `None` selects the primary monitor.
///
/// Uses a short-lived Desktop Duplication session and falls back to a GDI grab when
/// duplication cannot deliver a frame (for example while this process already runs a
/// [`Capturer`] on that monitor; prefer [`Capturer::snapshot`] then). The GDI path shows SDR
/// content darker than duplication does on HDR desktops.
pub fn capture_thumbnail(monitor_id: Option<&str>, max_width: u32) -> Result<Frame, CaptureError> {
    #[cfg(windows)]
    {
        win::capture_thumbnail(monitor_id, max_width)
    }
    #[cfg(not(windows))]
    {
        let _ = (monitor_id, max_width);
        Err(CaptureError::Unsupported)
    }
}

/// A desktop-duplication session bound to one monitor.
///
/// Windows allows only one duplication of an output per process, so a second `Capturer` on the
/// same monitor fails with [`CaptureError::Os`] while the first is alive; drop it first.
/// [`capture_thumbnail`] handles this by falling back to a GDI grab.
///
/// A `Capturer` is meant to be driven from one dedicated thread. It is [`Send`] so it can be
/// created elsewhere and moved there, but not [`Sync`].
pub struct Capturer {
    #[cfg(windows)]
    inner: win::Duplicator,
    #[cfg(windows)]
    monitor: MonitorInfo,
}

impl std::fmt::Debug for Capturer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut s = f.debug_struct("Capturer");
        #[cfg(windows)]
        s.field("monitor", &self.monitor);
        s.finish_non_exhaustive()
    }
}

impl Capturer {
    /// Opens a capture session on `monitor_id` (`None` = primary). Frames come back downscaled
    /// to the smallest power-of-two reduction whose longest side is still at least
    /// `max_dimension` pixels (or the full region size when that is smaller), so the longest
    /// side is below `2 * max_dimension`.
    pub fn new(monitor_id: Option<&str>, max_dimension: u32) -> Result<Capturer, CaptureError> {
        #[cfg(windows)]
        {
            let entry = win::find_monitor(monitor_id)?;
            let inner = win::Duplicator::new(&entry, max_dimension)?;
            Ok(Capturer {
                inner,
                monitor: entry.info,
            })
        }
        #[cfg(not(windows))]
        {
            let _ = (monitor_id, max_dimension);
            Err(CaptureError::Unsupported)
        }
    }

    /// The monitor this session captures.
    #[cfg(windows)]
    pub fn monitor(&self) -> &MonitorInfo {
        &self.monitor
    }

    /// Timings of the most recent [`Capturer::grab`].
    #[cfg(windows)]
    pub fn last_timings(&self) -> GrabTimings {
        self.inner.timings()
    }

    /// Changes the downscale target for subsequent [`Capturer::grab`] calls. The next `grab`
    /// delivers a frame immediately from the last desktop image seen.
    pub fn set_max_dimension(&mut self, max_dimension: u32) {
        #[cfg(windows)]
        self.inner.set_max_dimension(max_dimension);
        #[cfg(not(windows))]
        let _ = max_dimension;
    }

    /// Returns the whole monitor, at most `max_width` pixels wide, from this session's latest
    /// desktop image, waiting up to `timeout` if no image has arrived yet. Use this instead of
    /// [`capture_thumbnail`] while a session is running: it works around the one-duplication-per-
    /// output limit and, unlike the GDI fallback, matches what duplication delivers on HDR
    /// desktops. The next [`Capturer::grab`] delivers a frame immediately.
    pub fn snapshot(&mut self, max_width: u32, timeout: Duration) -> Result<Frame, CaptureError> {
        #[cfg(windows)]
        {
            win::snapshot(&mut self.inner, max_width, timeout)
        }
        #[cfg(not(windows))]
        {
            let _ = (max_width, timeout);
            Err(CaptureError::Unsupported)
        }
    }

    /// Waits up to `timeout` for a new desktop frame and returns the downscaled `region` of
    /// it. `Ok(None)` means the screen did not change within the timeout (or the GPU needed
    /// longer than the timeout for the change, which the next call then delivers); pointer-only
    /// updates do not count as a change. When `region` differs from the previously delivered
    /// one, a frame is returned immediately from the last desktop image seen.
    /// [`CaptureError::AccessLost`] means the caller must recreate the capturer.
    pub fn grab(
        &mut self,
        region: NormRect,
        timeout: Duration,
    ) -> Result<Option<Frame>, CaptureError> {
        #[cfg(windows)]
        {
            self.inner.grab(region, timeout)
        }
        #[cfg(not(windows))]
        {
            let _ = (region, timeout);
            Err(CaptureError::Unsupported)
        }
    }
}
