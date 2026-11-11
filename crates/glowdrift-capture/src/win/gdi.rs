//! GDI `BitBlt` screen grab, used when Desktop Duplication cannot deliver a thumbnail.

use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CAPTUREBLT, CreateCompatibleDC, CreateDIBSection,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, HBITMAP, HDC, HGDIOBJ, ReleaseDC, SRCCOPY,
    SelectObject,
};

use super::monitors::MonitorEntry;
use crate::{CaptureError, Frame};

struct ScreenDc(HDC);

impl Drop for ScreenDc {
    fn drop(&mut self) {
        // SAFETY: the DC came from `GetDC(None)` and is released exactly once.
        unsafe { ReleaseDC(None, self.0) };
    }
}

struct MemoryDc(HDC);

impl Drop for MemoryDc {
    fn drop(&mut self) {
        // SAFETY: the DC came from `CreateCompatibleDC` and is deleted exactly once.
        unsafe {
            let _ = DeleteDC(self.0);
        }
    }
}

struct Bitmap(HBITMAP);

impl Drop for Bitmap {
    fn drop(&mut self) {
        // SAFETY: the bitmap came from `CreateDIBSection`, is deselected from every DC by field
        // drop order in `capture_full`, and is deleted exactly once.
        unsafe {
            let _ = DeleteObject(HGDIOBJ(self.0.0));
        }
    }
}

/// Copies the whole monitor into an owned BGRA frame at native size (alpha forced opaque).
///
/// Coordinates are physical pixels, so the calling process must be per-monitor DPI aware.
pub(crate) fn capture_full(entry: &MonitorEntry) -> Result<Frame, CaptureError> {
    let (width, height) = (entry.info.width, entry.info.height);
    if width == 0 || height == 0 {
        return Err(CaptureError::Os("monitor has no area".into()));
    }
    let (Ok(width_i32), Ok(height_i32)) = (i32::try_from(width), i32::try_from(height)) else {
        return Err(CaptureError::Os("monitor size out of range".into()));
    };
    let header = BITMAPINFOHEADER {
        biSize: size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: width_i32,
        // Negative height requests a top-down bitmap.
        biHeight: -height_i32,
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB.0,
        ..Default::default()
    };
    let info = BITMAPINFO {
        bmiHeader: header,
        ..Default::default()
    };

    // SAFETY: GDI calls below follow the documented lifetime rules: every object created here is
    // owned by a guard that releases it, and the bitmap is deselected before it is deleted.
    unsafe {
        let screen = ScreenDc(GetDC(None));
        if screen.0.is_invalid() {
            return Err(CaptureError::Os("GetDC failed".into()));
        }
        let memory = MemoryDc(CreateCompatibleDC(Some(screen.0)));
        if memory.0.is_invalid() {
            return Err(CaptureError::Os("CreateCompatibleDC failed".into()));
        }
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let bitmap = Bitmap(
            CreateDIBSection(
                Some(screen.0),
                &raw const info,
                DIB_RGB_COLORS,
                &raw mut bits,
                None,
                0,
            )
            .map_err(|e| CaptureError::Os(format!("CreateDIBSection: {e}")))?,
        );
        if bits.is_null() {
            return Err(CaptureError::Os(
                "CreateDIBSection returned no pixels".into(),
            ));
        }
        let previous = SelectObject(memory.0, HGDIOBJ(bitmap.0.0));
        let blit = BitBlt(
            memory.0,
            0,
            0,
            width_i32,
            height_i32,
            Some(screen.0),
            entry.info.x,
            entry.info.y,
            SRCCOPY | CAPTUREBLT,
        );
        SelectObject(memory.0, previous);
        blit.map_err(|e| CaptureError::Os(format!("BitBlt: {e}")))?;

        let len = width as usize * height as usize * 4;
        // SAFETY: the DIB section owns `width * height * 4` bytes at `bits` for as long as
        // `bitmap` lives, and the blit has completed.
        let mut data = std::slice::from_raw_parts(bits.cast::<u8>(), len).to_vec();
        for pixel in data.as_chunks_mut::<4>().0 {
            pixel[3] = 255;
        }
        Ok(Frame {
            data,
            width,
            height,
            stride: width as usize * 4,
        })
    }
}
