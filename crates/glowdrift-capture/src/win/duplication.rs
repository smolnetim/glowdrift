//! DXGI Desktop Duplication with a GPU-side mip-chain downscale.
//!
//! Every acquired desktop image is copied into a persistent GPU texture (`cache`), so a region
//! change on a static screen can still be served without waiting for the next desktop update.
//! The requested region is copied from the cache into a texture with a full mip chain,
//! `GenerateMips` averages it (in linear light when the sRGB format is usable), and only the
//! selected small mip level is copied to a staging texture and read back.

use std::time::{Duration, Instant};

use glowdrift_core::{NormRect, PixelRect};
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL_10_0, D3D_FEATURE_LEVEL_10_1,
    D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_BOX, D3D11_CPU_ACCESS_READ,
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_FORMAT_SUPPORT_MIP_AUTOGEN,
    D3D11_FORMAT_SUPPORT_RENDER_TARGET, D3D11_FORMAT_SUPPORT_SHADER_SAMPLE,
    D3D11_FORMAT_SUPPORT_TEXTURE2D, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE,
    D3D11_RESOURCE_MISC_GENERATE_MIPS, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC,
    D3D11_USAGE_DEFAULT, D3D11_USAGE_STAGING, D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext,
    ID3D11ShaderResourceView, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_B8G8R8A8_UNORM_SRGB,
    DXGI_MODE_ROTATION_ROTATE90, DXGI_MODE_ROTATION_ROTATE180, DXGI_MODE_ROTATION_ROTATE270,
    DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    DXGI_OUTDUPL_FRAME_INFO, IDXGIOutput1, IDXGIOutputDuplication, IDXGIResource,
};
use windows::core::Interface;

use crate::math::{
    Rotation, mip_count, mip_size, region_on_surface, select_mip_level, thumbnail_dimension,
};
use crate::{CaptureError, Frame};

use super::error::{Class, classify, map_err};
use super::monitors::MonitorEntry;

struct Cache {
    texture: ID3D11Texture2D,
    width: u32,
    height: u32,
}

struct Work {
    texture: ID3D11Texture2D,
    view: ID3D11ShaderResourceView,
    width: u32,
    height: u32,
    mips: u32,
}

struct Staging {
    texture: ID3D11Texture2D,
    width: u32,
    height: u32,
}

/// Desktop Duplication session plus all reusable GPU resources.
pub(crate) struct Duplicator {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    duplication: IDXGIOutputDuplication,
    max_dimension: u32,
    /// Size of the display as the user sees it, to detect an unrotated surface on rotated outputs.
    display_size: (u32, u32),
    surface_rotation: Rotation,
    mip_format: DXGI_FORMAT,
    cache: Option<Cache>,
    work: Option<Work>,
    staging: Option<Staging>,
    /// Region of the most recently delivered frame.
    last_region: Option<NormRect>,
}

// SAFETY: The D3D11 device is free-threaded, and every other COM object (immediate context,
// duplication interface, textures, view) is created from it, owned exclusively by this struct
// and only touched through `&mut self`. Moving the struct to another thread therefore never
// results in concurrent use of the immediate context. It is intentionally not `Sync`.
unsafe impl Send for Duplicator {}

impl Duplicator {
    pub(crate) fn new(entry: &MonitorEntry, max_dimension: u32) -> Result<Self, CaptureError> {
        let levels = [
            D3D_FEATURE_LEVEL_11_1,
            D3D_FEATURE_LEVEL_11_0,
            D3D_FEATURE_LEVEL_10_1,
            D3D_FEATURE_LEVEL_10_0,
        ];
        let mut device: Option<ID3D11Device> = None;
        let mut context: Option<ID3D11DeviceContext> = None;
        // SAFETY: out pointers refer to live locals; the adapter is a live COM object. The driver
        // type must be UNKNOWN when an explicit adapter is supplied.
        unsafe {
            D3D11CreateDevice(
                &entry.adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&levels),
                D3D11_SDK_VERSION,
                Some(&raw mut device),
                None,
                Some(&raw mut context),
            )
        }
        .map_err(|e| map_err("D3D11CreateDevice", &e))?;
        let (Some(device), Some(context)) = (device, context) else {
            return Err(CaptureError::Os(
                "D3D11CreateDevice returned no device".into(),
            ));
        };

        let output1: IDXGIOutput1 = entry
            .output
            .cast()
            .map_err(|e| map_err("IDXGIOutput1 cast", &e))?;
        // SAFETY: `device` is the live device created on the adapter that owns the output.
        let duplication = unsafe { output1.DuplicateOutput(&device) }
            .map_err(|e| map_err("DuplicateOutput", &e))?;
        // SAFETY: plain getter on a live COM object.
        let desc = unsafe { duplication.GetDesc() };
        let surface_rotation = match desc.Rotation {
            DXGI_MODE_ROTATION_ROTATE90 => Rotation::Rotate90,
            DXGI_MODE_ROTATION_ROTATE180 => Rotation::Rotate180,
            DXGI_MODE_ROTATION_ROTATE270 => Rotation::Rotate270,
            _ => Rotation::Identity,
        };

        Ok(Self {
            mip_format: choose_mip_format(&device),
            device,
            context,
            duplication,
            max_dimension,
            display_size: (entry.info.width, entry.info.height),
            surface_rotation,
            cache: None,
            work: None,
            staging: None,
            last_region: None,
        })
    }

    pub(crate) fn grab(
        &mut self,
        region: NormRect,
        timeout: Duration,
    ) -> Result<Option<Frame>, CaptureError> {
        let region = region.clamped();
        let deadline = Instant::now() + timeout;
        loop {
            // A region edit on a static screen is served from the cached desktop image without
            // waiting; the poll below still folds in any desktop update that is already pending.
            let region_changed = self.cache.is_some() && self.last_region != Some(region);
            let wait = if region_changed {
                Duration::ZERO
            } else {
                deadline.saturating_duration_since(Instant::now())
            };
            if self.acquire(wait)? || region_changed {
                break;
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
        }
        let frame = self.render(region, self.max_dimension)?;
        self.last_region = Some(region);
        Ok(Some(frame))
    }

    pub(crate) fn set_max_dimension(&mut self, max_dimension: u32) {
        if self.max_dimension != max_dimension {
            self.max_dimension = max_dimension;
            self.last_region = None;
        }
    }

    /// Renders the whole desktop image at a size suitable for `max_width`, waiting up to
    /// `timeout` for the first desktop image when none has been seen yet. The result may be up to
    /// twice `max_width` wide; the caller shrinks it exactly. The next `grab` delivers a fresh
    /// frame because the shared GPU work texture is reused for this render.
    pub(crate) fn snapshot(
        &mut self,
        max_width: u32,
        timeout: Duration,
    ) -> Result<Frame, CaptureError> {
        let deadline = Instant::now() + timeout;
        // Pick up a desktop update that is already waiting.
        self.acquire(Duration::ZERO)?;
        while self.cache.is_none() {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(CaptureError::Os("no desktop frame delivered".into()));
            }
            self.acquire(remaining)?;
        }
        let Some(cache) = &self.cache else {
            return Err(CaptureError::Os("no desktop image cached".into()));
        };
        let dimension = thumbnail_dimension(cache.width, cache.height, max_width);
        let frame = self.render(NormRect::full(), dimension)?;
        self.last_region = None;
        Ok(frame)
    }

    /// Waits for one duplication event. Returns true when a new desktop image was stored in the
    /// cache, false on timeout or when only the pointer changed.
    fn acquire(&mut self, wait: Duration) -> Result<bool, CaptureError> {
        let millis = u32::try_from(wait.as_millis()).unwrap_or(u32::MAX);
        let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut resource: Option<IDXGIResource> = None;
        // SAFETY: out pointers refer to live locals.
        let acquired = unsafe {
            self.duplication
                .AcquireNextFrame(millis, &raw mut info, &raw mut resource)
        };
        if let Err(e) = acquired {
            return match classify(e.code().0) {
                Class::Timeout => Ok(false),
                _ => Err(map_err("AcquireNextFrame", &e)),
            };
        }

        let desktop_updated = info.LastPresentTime != 0 || info.AccumulatedFrames > 0;
        let stored = if desktop_updated {
            resource
                .ok_or_else(|| CaptureError::Os("AcquireNextFrame returned no resource".into()))
                .and_then(|r| self.store_desktop(&r))
        } else {
            Ok(())
        };
        // SAFETY: a frame was acquired above and has not been released yet. It is released on
        // every path, including after a failed copy.
        let released = unsafe { self.duplication.ReleaseFrame() };
        stored?;
        released.map_err(|e| map_err("ReleaseFrame", &e))?;
        Ok(desktop_updated)
    }

    fn store_desktop(&mut self, resource: &IDXGIResource) -> Result<(), CaptureError> {
        let desktop: ID3D11Texture2D = resource
            .cast()
            .map_err(|e| map_err("desktop texture cast", &e))?;
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: `desc` is a live out parameter.
        unsafe { desktop.GetDesc(&raw mut desc) };
        if desc.Format != DXGI_FORMAT_B8G8R8A8_UNORM {
            return Err(CaptureError::Os(format!(
                "unsupported desktop surface format {}",
                desc.Format.0
            )));
        }
        let reuse = self
            .cache
            .as_ref()
            .is_some_and(|c| c.width == desc.Width && c.height == desc.Height);
        if !reuse {
            let cache_desc = D3D11_TEXTURE2D_DESC {
                MipLevels: 1,
                ArraySize: 1,
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: 0,
                CPUAccessFlags: 0,
                MiscFlags: 0,
                ..desc
            };
            self.cache = Some(Cache {
                texture: self.create_texture(&cache_desc)?,
                width: desc.Width,
                height: desc.Height,
            });
            self.work = None;
        }
        if let Some(cache) = &self.cache {
            // SAFETY: both textures live on `self.device`, share size and format.
            unsafe { self.context.CopyResource(&cache.texture, &desktop) };
        }
        Ok(())
    }

    fn create_texture(&self, desc: &D3D11_TEXTURE2D_DESC) -> Result<ID3D11Texture2D, CaptureError> {
        let mut texture: Option<ID3D11Texture2D> = None;
        // SAFETY: `desc` and the out pointer refer to live locals.
        unsafe {
            self.device
                .CreateTexture2D(desc, None, Some(&raw mut texture))
        }
        .map_err(|e| map_err("CreateTexture2D", &e))?;
        texture.ok_or_else(|| CaptureError::Os("CreateTexture2D returned no texture".into()))
    }

    fn ensure_work(&mut self, width: u32, height: u32) -> Result<(), CaptureError> {
        if self
            .work
            .as_ref()
            .is_some_and(|w| w.width == width && w.height == height)
        {
            return Ok(());
        }
        let mips = mip_count(width, height);
        let desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: mips,
            ArraySize: 1,
            Format: self.mip_format,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_RENDER_TARGET.0) as u32,
            CPUAccessFlags: 0,
            MiscFlags: D3D11_RESOURCE_MISC_GENERATE_MIPS.0 as u32,
        };
        let texture = self.create_texture(&desc)?;
        let mut view: Option<ID3D11ShaderResourceView> = None;
        // SAFETY: the default view covers all mip levels of a live texture.
        unsafe {
            self.device
                .CreateShaderResourceView(&texture, None, Some(&raw mut view))
        }
        .map_err(|e| map_err("CreateShaderResourceView", &e))?;
        let view = view
            .ok_or_else(|| CaptureError::Os("CreateShaderResourceView returned no view".into()))?;
        self.work = Some(Work {
            texture,
            view,
            width,
            height,
            mips,
        });
        Ok(())
    }

    fn ensure_staging(&mut self, width: u32, height: u32) -> Result<(), CaptureError> {
        if self
            .staging
            .as_ref()
            .is_some_and(|s| s.width == width && s.height == height)
        {
            return Ok(());
        }
        let desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: self.mip_format,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
        };
        self.staging = Some(Staging {
            texture: self.create_texture(&desc)?,
            width,
            height,
        });
        Ok(())
    }

    /// Pixel rectangle of `region` on the cached surface, correcting for rotated outputs.
    fn region_pixels(&self, cache: &Cache, region: NormRect) -> PixelRect {
        // A rotated display reports its surface unrotated when the surface dimensions are the
        // display dimensions swapped; only then does the region need rotating.
        let swapped = self.display_size.0 == cache.height
            && self.display_size.1 == cache.width
            && cache.width != cache.height;
        let surface_region = match self.surface_rotation {
            Rotation::Rotate90 | Rotation::Rotate270 if swapped => {
                region_on_surface(region, self.surface_rotation)
            }
            _ => region,
        };
        surface_region.to_pixels(cache.width, cache.height)
    }

    fn render(&mut self, region: NormRect, max_dimension: u32) -> Result<Frame, CaptureError> {
        let (rect, cache_texture) = match &self.cache {
            Some(cache) => (self.region_pixels(cache, region), cache.texture.clone()),
            None => return Err(CaptureError::Os("no desktop image cached".into())),
        };
        self.ensure_work(rect.width, rect.height)?;
        let Some(work) = &self.work else {
            return Err(CaptureError::Os("work texture missing".into()));
        };
        let (work_texture, work_view, mips) = (work.texture.clone(), work.view.clone(), work.mips);
        let level = select_mip_level(rect.width, rect.height, max_dimension, mips);
        let (out_w, out_h) = mip_size(rect.width, rect.height, level);
        self.ensure_staging(out_w, out_h)?;
        let Some(staging) = &self.staging else {
            return Err(CaptureError::Os("staging texture missing".into()));
        };
        let staging_texture = staging.texture.clone();

        let source_box = D3D11_BOX {
            left: rect.x,
            top: rect.y,
            front: 0,
            right: rect.x + rect.width,
            bottom: rect.y + rect.height,
            back: 1,
        };
        // SAFETY: all resources belong to `self.device`; the box lies inside the cached surface
        // (guaranteed by `PixelRect` clamping) and the destination mip 0 has the region's size.
        // For a full mip chain, subresource index equals the mip level.
        unsafe {
            self.context.CopySubresourceRegion(
                &work_texture,
                0,
                0,
                0,
                0,
                &cache_texture,
                0,
                Some(&raw const source_box),
            );
            if level > 0 {
                self.context.GenerateMips(&work_view);
            }
            self.context.CopySubresourceRegion(
                &staging_texture,
                0,
                0,
                0,
                0,
                &work_texture,
                level,
                None,
            );
        }
        self.read_back(&staging_texture, out_w, out_h)
    }

    fn read_back(
        &self,
        staging: &ID3D11Texture2D,
        width: u32,
        height: u32,
    ) -> Result<Frame, CaptureError> {
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        // SAFETY: the staging texture is CPU-readable and not mapped elsewhere.
        unsafe {
            self.context
                .Map(staging, 0, D3D11_MAP_READ, 0, Some(&raw mut mapped))
        }
        .map_err(|e| map_err("Map", &e))?;

        let row_bytes = width as usize * 4;
        let mut data = vec![0u8; row_bytes * height as usize];
        let base = mapped.pData.cast::<u8>().cast_const();
        for (y, row) in data.chunks_exact_mut(row_bytes).enumerate() {
            // SAFETY: while mapped, `pData` addresses `height` rows of `RowPitch` bytes, each at
            // least `width * 4` bytes long; `y < height`.
            let src = unsafe {
                std::slice::from_raw_parts(base.add(y * mapped.RowPitch as usize), row_bytes)
            };
            row.copy_from_slice(src);
        }
        // SAFETY: the resource was mapped above.
        unsafe { self.context.Unmap(staging, 0) };

        for pixel in data.as_chunks_mut::<4>().0 {
            pixel[3] = 255;
        }
        Ok(Frame {
            data,
            width,
            height,
            stride: row_bytes,
        })
    }
}

/// Prefers an sRGB format so mip generation averages in linear light; falls back to UNORM when
/// the device cannot auto-generate mips for it.
fn choose_mip_format(device: &ID3D11Device) -> DXGI_FORMAT {
    let needed = (D3D11_FORMAT_SUPPORT_TEXTURE2D.0
        | D3D11_FORMAT_SUPPORT_MIP_AUTOGEN.0
        | D3D11_FORMAT_SUPPORT_RENDER_TARGET.0
        | D3D11_FORMAT_SUPPORT_SHADER_SAMPLE.0) as u32;
    // SAFETY: plain query on a live device.
    let srgb_ok = unsafe { device.CheckFormatSupport(DXGI_FORMAT_B8G8R8A8_UNORM_SRGB) }
        .is_ok_and(|bits| bits & needed == needed);
    if srgb_ok {
        DXGI_FORMAT_B8G8R8A8_UNORM_SRGB
    } else {
        log::warn!("sRGB mip generation unavailable; averaging in gamma space");
        DXGI_FORMAT_B8G8R8A8_UNORM
    }
}
