//! Colour mapping from 8-bit sRGB to what a Hue light takes: a CIE 1931 `xy` chromaticity inside
//! the light's gamut, a brightness percentage, or "off".
//!
//! Pipeline: sRGB decode, linear sRGB to CIE XYZ (D65), projection to `xy`, clamp into the
//! gamut triangle (closest point on the triangle when outside).
//!
//! Brightness is the largest 8-bit channel as a percentage (the HSV value). Hue brightness
//! scales the light's own output at the chosen chromaticity, so a saturated primary at 100 % is
//! already as bright as that light can render it; mapping from luminance would darken blues and
//! reds a second time. The value follows the sRGB encoding, which tracks the roughly
//! perceptual response of Hue's brightness scale better than linear light does. Colours whose
//! largest channel is below [`OFF_BELOW`] turn the light off; every other colour is at least
//! [`MIN_BRIGHTNESS`] percent, because Hue treats 0 as "dimmest" rather than "off".

/// Largest 8-bit channel below which the light is switched off.
pub(crate) const OFF_BELOW: u8 = 3;
/// Lowest brightness percentage sent for a lit colour.
pub(crate) const MIN_BRIGHTNESS: f64 = 1.0;

/// A CIE 1931 chromaticity.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Xy {
    pub x: f64,
    pub y: f64,
}

impl Xy {
    pub const fn new(x: f64, y: f64) -> Xy {
        Xy { x, y }
    }

    fn sub(self, other: Xy) -> Xy {
        Xy::new(self.x - other.x, self.y - other.y)
    }

    fn dot(self, other: Xy) -> f64 {
        self.x * other.x + self.y * other.y
    }

    fn cross(self, other: Xy) -> f64 {
        self.x * other.y - self.y * other.x
    }

    fn distance_sq(self, other: Xy) -> f64 {
        let d = self.sub(other);
        d.dot(d)
    }
}

/// The triangle of chromaticities a light can render.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Gamut {
    pub red: Xy,
    pub green: Xy,
    pub blue: Xy,
}

impl Gamut {
    /// Older Hue strips and Living Colors.
    pub const A: Gamut = Gamut {
        red: Xy::new(0.704, 0.296),
        green: Xy::new(0.2151, 0.7106),
        blue: Xy::new(0.138, 0.08),
    };
    /// First-generation Hue bulbs.
    pub const B: Gamut = Gamut {
        red: Xy::new(0.675, 0.322),
        green: Xy::new(0.409, 0.518),
        blue: Xy::new(0.167, 0.04),
    };
    /// Current Hue bulbs and strips.
    pub const C: Gamut = Gamut {
        red: Xy::new(0.6915, 0.3083),
        green: Xy::new(0.17, 0.7),
        blue: Xy::new(0.1532, 0.0475),
    };

    /// The gamut for a Hue `gamut_type` letter.
    pub fn from_type(kind: &str) -> Option<Gamut> {
        match kind {
            "A" => Some(Gamut::A),
            "B" => Some(Gamut::B),
            "C" => Some(Gamut::C),
            _ => None,
        }
    }

    /// Whether the corners form a usable triangle (finite, non-zero area).
    pub fn is_valid(&self) -> bool {
        let corners = [self.red, self.green, self.blue];
        corners
            .iter()
            .all(|c| c.x.is_finite() && c.y.is_finite() && (0.0..=1.0).contains(&c.x))
            && self
                .green
                .sub(self.red)
                .cross(self.blue.sub(self.red))
                .abs()
                > 1e-6
    }

    /// Whether `p` lies inside or on the triangle.
    pub fn contains(&self, p: Xy) -> bool {
        let side = |a: Xy, b: Xy| b.sub(a).cross(p.sub(a));
        let d1 = side(self.red, self.green);
        let d2 = side(self.green, self.blue);
        let d3 = side(self.blue, self.red);
        let has_neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
        let has_pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
        !(has_neg && has_pos)
    }

    /// `p` if it is inside the triangle, otherwise the closest point on its boundary.
    pub fn clamp(&self, p: Xy) -> Xy {
        if self.contains(p) {
            return p;
        }
        [
            (self.red, self.green),
            (self.green, self.blue),
            (self.blue, self.red),
        ]
        .into_iter()
        .map(|(a, b)| closest_on_segment(a, b, p))
        .min_by(|l, r| l.distance_sq(p).total_cmp(&r.distance_sq(p)))
        .unwrap_or(p)
    }
}

fn closest_on_segment(a: Xy, b: Xy, p: Xy) -> Xy {
    let ab = b.sub(a);
    let len_sq = ab.dot(ab);
    if len_sq == 0.0 {
        return a;
    }
    let t = (p.sub(a).dot(ab) / len_sq).clamp(0.0, 1.0);
    Xy::new(a.x + ab.x * t, a.y + ab.y * t)
}

/// What to send to one light for a colour.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mapped {
    /// Switch the light off.
    Off,
    /// Switch the light on at this chromaticity (already inside the gamut) and brightness
    /// percentage (`MIN_BRIGHTNESS..=100`).
    On { xy: Xy, brightness: f64 },
}

fn srgb_to_linear(channel: u8) -> f64 {
    let c = f64::from(channel) / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// The `xy` chromaticity of an sRGB colour, or `None` for pure black (no chromaticity).
pub fn rgb_to_xy(r: u8, g: u8, b: u8) -> Option<Xy> {
    let (r, g, b) = (srgb_to_linear(r), srgb_to_linear(g), srgb_to_linear(b));
    let x = 0.412_456_4 * r + 0.357_576_1 * g + 0.180_437_5 * b;
    let y = 0.212_672_9 * r + 0.715_152_2 * g + 0.072_175_0 * b;
    let z = 0.019_333_9 * r + 0.119_192_0 * g + 0.950_304_1 * b;
    let sum = x + y + z;
    (sum > 0.0).then(|| Xy::new(x / sum, y / sum))
}

/// Brightness percentage for a colour, before the off threshold and minimum are applied.
pub fn brightness_percent(r: u8, g: u8, b: u8) -> f64 {
    f64::from(r.max(g).max(b)) / 255.0 * 100.0
}

/// Maps an sRGB colour for a light with the given gamut.
pub fn map_color(r: u8, g: u8, b: u8, gamut: &Gamut) -> Mapped {
    if r.max(g).max(b) < OFF_BELOW {
        return Mapped::Off;
    }
    let Some(xy) = rgb_to_xy(r, g, b) else {
        return Mapped::Off;
    };
    Mapped::On {
        xy: gamut.clamp(xy),
        brightness: brightness_percent(r, g, b).max(MIN_BRIGHTNESS),
    }
}

/// Brightness-only mapping for lights without colour: off below the threshold, otherwise the
/// brightness percentage.
pub fn map_brightness(r: u8, g: u8, b: u8) -> Mapped {
    if r.max(g).max(b) < OFF_BELOW {
        return Mapped::Off;
    }
    Mapped::On {
        xy: Xy::new(0.0, 0.0),
        brightness: brightness_percent(r, g, b).max(MIN_BRIGHTNESS),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn white_maps_to_d65() {
        let xy = rgb_to_xy(255, 255, 255).unwrap();
        assert!(close(xy.x, 0.3127), "{xy:?}");
        assert!(close(xy.y, 0.3290), "{xy:?}");
    }

    #[test]
    fn srgb_primaries_match_the_standard() {
        let red = rgb_to_xy(255, 0, 0).unwrap();
        assert!(close(red.x, 0.64) && close(red.y, 0.33), "{red:?}");
        let green = rgb_to_xy(0, 255, 0).unwrap();
        assert!(close(green.x, 0.30) && close(green.y, 0.60), "{green:?}");
        let blue = rgb_to_xy(0, 0, 255).unwrap();
        assert!(close(blue.x, 0.15) && close(blue.y, 0.06), "{blue:?}");
    }

    #[test]
    fn chromaticity_ignores_intensity_for_greys() {
        let dim = rgb_to_xy(40, 40, 40).unwrap();
        let bright = rgb_to_xy(200, 200, 200).unwrap();
        assert!(close(dim.x, bright.x) && close(dim.y, bright.y));
    }

    #[test]
    fn black_has_no_chromaticity() {
        assert_eq!(rgb_to_xy(0, 0, 0), None);
    }

    #[test]
    fn srgb_decode_is_piecewise() {
        assert!(close(srgb_to_linear(0), 0.0));
        assert!(close(srgb_to_linear(255), 1.0));
        assert!(close(srgb_to_linear(10), 10.0 / 255.0 / 12.92));
        assert!(close(srgb_to_linear(128), 0.2158));
    }

    #[test]
    fn point_inside_gamut_is_unchanged() {
        let white = Xy::new(0.3127, 0.3290);
        assert!(Gamut::C.contains(white));
        assert_eq!(Gamut::C.clamp(white), white);
    }

    #[test]
    fn corners_and_edges_count_as_inside() {
        assert!(Gamut::C.contains(Gamut::C.red));
        let mid = Xy::new(
            f64::midpoint(Gamut::C.red.x, Gamut::C.blue.x),
            f64::midpoint(Gamut::C.red.y, Gamut::C.blue.y),
        );
        assert!(Gamut::C.contains(mid));
    }

    #[test]
    fn point_outside_is_pulled_onto_the_boundary() {
        // sRGB green is outside gamut A's neighbourhood on the red-green edge side.
        let outside = Xy::new(0.1, 0.9);
        assert!(!Gamut::C.contains(outside));
        let clamped = Gamut::C.clamp(outside);
        assert_ne!(clamped, outside);
        // The closest point lies on the boundary: re-clamping is a no-op.
        let again = Gamut::C.clamp(clamped);
        assert!(close(again.x, clamped.x) && close(again.y, clamped.y));
        // And it is the nearest of the boundary candidates.
        assert!(clamped.distance_sq(outside) <= Gamut::C.green.distance_sq(outside));
    }

    #[test]
    fn far_point_clamps_to_nearest_corner() {
        let clamped = Gamut::C.clamp(Xy::new(0.9, 0.05));
        // Beyond the red corner, away from the blue side.
        assert!(close(clamped.x, Gamut::C.red.x) && close(clamped.y, Gamut::C.red.y));
    }

    #[test]
    fn narrow_gamut_clamps_srgb_primaries() {
        // sRGB blue is outside gamut B (its blue corner is more saturated towards x=.167).
        let blue = rgb_to_xy(0, 0, 255).unwrap();
        let clamped = Gamut::B.clamp(blue);
        assert!(Gamut::B.contains(clamped));
    }

    #[test]
    fn gamut_validity() {
        assert!(Gamut::A.is_valid() && Gamut::B.is_valid() && Gamut::C.is_valid());
        let flat = Gamut {
            red: Xy::new(0.1, 0.1),
            green: Xy::new(0.2, 0.2),
            blue: Xy::new(0.3, 0.3),
        };
        assert!(!flat.is_valid());
        let nan = Gamut {
            red: Xy::new(f64::NAN, 0.1),
            ..Gamut::C
        };
        assert!(!nan.is_valid());
    }

    #[test]
    fn gamut_types() {
        assert_eq!(Gamut::from_type("A"), Some(Gamut::A));
        assert_eq!(Gamut::from_type("B"), Some(Gamut::B));
        assert_eq!(Gamut::from_type("C"), Some(Gamut::C));
        assert_eq!(Gamut::from_type("other"), None);
    }

    #[test]
    fn white_is_full_brightness() {
        let Mapped::On { xy, brightness } = map_color(255, 255, 255, &Gamut::C) else {
            panic!("white must be on");
        };
        assert!(close(brightness, 100.0));
        assert!(close(xy.x, 0.3127));
    }

    #[test]
    fn brightness_follows_the_largest_channel() {
        let Mapped::On { brightness, .. } = map_color(0, 0, 255, &Gamut::C) else {
            panic!()
        };
        assert!(close(brightness, 100.0));
        let Mapped::On { brightness, .. } = map_color(51, 10, 0, &Gamut::C) else {
            panic!()
        };
        assert!(close(brightness, 20.0));
    }

    #[test]
    fn near_black_turns_off_and_floor_applies_above() {
        assert_eq!(map_color(0, 0, 0, &Gamut::C), Mapped::Off);
        assert_eq!(map_color(2, 1, 0, &Gamut::C), Mapped::Off);
        let Mapped::On { brightness, .. } = map_color(3, 0, 0, &Gamut::C) else {
            panic!("3/255 must be lit");
        };
        assert!(brightness >= MIN_BRIGHTNESS);
        assert!(close(brightness, 3.0 / 255.0 * 100.0));
    }

    #[test]
    fn floor_keeps_dim_lit_colours_above_zero() {
        // 3/255 = 1.18 %, above the 1 % floor; the floor only matters if the threshold drops.
        assert!(MIN_BRIGHTNESS <= brightness_percent(OFF_BELOW, 0, 0));
    }

    #[test]
    fn mapped_xy_is_always_in_gamut() {
        for &(r, g, b) in &[
            (255, 0, 0),
            (0, 255, 0),
            (0, 0, 255),
            (255, 255, 0),
            (0, 255, 255),
            (255, 0, 255),
            (10, 200, 40),
        ] {
            for gamut in [Gamut::A, Gamut::B, Gamut::C] {
                let Mapped::On { xy, .. } = map_color(r, g, b, &gamut) else {
                    panic!()
                };
                assert!(
                    gamut.contains(Xy::new(xy.x, xy.y)) || {
                        // Allow float noise on the boundary.
                        let again = gamut.clamp(xy);
                        close(again.x, xy.x) && close(again.y, xy.y)
                    },
                    "{r},{g},{b} -> {xy:?}"
                );
            }
        }
    }

    #[test]
    fn brightness_only_mapping() {
        assert_eq!(map_brightness(1, 1, 1), Mapped::Off);
        let Mapped::On { brightness, .. } = map_brightness(0, 128, 0) else {
            panic!()
        };
        assert!(close(brightness, 128.0 / 255.0 * 100.0));
    }
}
