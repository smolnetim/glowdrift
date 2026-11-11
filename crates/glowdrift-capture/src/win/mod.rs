//! Windows implementation: DXGI enumeration, Desktop Duplication and a GDI fallback.

use std::time::Duration;

use crate::math::{downscale_bgra, fit_width};
use crate::{CaptureError, Frame, MonitorInfo};

mod duplication;
mod error;
mod gdi;
mod monitors;

pub(crate) use duplication::Duplicator;
pub(crate) use monitors::MonitorEntry;

const THUMBNAIL_WAIT: Duration = Duration::from_millis(1500);

pub(crate) fn list_monitors() -> Result<Vec<MonitorInfo>, CaptureError> {
    Ok(monitors::enumerate()?.into_iter().map(|e| e.info).collect())
}

pub(crate) fn find_monitor(monitor_id: Option<&str>) -> Result<MonitorEntry, CaptureError> {
    monitors::find(monitor_id)
}

fn thumbnail_duplication(entry: &MonitorEntry, max_width: u32) -> Result<Frame, CaptureError> {
    let mut duplicator = Duplicator::new(entry, 1)?;
    snapshot(&mut duplicator, max_width, THUMBNAIL_WAIT)
}

/// Whole-monitor frame at most `max_width` wide from the session's latest desktop image.
pub(crate) fn snapshot(
    duplicator: &mut Duplicator,
    max_width: u32,
    timeout: Duration,
) -> Result<Frame, CaptureError> {
    Ok(fit_frame(
        duplicator.snapshot(max_width, timeout)?,
        max_width,
    ))
}

/// Shrinks `frame` to at most `max_width` wide, keeping the aspect ratio.
pub(crate) fn fit_frame(frame: Frame, max_width: u32) -> Frame {
    let (w, h) = fit_width(frame.width, frame.height, max_width);
    if (w, h) == (frame.width, frame.height) && frame.stride == frame.width as usize * 4 {
        return frame;
    }
    let data = downscale_bgra(&frame.data, frame.width, frame.height, frame.stride, w, h);
    Frame {
        data,
        width: w,
        height: h,
        stride: w as usize * 4,
    }
}

pub(crate) fn capture_thumbnail(
    monitor_id: Option<&str>,
    max_width: u32,
) -> Result<Frame, CaptureError> {
    let entry = find_monitor(monitor_id)?;
    let frame = match thumbnail_duplication(&entry, max_width) {
        Ok(frame) => frame,
        Err(error) => {
            log::debug!("duplication thumbnail failed ({error}); using GDI");
            gdi::capture_full(&entry)?
        }
    };
    Ok(fit_frame(frame, max_width))
}

#[cfg(test)]
mod tests {
    use super::*;
    use glowdrift_core::NormRect;

    #[test]
    #[ignore = "needs an interactive desktop"]
    fn gdi_fallback_captures_a_frame() {
        let entry = find_monitor(None).expect("primary monitor");
        let frame = fit_frame(gdi::capture_full(&entry).expect("gdi grab"), 320);
        assert!(frame.width <= 320 && frame.width > 0 && frame.height > 0);
        assert!(frame.view().is_valid());
        assert!(frame.data.as_chunks::<4>().0.iter().all(|p| p[3] == 255));
    }

    #[test]
    #[ignore = "needs an interactive desktop; assumes the screen is static for a moment"]
    fn gpu_downscale_matches_cpu_linear_average() {
        use glowdrift_core::{ReduceMode, reduce};
        let entry = find_monitor(None).expect("primary monitor");
        let grab = |max_dimension: u32| {
            let mut duplicator = Duplicator::new(&entry, max_dimension).expect("duplicator");
            duplicator
                .grab(NormRect::full(), Duration::from_secs(2))
                .expect("grab")
                .expect("frame")
        };
        let small = grab(32);
        let full = grab(u32::MAX);
        assert!(small.width < full.width / 8);
        let gpu = reduce(&small.view(), ReduceMode::Average).to_srgb();
        let cpu = reduce(&full.view(), ReduceMode::Average).to_srgb();
        println!("gpu {} cpu {}", gpu.to_hex(), cpu.to_hex());
        for (a, b) in [(gpu.r, cpu.r), (gpu.g, cpu.g), (gpu.b, cpu.b)] {
            assert!(a.abs_diff(b) <= 12, "gpu {gpu:?} vs cpu {cpu:?}");
        }
    }
}
