//! Normalised and pixel rectangles.

use serde::{Deserialize, Serialize};

/// A rectangle in fractions (`0.0..=1.0`) of a monitor's size.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct NormRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl NormRect {
    /// Smallest width/height fraction kept by [`NormRect::clamped`].
    pub const MIN_SIZE: f32 = 0.01;

    pub const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// The whole monitor.
    pub const fn full() -> Self {
        Self::new(0.0, 0.0, 1.0, 1.0)
    }

    /// Returns a rectangle fully inside the unit square with at least [`NormRect::MIN_SIZE`]
    /// extent on each axis. NaN components fall back to the full-screen values.
    #[must_use]
    pub fn clamped(self) -> Self {
        fn fix(v: f32, fallback: f32) -> f32 {
            if v.is_nan() { fallback } else { v }
        }
        let width = fix(self.width, 1.0).clamp(Self::MIN_SIZE, 1.0);
        let height = fix(self.height, 1.0).clamp(Self::MIN_SIZE, 1.0);
        let x = fix(self.x, 0.0).clamp(0.0, 1.0 - width);
        let y = fix(self.y, 0.0).clamp(0.0, 1.0 - height);
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// Converts to a pixel rectangle inside a `w` x `h` surface. The result is clamped first,
    /// never empty (at least 1x1 when `w` and `h` are non-zero) and never exceeds the surface.
    pub fn to_pixels(self, w: u32, h: u32) -> PixelRect {
        let r = self.clamped();
        let axis = |start: f32, len: f32, total: u32| -> (u32, u32) {
            if total == 0 {
                return (0, 0);
            }
            let t = total as f32;
            let p0 = ((start * t).round() as u32).min(total - 1);
            let p1 = (((start + len) * t).round() as u32).clamp(p0 + 1, total);
            (p0, p1 - p0)
        };
        let (x, width) = axis(r.x, r.width, w);
        let (y, height) = axis(r.y, r.height, h);
        PixelRect {
            x,
            y,
            width,
            height,
        }
    }
}

impl Default for NormRect {
    fn default() -> Self {
        Self::full()
    }
}

/// A rectangle in whole pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PixelRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_maps_to_whole_surface() {
        assert_eq!(
            NormRect::full().to_pixels(1920, 1080),
            PixelRect {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080
            }
        );
    }

    #[test]
    fn clamped_keeps_rect_inside() {
        let r = NormRect::new(0.9, -1.0, 0.5, f32::NAN).clamped();
        assert!(r.x + r.width <= 1.0 + f32::EPSILON);
        assert!(r.y >= 0.0 && r.height == 1.0);
    }

    #[test]
    fn to_pixels_never_empty_or_out_of_bounds() {
        let p = NormRect::new(0.999, 0.999, 0.0, 0.0).to_pixels(100, 50);
        assert!(p.width >= 1 && p.height >= 1);
        assert!(p.x + p.width <= 100 && p.y + p.height <= 50);
    }
}
