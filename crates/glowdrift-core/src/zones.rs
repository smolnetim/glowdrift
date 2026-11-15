//! Per-light screen zones: where each light samples the monitor, and the reduction of every
//! zone of a frame to one colour. Zones are fractions of the monitor.
//!
//! # Default zones
//!
//! [`default_zones`] maps Hue entertainment positions to rectangles of the monitor:
//!
//! - The centre x is `(x + 1) / 2` (left to right) and the centre y is `(1 - z) / 2`
//!   (ceiling at the top, floor at the bottom). Depth (`y`) is ignored.
//! - A light with a position gets a zone [`ZONE_WIDTH`] wide and [`ZONE_HEIGHT`] tall around
//!   that centre, moved to stay inside the monitor.
//! - A light without a position (or with a non-finite coordinate) gets a box centred on the
//!   monitor, [`EXTRA_ZONE_WIDTH`] wide and as tall as the monitor.
//! - When no light has a position, the monitor is split into as many equal full-height
//!   columns as there are lights, one per light in order.

use serde::{Deserialize, Serialize};

use crate::color::LinearRgb;
use crate::config::ReduceMode;
use crate::frame::FrameView;
use crate::geometry::NormRect;
use crate::reduce::reduce;

/// Width of the default zone of a light without a position (when other lights have one), as a
/// fraction of the monitor.
pub const EXTRA_ZONE_WIDTH: f32 = 1.0 / 5.0;
/// Width of the default zone of a light with a position, as a fraction of the monitor.
pub const ZONE_WIDTH: f32 = 1.0 / 3.0;
/// Height of the default zone of a light with a position, as a fraction of the monitor.
pub const ZONE_HEIGHT: f32 = 2.0 / 3.0;

/// Hue entertainment coordinates, each `-1.0..=1.0`: `x` left to right, `y` back to front,
/// `z` floor to ceiling.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LightPosition {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl LightPosition {
    /// The zone centre `(x, y)` as fractions of the monitor, or `None` when a coordinate used
    /// for it is not finite.
    fn centre(self) -> Option<(f32, f32)> {
        (self.x.is_finite() && self.z.is_finite()).then(|| {
            (
                f32::midpoint(self.x, 1.0).clamp(0.0, 1.0),
                f32::midpoint(1.0, -self.z).clamp(0.0, 1.0),
            )
        })
    }
}

/// The default zone of each light, in the same order as `positions`. Rectangles are fractions
/// of the monitor and lie inside it; see the module documentation for the mapping.
pub fn default_zones(positions: &[Option<LightPosition>]) -> Vec<NormRect> {
    let count = positions.len();
    let centred: Vec<Option<(f32, f32)>> = positions
        .iter()
        .map(|p| p.and_then(LightPosition::centre))
        .collect();
    if centred.iter().all(Option::is_none) {
        let width = 1.0 / count.max(1) as f32;
        return (0..count)
            .map(|i| NormRect::new(i as f32 * width, 0.0, width, 1.0).clamped())
            .collect();
    }
    centred
        .iter()
        .map(|centre| match centre {
            Some((cx, cy)) => NormRect::new(
                cx - ZONE_WIDTH / 2.0,
                cy - ZONE_HEIGHT / 2.0,
                ZONE_WIDTH,
                ZONE_HEIGHT,
            )
            .clamped(),
            None => NormRect::new((1.0 - EXTRA_ZONE_WIDTH) / 2.0, 0.0, EXTRA_ZONE_WIDTH, 1.0),
        })
        .collect()
}

/// The smallest rectangle containing all of `rects` (each clamped into the unit square). An
/// empty slice yields the full monitor.
pub fn bounding_rect(rects: &[NormRect]) -> NormRect {
    let mut bounds: Option<(f32, f32, f32, f32)> = None;
    for r in rects.iter().map(|r| r.clamped()) {
        let (x1, y1) = (r.x + r.width, r.y + r.height);
        bounds = Some(match bounds {
            None => (r.x, r.y, x1, y1),
            Some((bx0, by0, bx1, by1)) => (bx0.min(r.x), by0.min(r.y), bx1.max(x1), by1.max(y1)),
        });
    }
    bounds.map_or_else(NormRect::full, |(x0, y0, x1, y1)| {
        NormRect::new(x0, y0, x1 - x0, y1 - y0).clamped()
    })
}

/// Reduces each zone to one colour and stores the colours in `out`, in zone order. `out` is
/// cleared first and reused: no allocation happens once its capacity suffices.
///
/// `frame` shows the part of the monitor `frame_rect` (a fraction of the monitor); `zones`
/// are fractions of the monitor and are mapped into the frame. The part of a zone outside the
/// frame is cut off, and a zone smaller than a pixel still covers one pixel. An invalid frame
/// reduces every zone to black.
pub fn reduce_zones(
    frame: &FrameView,
    frame_rect: NormRect,
    mode: ReduceMode,
    zones: &[NormRect],
    out: &mut Vec<LinearRgb>,
) {
    out.clear();
    let valid = frame.is_valid();
    let origin = frame_rect.clamped();
    out.extend(zones.iter().map(|zone| {
        if !valid {
            return LinearRgb::BLACK;
        }
        let zone = zone.clamped();
        let local = NormRect::new(
            (zone.x - origin.x) / origin.width,
            (zone.y - origin.y) / origin.height,
            zone.width / origin.width,
            zone.height / origin.height,
        );
        let px = local.to_pixels(frame.width, frame.height);
        let start = px.y as usize * frame.stride + px.x as usize * FrameView::BYTES_PER_PIXEL;
        let Some(data) = frame.data.get(start..) else {
            return LinearRgb::BLACK;
        };
        reduce(
            &FrameView::new(data, px.width, px.height, frame.stride),
            mode,
        )
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Rgb;

    #[allow(clippy::unnecessary_wraps)]
    fn pos(x: f32, z: f32) -> Option<LightPosition> {
        Some(LightPosition { x, y: 0.0, z })
    }

    fn assert_inside(zone: NormRect) {
        assert_eq!(zone, zone.clamped(), "{zone:?}");
    }

    /// A BGRA frame whose pixel colour is `f(x, y)`.
    fn frame_data(w: u32, h: u32, pad: usize, f: impl Fn(u32, u32) -> Rgb) -> (Vec<u8>, usize) {
        let stride = w as usize * 4 + pad;
        let mut data = vec![0xAB_u8; stride * h as usize];
        for y in 0..h {
            for x in 0..w {
                let c = f(x, y);
                let o = y as usize * stride + x as usize * 4;
                data[o..o + 4].copy_from_slice(&[c.b, c.g, c.r, 255]);
            }
        }
        (data, stride)
    }

    fn red_left_blue_right(x: u32, w: u32) -> Rgb {
        if x < w / 2 {
            Rgb::new(255, 0, 0)
        } else {
            Rgb::new(0, 0, 255)
        }
    }

    #[test]
    fn no_lights_no_zones() {
        assert_eq!(default_zones(&[]), Vec::<NormRect>::new());
    }

    #[test]
    fn all_none_gives_even_full_height_columns() {
        let zones = default_zones(&[None, None, None, None]);
        assert_eq!(zones.len(), 4);
        for (i, z) in zones.iter().enumerate() {
            assert_eq!(*z, NormRect::new(i as f32 * 0.25, 0.0, 0.25, 1.0));
        }
        assert_eq!(default_zones(&[None]), vec![NormRect::full()]);
    }

    #[test]
    fn two_lights_left_and_right() {
        let zones = default_zones(&[pos(-1.0, 0.0), pos(1.0, 0.0)]);
        assert_eq!(zones.len(), 2);
        assert_eq!(zones[0].x, 0.0);
        assert!((zones[1].x + zones[1].width - 1.0).abs() < 1e-6);
        assert!(zones[0].x + zones[0].width <= zones[1].x);
        for z in &zones {
            assert!((z.width - ZONE_WIDTH).abs() < 1e-6);
            assert!((z.height - ZONE_HEIGHT).abs() < 1e-6);
            assert!((z.y - (1.0 - ZONE_HEIGHT) / 2.0).abs() < 1e-6);
            assert_inside(*z);
        }
    }

    #[test]
    fn height_follows_z() {
        let zones = default_zones(&[pos(0.0, 1.0), pos(0.0, 0.0), pos(0.0, -1.0)]);
        assert_eq!(zones[0].y, 0.0);
        assert!((zones[2].y + zones[2].height - 1.0).abs() < 1e-6);
        assert!(zones[0].y < zones[1].y && zones[1].y < zones[2].y);
        assert!((zones[1].x - (0.5 - ZONE_WIDTH / 2.0)).abs() < 1e-6);
    }

    #[test]
    fn depth_is_ignored() {
        let at = |y| Some(LightPosition { x: 0.3, y, z: 0.2 });
        assert_eq!(default_zones(&[at(-1.0)]), default_zones(&[at(1.0)]));
    }

    #[test]
    fn three_lights_at_the_same_x_share_a_column() {
        let zones = default_zones(&[pos(0.5, 1.0), pos(0.5, 0.0), pos(0.5, -1.0)]);
        assert_eq!(zones.len(), 3);
        assert!(
            zones
                .iter()
                .all(|z| z.x == zones[0].x && z.width == zones[0].width)
        );
        assert!(zones[0].y < zones[1].y && zones[1].y < zones[2].y);
        zones.into_iter().for_each(assert_inside);
    }

    #[test]
    fn out_of_range_and_non_finite_positions_stay_inside() {
        let zones = default_zones(&[
            pos(-5.0, 9.0),
            pos(5.0, -9.0),
            pos(f32::NAN, 0.0),
            pos(0.0, f32::INFINITY),
        ]);
        assert_eq!(zones.len(), 4);
        zones.into_iter().for_each(assert_inside);
    }

    #[test]
    fn missing_position_among_known_ones_gets_a_centred_box() {
        let zones = default_zones(&[pos(-1.0, 0.0), None, pos(1.0, 0.0), None]);
        assert_eq!(zones[1], NormRect::new(0.4, 0.0, EXTRA_ZONE_WIDTH, 1.0));
        assert_eq!(zones[3], zones[1]);
        assert_inside(zones[1]);
    }

    #[test]
    fn bounding_rect_covers_all_rects() {
        assert_eq!(bounding_rect(&[]), NormRect::full());
        let a = NormRect::new(0.1, 0.2, 0.2, 0.2);
        let single = bounding_rect(&[a]);
        assert!((single.x - a.x).abs() < 1e-6 && (single.width - a.width).abs() < 1e-6);
        assert!((single.y - a.y).abs() < 1e-6 && (single.height - a.height).abs() < 1e-6);
        let b = NormRect::new(0.6, 0.5, 0.3, 0.4);
        let bounds = bounding_rect(&[a, b]);
        assert!((bounds.x - 0.1).abs() < 1e-6 && (bounds.y - 0.2).abs() < 1e-6);
        assert!((bounds.width - 0.8).abs() < 1e-6 && (bounds.height - 0.7).abs() < 1e-6);
        assert_inside(bounds);
        let wild = bounding_rect(&[NormRect::new(-3.0, 9.0, f32::NAN, 0.0), a]);
        assert_inside(wild);
    }

    #[test]
    fn zones_are_mapped_into_the_frame_rect() {
        // The frame covers the right half of the monitor: red | blue inside it.
        let (data, stride) = frame_data(40, 20, 4, |x, _| red_left_blue_right(x, 40));
        let frame = FrameView::new(&data, 40, 20, stride);
        let frame_rect = NormRect::new(0.5, 0.0, 0.5, 1.0);
        let zones = [
            NormRect::new(0.5, 0.0, 0.25, 1.0),
            NormRect::new(0.75, 0.0, 0.25, 1.0),
            frame_rect,
        ];
        let mut out = Vec::new();
        reduce_zones(&frame, frame_rect, ReduceMode::Average, &zones, &mut out);
        assert!(out[0].r > 0.99 && out[0].b < 0.01);
        assert!(out[1].b > 0.99 && out[1].r < 0.01);
        assert!((out[2].r - 0.5).abs() < 1e-3);
        // A zone left of the frame is cut off to its left edge pixel column.
        reduce_zones(
            &frame,
            frame_rect,
            ReduceMode::Average,
            &[NormRect::new(0.0, 0.0, 0.1, 1.0)],
            &mut out,
        );
        assert!(out[0].r > 0.99);
    }

    #[test]
    fn reduces_each_zone_separately() {
        let (data, stride) = frame_data(40, 20, 8, |x, _| red_left_blue_right(x, 40));
        let frame = FrameView::new(&data, 40, 20, stride);
        let zones = [
            NormRect::new(0.0, 0.0, 0.5, 1.0),
            NormRect::new(0.5, 0.0, 0.5, 1.0),
            NormRect::full(),
        ];
        for mode in [ReduceMode::Average, ReduceMode::Dominant] {
            let mut out = Vec::new();
            reduce_zones(&frame, NormRect::full(), mode, &zones, &mut out);
            assert_eq!(out.len(), 3);
            assert!(out[0].r > 0.99 && out[0].b < 0.01, "{mode:?} {:?}", out[0]);
            assert!(out[1].b > 0.99 && out[1].r < 0.01, "{mode:?} {:?}", out[1]);
        }
        let mut out = Vec::new();
        reduce_zones(
            &frame,
            NormRect::full(),
            ReduceMode::Average,
            &zones,
            &mut out,
        );
        assert!((out[2].r - 0.5).abs() < 1e-3 && (out[2].b - 0.5).abs() < 1e-3);
    }

    #[test]
    fn zone_matches_reducing_the_cropped_pixels() {
        let (data, stride) =
            frame_data(30, 30, 0, |x, y| Rgb::new((x * 8) as u8, (y * 8) as u8, 40));
        let frame = FrameView::new(&data, 30, 30, stride);
        let zone = NormRect::new(0.2, 0.3, 0.4, 0.5);
        let px = zone.to_pixels(30, 30);
        let (cropped, cstride) = frame_data(px.width, px.height, 0, |x, y| {
            Rgb::new(((x + px.x) * 8) as u8, ((y + px.y) * 8) as u8, 40)
        });
        let expected = reduce(
            &FrameView::new(&cropped, px.width, px.height, cstride),
            ReduceMode::Average,
        );
        let mut out = Vec::new();
        reduce_zones(
            &frame,
            NormRect::full(),
            ReduceMode::Average,
            &[zone],
            &mut out,
        );
        assert_eq!(out, vec![expected]);
    }

    #[test]
    fn tiny_zones_still_yield_a_colour() {
        let (data, stride) = frame_data(40, 20, 0, |x, _| red_left_blue_right(x, 40));
        let frame = FrameView::new(&data, 40, 20, stride);
        let zones = [
            NormRect::new(0.0, 0.0, 0.0, 0.0),
            NormRect::new(1.0, 1.0, 0.0001, 0.0001),
            NormRect::new(0.99, 0.0, 0.0, 0.0),
        ];
        let mut out = Vec::new();
        reduce_zones(
            &frame,
            NormRect::full(),
            ReduceMode::Average,
            &zones,
            &mut out,
        );
        assert!(out[0].r > 0.99);
        assert!(out[1].b > 0.99);
        assert!(out[2].b > 0.99);
    }

    #[test]
    fn zones_at_the_edges_stay_in_bounds() {
        let (data, stride) = frame_data(7, 5, 3, |x, y| Rgb::new(x as u8 * 30, y as u8 * 50, 9));
        let frame = FrameView::new(&data, 7, 5, stride);
        let zones = [
            NormRect::new(0.0, 0.0, 1.0, 1.0),
            NormRect::new(0.9, 0.9, 0.1, 0.1),
            NormRect::new(0.0, 0.9, 0.1, 0.1),
            NormRect::new(5.0, 5.0, 5.0, 5.0),
            NormRect::new(-3.0, -3.0, 0.2, 0.2),
        ];
        for mode in [ReduceMode::Average, ReduceMode::Dominant] {
            let mut out = Vec::new();
            reduce_zones(&frame, NormRect::full(), mode, &zones, &mut out);
            assert_eq!(out.len(), zones.len());
            assert!(out.iter().all(|c| c.r.is_finite() && c.b > 0.0));
        }
    }

    #[test]
    fn invalid_frames_reduce_to_black() {
        let mut out = vec![LinearRgb::new(1.0, 1.0, 1.0)];
        let zones = [NormRect::full(), NormRect::full()];
        reduce_zones(
            &FrameView::new(&[], 0, 0, 0),
            NormRect::full(),
            ReduceMode::Average,
            &zones,
            &mut out,
        );
        assert_eq!(out, vec![LinearRgb::BLACK; 2]);
        let short = [0_u8; 16];
        reduce_zones(
            &FrameView::new(&short, 4, 4, 16),
            NormRect::full(),
            ReduceMode::Dominant,
            &zones,
            &mut out,
        );
        assert_eq!(out, vec![LinearRgb::BLACK; 2]);
    }

    #[test]
    fn out_is_cleared_and_reused() {
        let (data, stride) = frame_data(8, 8, 0, |_, _| Rgb::new(10, 20, 30));
        let frame = FrameView::new(&data, 8, 8, stride);
        let zones = [NormRect::full(); 3];
        let mut out = Vec::with_capacity(8);
        out.push(LinearRgb::new(1.0, 1.0, 1.0));
        let ptr = out.as_ptr();
        for mode in [ReduceMode::Average, ReduceMode::Dominant] {
            reduce_zones(&frame, NormRect::full(), mode, &zones, &mut out);
            assert_eq!(out.len(), 3);
            assert_eq!(out.as_ptr(), ptr);
            assert_eq!(out.capacity(), 8);
        }
        reduce_zones(&frame, NormRect::full(), ReduceMode::Average, &[], &mut out);
        assert_eq!(out, Vec::<LinearRgb>::new());
    }
}
