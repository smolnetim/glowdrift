//! Platform-independent helpers: mip selection, region mapping, CPU downscale, id hashing.

use glowdrift_core::NormRect;

/// Number of mip levels in a full chain for a `width` x `height` texture.
pub(crate) fn mip_count(width: u32, height: u32) -> u32 {
    u32::BITS - width.max(height).max(1).leading_zeros()
}

/// Size of mip `level` (each level halves, never below 1).
pub(crate) fn mip_size(width: u32, height: u32, level: u32) -> (u32, u32) {
    let shift = level.min(u32::BITS - 1);
    ((width >> shift).max(1), (height >> shift).max(1))
}

/// Picks the smallest mip whose longest side is still at least `max_dimension`.
/// When even level 0 is smaller than `max_dimension`, level 0 is returned.
/// The result is always below `mip_count`.
pub(crate) fn select_mip_level(width: u32, height: u32, max_dimension: u32, mip_count: u32) -> u32 {
    let target = max_dimension.max(1);
    let last = mip_count.saturating_sub(1);
    let mut level = 0;
    while level < last {
        let (w, h) = mip_size(width, height, level + 1);
        if w.max(h) < target {
            break;
        }
        level += 1;
    }
    level
}

/// Rotation of the displayed image relative to the unrotated duplicated surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Rotation {
    Identity,
    Rotate90,
    Rotate180,
    Rotate270,
}

/// Maps a region given in displayed-image fractions onto an unrotated surface whose displayed
/// orientation is `rotation` (the surface rotated clockwise by that angle is the displayed image).
pub(crate) fn region_on_surface(region: NormRect, rotation: Rotation) -> NormRect {
    let NormRect {
        x,
        y,
        width: w,
        height: h,
    } = region.clamped();
    match rotation {
        Rotation::Identity => NormRect::new(x, y, w, h),
        Rotation::Rotate90 => NormRect::new(y, 1.0 - x - w, h, w),
        Rotation::Rotate180 => NormRect::new(1.0 - x - w, 1.0 - y - h, w, h),
        Rotation::Rotate270 => NormRect::new(1.0 - y - h, x, h, w),
    }
}

/// Area-average downscale of BGRA8 pixels (in the encoded byte space). Alpha is forced opaque.
/// Returns tightly packed rows. Panics if the source slice is too small for its dimensions.
pub(crate) fn downscale_bgra(
    src: &[u8],
    src_width: u32,
    src_height: u32,
    src_stride: usize,
    dst_width: u32,
    dst_height: u32,
) -> Vec<u8> {
    let (sw, sh) = (src_width as usize, src_height as usize);
    let (dw, dh) = (dst_width as usize, dst_height as usize);
    let mut out = vec![0u8; dw * dh * 4];
    if sw == 0 || sh == 0 || dw == 0 || dh == 0 {
        return out;
    }
    let range = |i: usize, dst: usize, srcn: usize| {
        let a = (i * srcn / dst).min(srcn - 1);
        let b = ((i + 1) * srcn).div_ceil(dst).clamp(a + 1, srcn);
        (a, b)
    };
    for dy in 0..dh {
        let (y0, y1) = range(dy, dh, sh);
        for dx in 0..dw {
            let (x0, x1) = range(dx, dw, sw);
            let mut sum = [0u64; 3];
            for y in y0..y1 {
                let row = &src[y * src_stride..y * src_stride + sw * 4];
                for px in row[x0 * 4..x1 * 4].as_chunks::<4>().0 {
                    sum[0] += u64::from(px[0]);
                    sum[1] += u64::from(px[1]);
                    sum[2] += u64::from(px[2]);
                }
            }
            let n = ((y1 - y0) * (x1 - x0)) as u64;
            let o = (dy * dw + dx) * 4;
            for (c, s) in sum.iter().enumerate() {
                out[o + c] = ((s + n / 2) / n) as u8;
            }
            out[o + 3] = 255;
        }
    }
    out
}

/// Output size that keeps the aspect ratio and is at most `max_width` wide (never upscales).
pub(crate) fn fit_width(width: u32, height: u32, max_width: u32) -> (u32, u32) {
    let target = max_width.max(1);
    if width <= target {
        return (width.max(1), height.max(1));
    }
    let h = (u64::from(height) * u64::from(target) + u64::from(width) / 2) / u64::from(width);
    (target, (h as u32).max(1))
}

/// Longest-side target that yields a mip at least `max_width` wide for a surface of this shape.
pub(crate) fn thumbnail_dimension(width: u32, height: u32, max_width: u32) -> u32 {
    let target = u64::from(max_width.max(1));
    let longest = u64::from(width.max(height).max(1));
    let scaled = (target * longest).div_ceil(u64::from(width.max(1)));
    u32::try_from(scaled).unwrap_or(u32::MAX)
}

/// Stable 64-bit FNV-1a hash of the lowercased text, rendered as `mon-<16 hex digits>`.
pub(crate) fn stable_id(text: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.to_lowercase().bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("mon-{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use glowdrift_core::PixelRect;

    #[test]
    fn mip_count_is_floor_log2_plus_one() {
        assert_eq!(mip_count(1, 1), 1);
        assert_eq!(mip_count(2, 1), 2);
        assert_eq!(mip_count(1920, 1080), 11);
        assert_eq!(mip_count(5120, 1440), 13);
        assert_eq!(mip_count(0, 0), 1);
    }

    #[test]
    fn mip_size_halves_and_floors_at_one() {
        assert_eq!(mip_size(5120, 1440, 0), (5120, 1440));
        assert_eq!(mip_size(5120, 1440, 3), (640, 180));
        assert_eq!(mip_size(100, 3, 4), (6, 1));
        assert_eq!(mip_size(100, 3, 99), (1, 1));
    }

    #[test]
    fn select_mip_is_closest_not_below() {
        let n = mip_count(5120, 1440);
        assert_eq!(select_mip_level(5120, 1440, 640, n), 3);
        assert_eq!(select_mip_level(5120, 1440, 641, n), 2);
        assert_eq!(select_mip_level(5120, 1440, 5120, n), 0);
        assert_eq!(select_mip_level(5120, 1440, 64, n), 6);
        for target in [1u32, 7, 33, 100, 500, 1000, 3000] {
            let level = select_mip_level(3440, 1440, target, mip_count(3440, 1440));
            let (w, h) = mip_size(3440, 1440, level);
            assert!(w.max(h) >= target.min(3440));
        }
    }

    #[test]
    fn select_mip_handles_small_sources_and_zero() {
        assert_eq!(select_mip_level(50, 30, 64, mip_count(50, 30)), 0);
        let n = mip_count(1920, 1080);
        assert_eq!(select_mip_level(1920, 1080, 0, n), n - 1);
        assert_eq!(select_mip_level(1920, 1080, 1, n), n - 1);
        assert_eq!(select_mip_level(1, 1, 64, 1), 0);
    }

    #[test]
    fn region_to_pixel_rect_uses_rounded_edges() {
        let r = NormRect::new(0.25, 0.5, 0.5, 0.25).to_pixels(5120, 1440);
        assert_eq!(
            r,
            PixelRect {
                x: 1280,
                y: 720,
                width: 2560,
                height: 360
            }
        );
        let full = NormRect::full().to_pixels(3440, 1440);
        assert_eq!(
            (full.x, full.y, full.width, full.height),
            (0, 0, 3440, 1440)
        );
        let tiny = NormRect::new(5.0, -3.0, 0.0, f32::NAN).to_pixels(1920, 1080);
        assert!(tiny.width >= 1 && tiny.height >= 1);
        assert!(tiny.x + tiny.width <= 1920 && tiny.y + tiny.height <= 1080);
    }

    #[test]
    fn region_rotation_mapping() {
        let r = NormRect::new(0.1, 0.2, 0.3, 0.4);
        assert_eq!(region_on_surface(r, Rotation::Identity), r);
        let r90 = region_on_surface(r, Rotation::Rotate90);
        assert_eq!(r90, NormRect::new(0.2, 1.0 - 0.1 - 0.3, 0.4, 0.3));
        let r180 = region_on_surface(r, Rotation::Rotate180);
        assert!((r180.x - 0.6).abs() < 1e-6 && (r180.y - 0.4).abs() < 1e-6);
        let back = region_on_surface(r90, Rotation::Rotate270);
        assert!((back.x - r.x).abs() < 1e-6 && (back.y - r.y).abs() < 1e-6);
        assert!((back.width - r.width).abs() < 1e-6 && (back.height - r.height).abs() < 1e-6);
        for rot in [Rotation::Rotate90, Rotation::Rotate180, Rotation::Rotate270] {
            assert_eq!(region_on_surface(NormRect::full(), rot), NormRect::full());
        }
    }

    #[test]
    fn downscale_averages_blocks_and_forces_opaque() {
        let src = [0, 0, 0, 0, 100, 200, 40, 7, 100, 0, 0, 0, 0, 200, 40, 9];
        let out = downscale_bgra(&src, 2, 2, 8, 1, 1);
        assert_eq!(out, vec![50, 100, 20, 255]);
        let mut padded = vec![0u8; 3 * 16];
        for row in 0..3 {
            for px in 0..3 {
                padded[row * 16 + px * 4..row * 16 + px * 4 + 4].copy_from_slice(&[10, 20, 30, 0]);
            }
        }
        let out = downscale_bgra(&padded, 3, 3, 16, 2, 2);
        assert_eq!(out.len(), 16);
        assert!(
            out.as_chunks::<4>()
                .0
                .iter()
                .all(|p| *p == [10, 20, 30, 255])
        );
    }

    #[test]
    fn fit_width_keeps_aspect_and_never_upscales() {
        assert_eq!(fit_width(5120, 1440, 640), (640, 180));
        assert_eq!(fit_width(1920, 1080, 4000), (1920, 1080));
        assert_eq!(fit_width(1080, 1920, 270), (270, 480));
        assert_eq!(fit_width(100, 100, 0), (1, 1));
    }

    #[test]
    fn thumbnail_dimension_tracks_width_for_any_orientation() {
        assert_eq!(thumbnail_dimension(5120, 1440, 640), 640);
        assert_eq!(thumbnail_dimension(1080, 1920, 270), 480);
        assert_eq!(thumbnail_dimension(100, 100, 0), 1);
    }

    #[test]
    fn stable_id_is_deterministic_and_case_insensitive() {
        let a = stable_id("\\\\?\\DISPLAY#GSM5B09#5&2d6e&0&UID4352#{abc}");
        assert_eq!(
            a,
            stable_id("\\\\?\\display#gsm5b09#5&2d6e&0&uid4352#{ABC}")
        );
        assert_ne!(
            a,
            stable_id("\\\\?\\DISPLAY#GSM5B09#5&2d6e&0&UID4353#{abc}")
        );
        assert!(a.starts_with("mon-") && a.len() == 4 + 16);
    }
}
