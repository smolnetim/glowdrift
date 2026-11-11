//! Borrowed pixel-buffer view used as input to reduction.

/// A borrowed BGRA8 frame whose bytes are sRGB-encoded.
#[derive(Debug, Clone, Copy)]
pub struct FrameView<'a> {
    /// Pixel bytes in B, G, R, A order. Row `y` starts at `y * stride`.
    pub data: &'a [u8],
    pub width: u32,
    pub height: u32,
    /// Bytes between the starts of consecutive rows (at least `width * 4`).
    pub stride: usize,
}

impl<'a> FrameView<'a> {
    pub const BYTES_PER_PIXEL: usize = 4;

    pub fn new(data: &'a [u8], width: u32, height: u32, stride: usize) -> Self {
        Self {
            data,
            width,
            height,
            stride,
        }
    }

    /// True when the dimensions are non-zero and `data` is large enough for every row.
    pub fn is_valid(&self) -> bool {
        if self.width == 0 || self.height == 0 {
            return false;
        }
        let row = self.width as usize * Self::BYTES_PER_PIXEL;
        let needed = self.stride * (self.height as usize - 1) + row;
        self.stride >= row && self.data.len() >= needed
    }

    /// Returns the BGRA bytes of row `y`, without stride padding, or `None` if out of range.
    pub fn row(&self, y: u32) -> Option<&'a [u8]> {
        if !self.is_valid() || y >= self.height {
            return None;
        }
        let start = self.stride * y as usize;
        self.data
            .get(start..start + self.width as usize * Self::BYTES_PER_PIXEL)
    }
}
