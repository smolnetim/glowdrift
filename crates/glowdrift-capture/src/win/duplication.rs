//! DXGI Desktop Duplication with a GPU-side mip-chain downscale.
//!
//! Every acquired desktop image is copied into a persistent GPU texture (`cache`), so a region
//! change on a static screen can still be served without waiting for the next desktop update.
//! The requested region is copied from the cache into a texture with a full mip chain,
//! `GenerateMips` averages it (in linear light when the sRGB format is usable), and only the
//! selected small mip level is copied to a staging texture and read back.
//!
//! Frames of an unchanged region are read back asynchronously: a render is recorded into one of a
//! small ring of staging textures, the thread sleeps on a GPU fence (or polls briefly when fences
//! are unavailable) and the result is collected with `D3D11_MAP_FLAG_DO_NOT_WAIT`, so the thread
//! never blocks inside `Map`. A render the GPU does not finish within the grab timeout is
//! returned by the next grab. Region or size changes and snapshots read back synchronously.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use glowdrift_core::{NormRect, PixelRect};
use windows::Win32::Foundation::{CloseHandle, HANDLE, HMODULE};
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL_10_0, D3D_FEATURE_LEVEL_10_1,
    D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_BOX, D3D11_CPU_ACCESS_READ,
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_FENCE_FLAG_NONE, D3D11_FORMAT_SUPPORT_MIP_AUTOGEN,
    D3D11_FORMAT_SUPPORT_RENDER_TARGET, D3D11_FORMAT_SUPPORT_SHADER_SAMPLE,
    D3D11_FORMAT_SUPPORT_TEXTURE2D, D3D11_MAP_FLAG_DO_NOT_WAIT, D3D11_MAP_READ,
    D3D11_MAPPED_SUBRESOURCE, D3D11_RESOURCE_MISC_GENERATE_MIPS, D3D11_SDK_VERSION,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, D3D11_USAGE_STAGING, D3D11CreateDevice,
    ID3D11Device, ID3D11Device5, ID3D11DeviceContext, ID3D11DeviceContext4, ID3D11Fence,
    ID3D11ShaderResourceView, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_B8G8R8A8_UNORM_SRGB,
    DXGI_MODE_ROTATION_ROTATE90, DXGI_MODE_ROTATION_ROTATE180, DXGI_MODE_ROTATION_ROTATE270,
    DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    DXGI_ERROR_WAS_STILL_DRAWING, DXGI_OUTDUPL_FRAME_INFO, IDXGIOutput1, IDXGIOutputDuplication,
    IDXGIResource,
};
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
use windows::core::Interface;

use crate::math::{
    Rotation, mip_count, mip_size, region_on_surface, select_mip_level, thumbnail_dimension,
};
use crate::{CaptureError, Frame, GrabTimings};

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

/// Number of staging textures that can hold a frame the GPU has not finished yet.
const RING_SLOTS: usize = 3;
/// Longest single wait for an in-flight readback when no fence is available.
const POLL_INTERVAL: Duration = Duration::from_millis(1);

/// GPU fence that lets the thread sleep until recorded work has finished.
struct GpuSignal {
    fence: ID3D11Fence,
    context: ID3D11DeviceContext4,
    event: HANDLE,
    last: u64,
}

impl GpuSignal {
    fn new(device: &ID3D11Device, context: &ID3D11DeviceContext) -> Option<Self> {
        let device5: ID3D11Device5 = device.cast().ok()?;
        let context4: ID3D11DeviceContext4 = context.cast().ok()?;
        let mut fence: Option<ID3D11Fence> = None;
        // SAFETY: out pointer refers to a live local.
        unsafe { device5.CreateFence(0, D3D11_FENCE_FLAG_NONE, &raw mut fence) }.ok()?;
        // SAFETY: plain event creation without name or security attributes.
        let event = unsafe { CreateEventW(None, false, false, None) }.ok()?;
        Some(Self {
            fence: fence?,
            context: context4,
            event,
            last: 0,
        })
    }

    /// Queues a signal after all work recorded so far and returns its value.
    fn signal(&mut self) -> Option<u64> {
        let value = self.last + 1;
        // SAFETY: the fence and context belong to the same device.
        unsafe { self.context.Signal(&self.fence, value) }.ok()?;
        self.last = value;
        Some(value)
    }

    /// Sleeps until the GPU has passed `value` or `timeout` elapsed.
    fn wait(&self, value: u64, timeout: Duration) {
        // SAFETY: plain getter on a live fence.
        if unsafe { self.fence.GetCompletedValue() } >= value {
            return;
        }
        // SAFETY: the event handle is owned by `self` and stays open.
        if unsafe { self.fence.SetEventOnCompletion(value, self.event) }.is_err() {
            return;
        }
        let millis = u32::try_from(timeout.as_micros().div_ceil(1000)).unwrap_or(u32::MAX);
        // SAFETY: the event handle is owned by `self` and stays open.
        unsafe { WaitForSingleObject(self.event, millis) };
    }
}

impl Drop for GpuSignal {
    fn drop(&mut self) {
        // SAFETY: the handle was created in `new` and is closed exactly once.
        let _ = unsafe { CloseHandle(self.event) };
    }
}

/// What a delivered frame was rendered for.
#[derive(Clone, Copy, PartialEq)]
struct Key {
    region: NormRect,
    max_dimension: u32,
}

#[derive(Default)]
struct Slot {
    staging: Option<Staging>,
    busy: bool,
}

/// A render recorded on the GPU whose result has not been read back yet.
struct Pending {
    slot: usize,
    key: Key,
    width: u32,
    height: u32,
    present_qpc: i64,
    fence_value: Option<u64>,
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
    /// Staging texture of synchronous readbacks.
    staging: Option<Staging>,
    ring: [Slot; RING_SLOTS],
    pending: VecDeque<Pending>,
    fence: Option<GpuSignal>,
    /// What the most recent render was made for.
    last_key: Option<Key>,
    /// The cache holds a desktop image that no render has been recorded for yet.
    dirty: bool,
    /// Presentation time (QPC ticks) of the image in the cache.
    present_qpc: i64,
    timings: GrabTimings,
}

// SAFETY: The D3D11 device is free-threaded, and every other COM object (immediate context,
// duplication interface, textures, view, fence) is created from it, owned exclusively by this
// struct and only touched through `&mut self`; the fence's event is a plain kernel handle. Moving the struct to another thread therefore never
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

        let fence = GpuSignal::new(&device, &context);
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
            ring: Default::default(),
            pending: VecDeque::new(),
            fence,
            last_key: None,
            dirty: false,
            present_qpc: 0,
            timings: GrabTimings::default(),
        })
    }

    pub(crate) fn grab(
        &mut self,
        region: NormRect,
        timeout: Duration,
    ) -> Result<Option<Frame>, CaptureError> {
        let key = Key {
            region: region.clamped(),
            max_dimension: self.max_dimension,
        };
        self.timings = GrabTimings::default();
        let deadline = Instant::now() + timeout;
        let mut ready = self.collect_ready(key)?;
        loop {
            // A region edit on a static screen is served from the cached desktop image without
            // waiting; the poll below still folds in any desktop update that is already pending.
            let remaining = deadline.saturating_duration_since(Instant::now());
            let wait = if ready.is_some() || self.key_changed(key) {
                Duration::ZERO
            } else if self.pending.is_empty() {
                remaining
            } else if self.fence.is_some() {
                Duration::ZERO
            } else {
                remaining.min(POLL_INTERVAL)
            };
            self.acquire(wait)?;
            if self.key_changed(key) {
                return self.render_sync(key).map(Some);
            }
            if self.dirty {
                self.submit(key)?;
            }
            if ready.is_none() {
                ready = self.collect_ready(key)?;
            }
            if ready.is_none() {
                self.wait_for_gpu(deadline.saturating_duration_since(Instant::now()));
                ready = self.collect_ready(key)?;
            }
            if ready.is_some() {
                return Ok(ready);
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
        }
    }

    /// Sleeps until the newest in-flight render is finished, at most `timeout`.
    fn wait_for_gpu(&self, timeout: Duration) {
        let value = self.pending.back().and_then(|p| p.fence_value);
        if let (Some(fence), Some(value)) = (&self.fence, value) {
            fence.wait(value, timeout);
        }
    }

    /// A desktop image exists but the last render was made for something else.
    fn key_changed(&self, key: Key) -> bool {
        self.cache.is_some() && self.last_key != Some(key)
    }

    pub(crate) fn timings(&self) -> GrabTimings {
        self.timings
    }

    pub(crate) fn set_max_dimension(&mut self, max_dimension: u32) {
        self.max_dimension = max_dimension;
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
        let frame = self.render_sync(Key {
            region: NormRect::full(),
            max_dimension: dimension,
        })?;
        self.last_key = None;
        Ok(frame)
    }

    /// Time since the desktop image presented at `present_qpc` was presented.
    fn since_present(present_qpc: i64) -> Option<Duration> {
        if present_qpc == 0 {
            return None;
        }
        let (mut now, mut freq) = (0i64, 0i64);
        // SAFETY: out parameters refer to live locals.
        let ok = unsafe { QueryPerformanceCounter(&raw mut now) }.is_ok()
            && unsafe { QueryPerformanceFrequency(&raw mut freq) }.is_ok();
        if !ok || freq <= 0 || now < present_qpc {
            return None;
        }
        let ticks = u64::try_from(now - present_qpc).ok()?;
        let freq = u64::try_from(freq).ok()?;
        Some(Duration::from_secs_f64(ticks as f64 / freq as f64))
    }

    /// Waits for one duplication event. Returns true when a new desktop image was stored in the
    /// cache (and marks it as not yet rendered), false on timeout or when only the pointer
    /// changed.
    fn acquire(&mut self, wait: Duration) -> Result<bool, CaptureError> {
        // Rounded up: a sub-millisecond wait must not turn into a non-blocking poll.
        let millis = u32::try_from(wait.as_micros().div_ceil(1000)).unwrap_or(u32::MAX);
        let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut resource: Option<IDXGIResource> = None;
        let started = Instant::now();
        // SAFETY: out pointers refer to live locals.
        let acquired = unsafe {
            self.duplication
                .AcquireNextFrame(millis, &raw mut info, &raw mut resource)
        };
        self.timings.acquire += started.elapsed();
        if let Err(e) = acquired {
            return match classify(e.code().0) {
                Class::Timeout => Ok(false),
                _ => Err(map_err("AcquireNextFrame", &e)),
            };
        }

        let desktop_updated = info.LastPresentTime != 0 || info.AccumulatedFrames > 0;
        let store_started = Instant::now();
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
        self.timings.store += store_started.elapsed();
        stored?;
        released.map_err(|e| map_err("ReleaseFrame", &e))?;
        if desktop_updated {
            self.dirty = true;
            self.present_qpc = info.LastPresentTime;
        }
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

    fn ensure_work(&mut self, width: u32, height: u32, mips: u32) -> Result<(), CaptureError> {
        if self
            .work
            .as_ref()
            .is_some_and(|w| w.width == width && w.height == height && w.mips == mips)
        {
            return Ok(());
        }
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

    fn create_staging(&self, width: u32, height: u32) -> Result<Staging, CaptureError> {
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
        Ok(Staging {
            texture: self.create_texture(&desc)?,
            width,
            height,
        })
    }

    /// Staging texture of the requested size for ring slot `slot`, or for synchronous
    /// readbacks when `slot` is `None`. A ring slot must not be in flight.
    fn staging_for(
        &mut self,
        slot: Option<usize>,
        width: u32,
        height: u32,
    ) -> Result<ID3D11Texture2D, CaptureError> {
        let current = match slot {
            Some(index) => &self.ring[index].staging,
            None => &self.staging,
        };
        if let Some(staging) = current
            && staging.width == width
            && staging.height == height
        {
            return Ok(staging.texture.clone());
        }
        let fresh = self.create_staging(width, height)?;
        let texture = fresh.texture.clone();
        match slot {
            Some(index) => self.ring[index].staging = Some(fresh),
            None => self.staging = Some(fresh),
        }
        Ok(texture)
    }

    /// Records the downscale of `key.region` from the cached desktop image and a copy of the
    /// result into the staging texture selected by `slot`. Returns that texture and its size.
    fn record(
        &mut self,
        key: Key,
        slot: Option<usize>,
    ) -> Result<(ID3D11Texture2D, u32, u32), CaptureError> {
        let (rect, cache_texture) = match &self.cache {
            Some(cache) => (self.region_pixels(cache, key.region), cache.texture.clone()),
            None => return Err(CaptureError::Os("no desktop image cached".into())),
        };
        // Only the levels down to the delivered one are generated.
        let level = select_mip_level(
            rect.width,
            rect.height,
            key.max_dimension,
            mip_count(rect.width, rect.height),
        );
        self.ensure_work(rect.width, rect.height, level + 1)?;
        let Some(work) = &self.work else {
            return Err(CaptureError::Os("work texture missing".into()));
        };
        let (work_texture, work_view) = (work.texture.clone(), work.view.clone());
        let (out_w, out_h) = mip_size(rect.width, rect.height, level);
        let staging_texture = self.staging_for(slot, out_w, out_h)?;

        let source_box = D3D11_BOX {
            left: rect.x,
            top: rect.y,
            front: 0,
            right: rect.x + rect.width,
            bottom: rect.y + rect.height,
            back: 1,
        };
        let started = Instant::now();
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
        self.timings.submit += started.elapsed();
        Ok((staging_texture, out_w, out_h))
    }

    /// Renders `key` from the cached desktop image and waits for the result.
    fn render_sync(&mut self, key: Key) -> Result<Frame, CaptureError> {
        let (staging, width, height) = self.record(key, None)?;
        let frame = self.read_back(&staging, width, height, false)?;
        self.last_key = Some(key);
        self.dirty = false;
        self.timings.latency = Self::since_present(self.present_qpc);
        frame.ok_or_else(|| CaptureError::Os("blocking Map returned no data".into()))
    }

    /// Records a render of the cached desktop image into a free ring slot without waiting for
    /// it. Does nothing while every slot is in flight; the image then stays marked as unrendered.
    fn submit(&mut self, key: Key) -> Result<(), CaptureError> {
        let Some(slot) = self.ring.iter().position(|s| !s.busy) else {
            return Ok(());
        };
        let (_, width, height) = self.record(key, Some(slot))?;
        let fence_value = self.fence.as_mut().and_then(GpuSignal::signal);
        if fence_value.is_none() {
            self.fence = None;
        }
        // SAFETY: plain call on the live immediate context. It hands the recorded work to the
        // GPU so the non-blocking `Map` in `collect_ready` can observe its completion.
        unsafe { self.context.Flush() };
        self.ring[slot].busy = true;
        self.pending.push_back(Pending {
            slot,
            key,
            width,
            height,
            present_qpc: self.present_qpc,
            fence_value,
        });
        self.last_key = Some(key);
        self.dirty = false;
        Ok(())
    }

    /// Reads back every in-flight render the GPU has finished and returns the newest one that
    /// was made for `key`. Never waits.
    fn collect_ready(&mut self, key: Key) -> Result<Option<Frame>, CaptureError> {
        let mut newest = None;
        while let Some(pending) = self.pending.front() {
            let (slot, width, height) = (pending.slot, pending.width, pending.height);
            let Some(texture) = self.ring[slot].staging.as_ref().map(|s| s.texture.clone()) else {
                return Err(CaptureError::Os("staging texture missing".into()));
            };
            let Some(frame) = self.read_back(&texture, width, height, true)? else {
                break;
            };
            if let Some(done) = self.pending.pop_front() {
                self.ring[done.slot].busy = false;
                if done.key == key {
                    newest = Some((frame, done.present_qpc));
                }
            }
        }
        Ok(newest.map(|(frame, present_qpc)| {
            self.timings.latency = Self::since_present(present_qpc);
            frame
        }))
    }

    /// Copies the staging texture to a [`Frame`]. With `poll`, returns `Ok(None)` when the GPU
    /// has not finished writing it; otherwise waits.
    fn read_back(
        &mut self,
        staging: &ID3D11Texture2D,
        width: u32,
        height: u32,
        poll: bool,
    ) -> Result<Option<Frame>, CaptureError> {
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        let flags = if poll {
            D3D11_MAP_FLAG_DO_NOT_WAIT.0 as u32
        } else {
            0
        };
        let started = Instant::now();
        // SAFETY: the staging texture is CPU-readable and not mapped elsewhere.
        let mapped_ok = unsafe {
            self.context
                .Map(staging, 0, D3D11_MAP_READ, flags, Some(&raw mut mapped))
        };
        self.timings.map += started.elapsed();
        if let Err(e) = mapped_ok {
            return if poll && e.code() == DXGI_ERROR_WAS_STILL_DRAWING {
                Ok(None)
            } else {
                Err(map_err("Map", &e))
            };
        }

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
        Ok(Some(Frame {
            data,
            width,
            height,
            stride: row_bytes,
        }))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::win::find_monitor;

    fn max_channel_difference(a: &Frame, b: &Frame) -> u8 {
        assert_eq!((a.width, a.height), (b.width, b.height));
        a.data
            .iter()
            .zip(&b.data)
            .map(|(x, y)| x.abs_diff(*y))
            .max()
            .unwrap_or(0)
    }

    #[test]
    #[ignore = "needs an interactive desktop; assumes the screen is static for a moment"]
    fn asynchronous_readback_matches_synchronous_readback() {
        let entry = find_monitor(None).expect("primary monitor");
        let mut duplicator = Duplicator::new(&entry, 64).expect("duplicator");
        let region = NormRect::new(0.1, 0.2, 0.5, 0.6);
        let key = Key {
            region,
            max_dimension: 64,
        };
        let sync = duplicator
            .grab(region, Duration::from_secs(2))
            .expect("grab")
            .expect("first grab renders synchronously");
        duplicator.submit(key).expect("submit");
        assert_eq!(duplicator.pending.len(), 1);
        duplicator.wait_for_gpu(Duration::from_secs(1));
        let mut asynchronous = duplicator.collect_ready(key).expect("collect");
        for _ in 0..1000 {
            if asynchronous.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
            asynchronous = duplicator.collect_ready(key).expect("collect");
        }
        let asynchronous = asynchronous.expect("the GPU finished the render");
        assert!(duplicator.pending.is_empty());
        assert!(duplicator.ring.iter().all(|slot| !slot.busy));
        assert!(max_channel_difference(&sync, &asynchronous) <= 1);
    }

    #[test]
    #[ignore = "needs an interactive desktop"]
    fn frames_for_another_region_are_not_delivered_for_this_one() {
        let entry = find_monitor(None).expect("primary monitor");
        let mut duplicator = Duplicator::new(&entry, 32).expect("duplicator");
        let first = NormRect::new(0.0, 0.0, 0.5, 0.5);
        let second = NormRect::new(0.5, 0.5, 0.5, 0.5);
        duplicator
            .grab(first, Duration::from_secs(2))
            .expect("grab")
            .expect("frame");
        duplicator
            .submit(Key {
                region: first,
                max_dimension: 32,
            })
            .expect("submit");
        // The in-flight render belongs to `first`; asking for `second` renders synchronously.
        let frame = duplicator
            .grab(second, Duration::from_secs(2))
            .expect("grab");
        assert!(frame.is_some());
        assert_eq!(
            duplicator.last_key.map(|k| k.region),
            Some(second.clamped())
        );
    }
}
