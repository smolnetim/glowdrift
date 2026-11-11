//! Colour types and sRGB <-> linear-light conversions.

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

/// An 8-bit sRGB-encoded colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const BLACK: Rgb = Rgb::new(0, 0, 0);

    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// Formats as lowercase `#rrggbb`.
    pub fn to_hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }

    /// Parses `#rrggbb` or `rrggbb` (case-insensitive). Returns `None` on any other input.
    pub fn from_hex(s: &str) -> Option<Rgb> {
        let s = s.strip_prefix('#').unwrap_or(s);
        if s.len() != 6 || !s.is_ascii() {
            return None;
        }
        let channel = |i: usize| u8::from_str_radix(&s[i..i + 2], 16).ok();
        Some(Rgb::new(channel(0)?, channel(2)?, channel(4)?))
    }

    /// Converts to linear light.
    pub fn to_linear(self) -> LinearRgb {
        LinearRgb {
            r: srgb_to_linear(self.r),
            g: srgb_to_linear(self.g),
            b: srgb_to_linear(self.b),
        }
    }
}

/// A colour in linear light, channels nominally in `0.0..=1.0`.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct LinearRgb {
    pub r: f32,
    pub g: f32,
    pub b: f32,
}

impl LinearRgb {
    pub const BLACK: LinearRgb = LinearRgb::new(0.0, 0.0, 0.0);

    pub const fn new(r: f32, g: f32, b: f32) -> Self {
        Self { r, g, b }
    }

    /// Converts to 8-bit sRGB, clamping each channel into range.
    pub fn to_srgb(self) -> Rgb {
        Rgb {
            r: linear_to_srgb(self.r),
            g: linear_to_srgb(self.g),
            b: linear_to_srgb(self.b),
        }
    }
}

/// Exact sRGB electro-optical transfer function for a channel in `0.0..=1.0`.
fn decode_exact(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Lookup table mapping every 8-bit sRGB value to linear light. Built once, then shared.
pub fn srgb_lut() -> &'static [f32; 256] {
    static LUT: OnceLock<[f32; 256]> = OnceLock::new();
    LUT.get_or_init(|| {
        let mut table = [0.0_f32; 256];
        for (i, entry) in table.iter_mut().enumerate() {
            *entry = decode_exact(i as f32 / 255.0);
        }
        table
    })
}

/// Decodes one 8-bit sRGB channel to linear light (`0.0..=1.0`).
pub fn srgb_to_linear(v: u8) -> f32 {
    srgb_lut()[usize::from(v)]
}

/// Encodes one linear-light channel to 8-bit sRGB. Out-of-range and NaN inputs are clamped.
pub fn linear_to_srgb(v: f32) -> u8 {
    let c = if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) };
    let s = if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0).round().clamp(0.0, 255.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trip() {
        let c = Rgb::new(0x12, 0xab, 0xff);
        assert_eq!(c.to_hex(), "#12abff");
        assert_eq!(Rgb::from_hex("#12ABFF"), Some(c));
        assert_eq!(Rgb::from_hex("12abff"), Some(c));
        assert_eq!(Rgb::from_hex("#12ab"), None);
        assert_eq!(Rgb::from_hex("#gggggg"), None);
    }

    #[test]
    fn srgb_linear_round_trip() {
        for v in 0..=255u8 {
            assert_eq!(linear_to_srgb(srgb_to_linear(v)), v);
        }
    }

    #[test]
    fn linear_to_srgb_clamps() {
        assert_eq!(linear_to_srgb(-1.0), 0);
        assert_eq!(linear_to_srgb(2.0), 255);
        assert_eq!(linear_to_srgb(f32::NAN), 0);
    }

    #[test]
    fn lut_matches_exact_formula() {
        let lut = srgb_lut();
        for v in 0..=255u8 {
            let exact = decode_exact(f32::from(v) / 255.0);
            assert!((lut[usize::from(v)] - exact).abs() < f32::EPSILON);
            assert_eq!(srgb_to_linear(v), lut[usize::from(v)]);
        }
    }

    #[test]
    fn transfer_function_known_values() {
        assert_eq!(srgb_to_linear(0), 0.0);
        assert!((srgb_to_linear(255) - 1.0).abs() < 1e-6);
        // Mid grey 128 decodes to about 0.2159.
        assert!((srgb_to_linear(128) - 0.215_86).abs() < 1e-4);
        // Linear 0.5 encodes to sRGB 188.
        assert_eq!(linear_to_srgb(0.5), 188);
        // Linear segment below the knee.
        assert!((srgb_to_linear(5) - (5.0 / 255.0) / 12.92).abs() < 1e-7);
    }

    #[test]
    fn transfer_function_is_monotonic() {
        let lut = srgb_lut();
        assert!(lut.windows(2).all(|w| w[0] < w[1]));
        let mut prev = 0;
        for i in 0..=1000 {
            let v = linear_to_srgb(i as f32 / 1000.0);
            assert!(v >= prev);
            prev = v;
        }
    }

    #[test]
    fn linear_to_srgb_handles_infinities() {
        assert_eq!(linear_to_srgb(f32::INFINITY), 255);
        assert_eq!(linear_to_srgb(f32::NEG_INFINITY), 0);
    }

    #[test]
    fn rgb_linear_round_trip_via_types() {
        let c = Rgb::new(10, 200, 77);
        assert_eq!(c.to_linear().to_srgb(), c);
    }

    #[test]
    fn hex_rejects_non_ascii_and_bad_length() {
        assert_eq!(Rgb::from_hex("#12345\u{e9}"), None);
        assert_eq!(Rgb::from_hex(""), None);
        assert_eq!(Rgb::from_hex("#1234567"), None);
        assert_eq!(Rgb::from_hex("##12345"), None);
    }
}
