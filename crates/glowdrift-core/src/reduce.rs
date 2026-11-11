//! Reduction of a frame to a single colour, and output colour adjustment.
//!
//! # Average
//!
//! The mean of every pixel in linear light (sRGB bytes are decoded through a lookup
//! table, alpha and stride padding are ignored).
//!
//! # Dominant
//!
//! A deterministic "what colour would a person call this image" pick:
//!
//! 1. Pixels are binned into a 16x16x16 histogram on the top four bits of each sRGB
//!    channel. Each bin keeps its pixel count and the linear-light sum of its pixels.
//! 2. Each bin gets a perceptual weight from its centre colour, `0.05 + 0.25 v + 0.7 s v`,
//!    with `v` the HSV value and `s` the HSV saturation. Vivid, bright bins weigh about 20x
//!    more than black, so a large black or grey letterbox does not outvote the picture,
//!    while a scene that is dark everywhere still resolves to dark because the weights of
//!    its bins stay comparable.
//! 3. The score of a bin is the weighted count summed over its 3x3x3 neighbourhood (a
//!    separable box filter), so a colour spread over adjacent bins by noise or gradients
//!    still wins as one cluster. Only occupied bins can win; ties go to the lowest index.
//! 4. The result is the weighted linear-light mean of the winning bin and its neighbours,
//!    not the bin centre.

use std::sync::OnceLock;

use crate::color::{LinearRgb, srgb_lut};
use crate::config::{Config, ReduceMode};
use crate::frame::FrameView;

/// Reduces `frame` to one colour in linear light.
///
/// `Average` is the mean in linear light; `Dominant` picks the most representative hue
/// cluster. Invalid or empty frames reduce to black.
pub fn reduce(frame: &FrameView, mode: ReduceMode) -> LinearRgb {
    if !frame.is_valid() {
        return LinearRgb::BLACK;
    }
    match mode {
        ReduceMode::Average => average(frame),
        ReduceMode::Dominant => dominant(frame),
    }
}

fn average(frame: &FrameView) -> LinearRgb {
    let lut = srgb_lut();
    let mut sum = [0.0_f64; 3];
    for y in 0..frame.height {
        let Some(row) = frame.row(y) else {
            return LinearRgb::BLACK;
        };
        let mut acc = [0.0_f32; 3];
        for px in row.as_chunks::<{ FrameView::BYTES_PER_PIXEL }>().0 {
            acc[0] += lut[usize::from(px[2])];
            acc[1] += lut[usize::from(px[1])];
            acc[2] += lut[usize::from(px[0])];
        }
        for (s, a) in sum.iter_mut().zip(acc) {
            *s += f64::from(a);
        }
    }
    let n = f64::from(frame.width) * f64::from(frame.height);
    LinearRgb::new(
        (sum[0] / n) as f32,
        (sum[1] / n) as f32,
        (sum[2] / n) as f32,
    )
}

const BITS: usize = 4;
const LEVELS: usize = 1 << BITS;
const BINS: usize = LEVELS * LEVELS * LEVELS;
const SHIFT: usize = 8 - BITS;
/// Bit offset of the red, green and blue coordinates within a bin index.
const AXIS_SHIFT: [usize; 3] = [2 * BITS, BITS, 0];

/// Per-bin perceptual weights, indexed like the histogram.
fn bin_weights() -> &'static [f32; BINS] {
    static WEIGHTS: OnceLock<[f32; BINS]> = OnceLock::new();
    WEIGHTS.get_or_init(|| {
        let mut table = [0.0_f32; BINS];
        let centre = |i: usize| ((i & (LEVELS - 1)) as f32 + 0.5) / LEVELS as f32;
        for (idx, weight) in table.iter_mut().enumerate() {
            let r = centre(idx >> AXIS_SHIFT[0]);
            let g = centre(idx >> AXIS_SHIFT[1]);
            let b = centre(idx >> AXIS_SHIFT[2]);
            let max = r.max(g).max(b);
            let min = r.min(g).min(b);
            let sat = if max > 0.0 { (max - min) / max } else { 0.0 };
            *weight = 0.05 + 0.25 * max + 0.7 * sat * max;
        }
        table
    })
}

/// Index of the neighbour of `idx` one step along `axis` (`-1` or `+1`), if in range.
fn neighbour(idx: usize, axis: usize, step: isize) -> Option<usize> {
    let shift = AXIS_SHIFT[axis];
    let coord = (idx >> shift) & (LEVELS - 1);
    let next = coord.checked_add_signed(step)?;
    if next >= LEVELS {
        return None;
    }
    Some((idx & !((LEVELS - 1) << shift)) | (next << shift))
}

/// `idx` and its in-range neighbours along `axis`.
fn axis_triple(idx: usize, axis: usize) -> [Option<usize>; 3] {
    [Some(idx), neighbour(idx, axis, -1), neighbour(idx, axis, 1)]
}

fn dominant(frame: &FrameView) -> LinearRgb {
    let lut = srgb_lut();
    let weights = bin_weights();
    let mut counts = [0_u32; BINS];
    let mut sums = vec![[0.0_f32; 3]; BINS];

    for y in 0..frame.height {
        let Some(row) = frame.row(y) else {
            return LinearRgb::BLACK;
        };
        for px in row.as_chunks::<{ FrameView::BYTES_PER_PIXEL }>().0 {
            let (b, g, r) = (px[0], px[1], px[2]);
            let idx = (usize::from(r >> SHIFT) << AXIS_SHIFT[0])
                | (usize::from(g >> SHIFT) << AXIS_SHIFT[1])
                | usize::from(b >> SHIFT);
            counts[idx] += 1;
            let sum = &mut sums[idx];
            sum[0] += lut[usize::from(r)];
            sum[1] += lut[usize::from(g)];
            sum[2] += lut[usize::from(b)];
        }
    }

    let mut scores = [0.0_f32; BINS];
    for (score, (&count, &weight)) in scores.iter_mut().zip(counts.iter().zip(weights)) {
        *score = count as f32 * weight;
    }
    for axis in 0..3 {
        let src = scores;
        for (idx, score) in scores.iter_mut().enumerate() {
            let mut total = src[idx];
            for step in [-1, 1] {
                if let Some(n) = neighbour(idx, axis, step) {
                    total += src[n];
                }
            }
            *score = total;
        }
    }

    let mut best: Option<(usize, f32)> = None;
    for (idx, (&count, &score)) in counts.iter().zip(scores.iter()).enumerate() {
        if count > 0 && best.is_none_or(|(_, top)| score > top) {
            best = Some((idx, score));
        }
    }
    let Some((winner, _)) = best else {
        return LinearRgb::BLACK;
    };

    let mut num = [0.0_f64; 3];
    let mut den = 0.0_f64;
    let mut accumulate = |idx: usize| {
        let w = f64::from(weights[idx]);
        for (n, s) in num.iter_mut().zip(sums[idx]) {
            *n += w * f64::from(s);
        }
        den += w * f64::from(counts[idx]);
    };
    for r in axis_triple(winner, 0).into_iter().flatten() {
        for g in axis_triple(r, 1).into_iter().flatten() {
            for b in axis_triple(g, 2).into_iter().flatten() {
                accumulate(b);
            }
        }
    }
    if den <= 0.0 {
        return LinearRgb::BLACK;
    }
    LinearRgb::new(
        (num[0] / den) as f32,
        (num[1] / den) as f32,
        (num[2] / den) as f32,
    )
}

/// Output gain and saturation applied after smoothing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColorAdjust {
    /// Linear gain, `0.0..=1.0`.
    pub brightness: f32,
    /// Saturation multiplier, `0.0..=2.0`; `1.0` leaves the colour unchanged.
    pub saturation: f32,
}

impl ColorAdjust {
    /// Rec. 709 luma weights for linear-light RGB.
    const LUMA: [f32; 3] = [0.2126, 0.7152, 0.0722];

    pub const fn new(brightness: f32, saturation: f32) -> Self {
        Self {
            brightness,
            saturation,
        }
    }

    /// Applies saturation then brightness to a linear colour. The saturation scales each
    /// channel's distance from the Rec. 709 luma; the result is clamped to `0.0..=1.0`.
    /// NaN parameters fall back to `1.0`, and out-of-range parameters are clamped to
    /// their valid ranges.
    pub fn apply(&self, color: LinearRgb) -> LinearRgb {
        fn param(v: f32, max: f32) -> f32 {
            if v.is_nan() { 1.0 } else { v.clamp(0.0, max) }
        }
        let brightness = param(self.brightness, 1.0);
        let saturation = param(self.saturation, Config::SATURATION_MAX);
        let channel = |v: f32| if v.is_nan() { 0.0 } else { v };
        let (r, g, b) = (channel(color.r), channel(color.g), channel(color.b));
        let luma = Self::LUMA[0] * r + Self::LUMA[1] * g + Self::LUMA[2] * b;
        let adjust = |c: f32| (luma + (c - luma) * saturation).clamp(0.0, 1.0) * brightness;
        LinearRgb::new(adjust(r), adjust(g), adjust(b))
    }
}

impl Default for ColorAdjust {
    fn default() -> Self {
        Self::new(1.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Rgb;

    /// Builds a BGRA buffer from per-pixel colours with `pad` extra bytes per row.
    fn buffer(w: u32, h: u32, pad: usize, mut f: impl FnMut(u32, u32) -> Rgb) -> (Vec<u8>, usize) {
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

    fn reduce_with(
        w: u32,
        h: u32,
        pad: usize,
        mode: ReduceMode,
        f: impl FnMut(u32, u32) -> Rgb,
    ) -> LinearRgb {
        let (data, stride) = buffer(w, h, pad, f);
        reduce(&FrameView::new(&data, w, h, stride), mode)
    }

    fn near(a: LinearRgb, b: LinearRgb, eps: f32) -> bool {
        (a.r - b.r).abs() < eps && (a.g - b.g).abs() < eps && (a.b - b.b).abs() < eps
    }

    /// Small deterministic pseudo-random generator.
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> u32 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (self.0 >> 33) as u32
        }
        fn byte(&mut self) -> u8 {
            (self.next() & 0xFF) as u8
        }
    }

    #[test]
    fn invalid_frames_are_black() {
        for mode in [ReduceMode::Average, ReduceMode::Dominant] {
            assert_eq!(
                reduce(&FrameView::new(&[], 0, 0, 0), mode),
                LinearRgb::BLACK
            );
            assert_eq!(
                reduce(&FrameView::new(&[0; 16], 4, 0, 16), mode),
                LinearRgb::BLACK
            );
            // Too little data for the declared size.
            assert_eq!(
                reduce(&FrameView::new(&[255; 16], 4, 4, 16), mode),
                LinearRgb::BLACK
            );
            // Stride smaller than a row.
            assert_eq!(
                reduce(&FrameView::new(&[255; 64], 4, 4, 8), mode),
                LinearRgb::BLACK
            );
        }
    }

    #[test]
    fn average_of_uniform_frame_is_that_colour() {
        let c = Rgb::new(30, 140, 220);
        let out = reduce_with(16, 9, 0, ReduceMode::Average, |_, _| c);
        assert!(near(out, c.to_linear(), 1e-5));
        assert_eq!(out.to_srgb(), c);
    }

    #[test]
    fn average_of_black_and_white_halves_is_linear_half() {
        let out = reduce_with(20, 10, 0, ReduceMode::Average, |x, _| {
            if x < 10 {
                Rgb::BLACK
            } else {
                Rgb::new(255, 255, 255)
            }
        });
        assert!(near(out, LinearRgb::new(0.5, 0.5, 0.5), 1e-5));
        assert_eq!(out.to_srgb(), Rgb::new(188, 188, 188));
    }

    #[test]
    fn average_is_per_channel() {
        let out = reduce_with(4, 4, 0, ReduceMode::Average, |x, _| {
            if x % 2 == 0 {
                Rgb::new(255, 0, 0)
            } else {
                Rgb::new(0, 0, 255)
            }
        });
        assert!(near(out, LinearRgb::new(0.5, 0.0, 0.5), 1e-5));
    }

    #[test]
    fn average_ignores_alpha() {
        let (mut data, stride) = buffer(4, 4, 0, |_, _| Rgb::new(255, 128, 0));
        for px in data.as_chunks_mut::<4>().0 {
            px[3] = 0;
        }
        let out = reduce(&FrameView::new(&data, 4, 4, stride), ReduceMode::Average);
        assert!(near(out, Rgb::new(255, 128, 0).to_linear(), 1e-5));
    }

    #[test]
    fn stride_padding_is_ignored() {
        let f = |x: u32, y: u32| Rgb::new((x * 30) as u8, (y * 40) as u8, 90);
        for mode in [ReduceMode::Average, ReduceMode::Dominant] {
            let tight = reduce_with(7, 5, 0, mode, f);
            let padded = reduce_with(7, 5, 36, mode, f);
            assert_eq!(tight, padded, "{mode:?}");
        }
    }

    #[test]
    fn last_row_needs_no_trailing_padding() {
        let stride = 4 * 4 + 8;
        let mut data = vec![0_u8; stride * 3 + 4 * 4];
        for px in data.as_chunks_mut::<4>().0 {
            px.copy_from_slice(&[0, 0, 255, 255]);
        }
        let frame = FrameView::new(&data, 4, 4, stride);
        assert!(frame.is_valid());
        assert!(reduce(&frame, ReduceMode::Average).r > 0.0);
    }

    #[test]
    fn dominant_of_uniform_frame_is_that_colour() {
        let c = Rgb::new(30, 140, 220);
        let out = reduce_with(16, 9, 0, ReduceMode::Dominant, |_, _| c);
        assert!(near(out, c.to_linear(), 1e-4));
    }

    #[test]
    fn dominant_two_colour_split_picks_the_larger_area() {
        let red = Rgb::new(230, 20, 20);
        let blue = Rgb::new(20, 20, 230);
        let out = reduce_with(100, 10, 0, ReduceMode::Dominant, |x, _| {
            if x < 60 { red } else { blue }
        });
        assert!(near(out, red.to_linear(), 1e-4), "{out:?}");
        let out = reduce_with(100, 10, 0, ReduceMode::Dominant, |x, _| {
            if x < 40 { red } else { blue }
        });
        assert!(near(out, blue.to_linear(), 1e-4), "{out:?}");
    }

    #[test]
    fn dominant_ignores_letterbox_bars() {
        let teal = Rgb::new(30, 170, 160);
        // 35% black bars top and bottom (70% total) around a smaller picture.
        let out = reduce_with(64, 100, 0, ReduceMode::Dominant, |_, y| {
            if (35..65).contains(&y) {
                teal
            } else {
                Rgb::BLACK
            }
        });
        assert!(near(out, teal.to_linear(), 0.02), "{out:?}");
        // The plain average, in contrast, is dragged dark.
        let avg = reduce_with(64, 100, 0, ReduceMode::Average, |_, y| {
            if (35..65).contains(&y) {
                teal
            } else {
                Rgb::BLACK
            }
        });
        assert!(avg.g < 0.2 * teal.to_linear().g * 2.0);
    }

    #[test]
    fn dominant_prefers_background_over_tiny_vivid_detail() {
        let white = Rgb::new(245, 245, 245);
        let out = reduce_with(100, 100, 0, ReduceMode::Dominant, |x, y| {
            if x < 8 && y < 8 {
                Rgb::new(255, 0, 0)
            } else {
                white
            }
        });
        assert!(near(out, white.to_linear(), 0.02), "{out:?}");
    }

    #[test]
    fn dominant_of_dark_scene_is_dark() {
        let dark = Rgb::new(6, 6, 8);
        let out = reduce_with(100, 100, 0, ReduceMode::Dominant, |x, y| {
            if x < 5 && y < 6 {
                Rgb::new(255, 20, 20)
            } else {
                dark
            }
        });
        let srgb = out.to_srgb();
        assert!(srgb.r < 40 && srgb.g < 40 && srgb.b < 40, "{srgb:?}");
    }

    #[test]
    fn dominant_of_pure_black_is_black() {
        let out = reduce_with(32, 18, 0, ReduceMode::Dominant, |_, _| Rgb::BLACK);
        assert_eq!(out, LinearRgb::BLACK);
    }

    #[test]
    fn dominant_survives_noise_around_a_colour() {
        let mut rng = Lcg(7);
        let base = [200_i32, 60, 60];
        let out = reduce_with(96, 54, 0, ReduceMode::Dominant, |_, _| {
            let mut jitter = |v: i32| (v + i32::from(rng.byte() % 17) - 8).clamp(0, 255) as u8;
            Rgb::new(jitter(base[0]), jitter(base[1]), jitter(base[2]))
        });
        let srgb = out.to_srgb();
        assert!(srgb.r.abs_diff(200) <= 10, "{srgb:?}");
        assert!(srgb.g.abs_diff(60) <= 10, "{srgb:?}");
        assert!(srgb.b.abs_diff(60) <= 10, "{srgb:?}");
    }

    #[test]
    fn dominant_is_deterministic_and_in_range_for_random_frames() {
        let mut rng = Lcg(42);
        for case in 0..20 {
            let (w, h) = (1 + rng.next() % 40, 1 + rng.next() % 40);
            let pad = (rng.next() % 3) as usize * 4;
            let (data, stride) = buffer(w, h, pad, |_, _| {
                Rgb::new(rng.byte(), rng.byte(), rng.byte())
            });
            let frame = FrameView::new(&data, w, h, stride);
            for mode in [ReduceMode::Average, ReduceMode::Dominant] {
                let a = reduce(&frame, mode);
                let b = reduce(&frame, mode);
                assert_eq!(a, b, "case {case}");
                for v in [a.r, a.g, a.b] {
                    assert!((0.0..=1.0).contains(&v), "case {case} {mode:?} {a:?}");
                }
            }
        }
    }

    #[test]
    fn dominant_neighbour_bins_merge_into_one_cluster() {
        // Two adjacent bins of the same hue: neither alone beats the other colour,
        // together they do.
        let a = Rgb::new(0x38, 0x08, 0x08);
        let b = Rgb::new(0x48, 0x08, 0x08);
        let other = Rgb::new(0x08, 0x08, 0x58);
        let out = reduce_with(100, 1, 0, ReduceMode::Dominant, |x, _| match x {
            0..=34 => a,
            35..=69 => b,
            _ => other,
        });
        let (la, lb) = (a.to_linear().r, b.to_linear().r);
        assert!(out.r > la && out.r < lb, "{out:?}");
        assert!(out.b < 0.01, "{out:?}");
    }

    #[test]
    fn adjust_identity() {
        let c = LinearRgb::new(0.2, 0.5, 0.8);
        assert!(near(ColorAdjust::default().apply(c), c, 1e-6));
    }

    #[test]
    fn adjust_brightness_scales_linearly() {
        let c = LinearRgb::new(0.2, 0.5, 0.8);
        let out = ColorAdjust::new(0.5, 1.0).apply(c);
        assert!(near(out, LinearRgb::new(0.1, 0.25, 0.4), 1e-6));
        assert_eq!(ColorAdjust::new(0.0, 1.0).apply(c), LinearRgb::BLACK);
    }

    #[test]
    fn adjust_zero_saturation_gives_luma_grey() {
        let c = LinearRgb::new(1.0, 0.0, 0.0);
        let out = ColorAdjust::new(1.0, 0.0).apply(c);
        assert!(near(out, LinearRgb::new(0.2126, 0.2126, 0.2126), 1e-6));
    }

    #[test]
    fn adjust_saturation_leaves_greys_alone_and_clamps() {
        let grey = LinearRgb::new(0.4, 0.4, 0.4);
        assert!(near(ColorAdjust::new(1.0, 2.0).apply(grey), grey, 1e-6));
        let out = ColorAdjust::new(1.0, 2.0).apply(LinearRgb::new(0.9, 0.1, 0.1));
        for v in [out.r, out.g, out.b] {
            assert!((0.0..=1.0).contains(&v));
        }
        assert!(out.r > 0.9 && out.g < 0.1);
    }

    #[test]
    fn adjust_preserves_luma_when_unclamped() {
        let c = LinearRgb::new(0.5, 0.4, 0.3);
        let luma = |c: LinearRgb| 0.2126 * c.r + 0.7152 * c.g + 0.0722 * c.b;
        let out = ColorAdjust::new(1.0, 1.5).apply(c);
        assert!((luma(out) - luma(c)).abs() < 1e-6);
    }

    #[test]
    fn adjust_handles_bad_inputs() {
        let out = ColorAdjust::new(f32::NAN, f32::NAN).apply(LinearRgb::new(0.3, 0.6, 0.9));
        assert!(near(out, LinearRgb::new(0.3, 0.6, 0.9), 1e-6));
        let out = ColorAdjust::new(9.0, 9.0).apply(LinearRgb::new(2.0, -1.0, f32::NAN));
        for v in [out.r, out.g, out.b] {
            assert!((0.0..=1.0).contains(&v));
        }
    }
}
