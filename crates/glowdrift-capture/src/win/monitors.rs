//! Monitor enumeration: DXGI outputs joined with display-config friendly names and paths.

use std::collections::HashMap;
use std::mem::size_of;

use windows::Win32::Devices::Display::{
    DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
    DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE, DISPLAYCONFIG_PATH_INFO,
    DISPLAYCONFIG_SOURCE_DEVICE_NAME, DISPLAYCONFIG_TARGET_DEVICE_NAME, DisplayConfigGetDeviceInfo,
    GetDisplayConfigBufferSizes, QDC_ONLY_ACTIVE_PATHS, QueryDisplayConfig,
};
use windows::Win32::Foundation::LUID;
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIAdapter1, IDXGIFactory1, IDXGIOutput,
};
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, HMONITOR, MONITORINFO, MONITORINFOEXW};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};

use crate::math::stable_id;
use crate::{CaptureError, MonitorInfo};

use super::error::map_err;

const MONITORINFOF_PRIMARY: u32 = 1;
const ERROR_INSUFFICIENT_BUFFER: u32 = 122;

/// A monitor together with the DXGI objects needed to duplicate it.
pub(crate) struct MonitorEntry {
    pub info: MonitorInfo,
    pub adapter: IDXGIAdapter1,
    pub output: IDXGIOutput,
}

/// Details from the active display configuration, keyed by upper-cased GDI source name.
struct ConfigEntry {
    friendly_name: String,
    device_path: String,
    /// Position and size in physical virtual-desktop pixels.
    geometry: Option<(i32, i32, u32, u32)>,
}

pub(crate) fn wide_to_string(wide: &[u16]) -> String {
    let end = wide.iter().position(|&c| c == 0).unwrap_or(wide.len());
    String::from_utf16_lossy(&wide[..end])
}

fn luid_eq(a: LUID, b: LUID) -> bool {
    a.LowPart == b.LowPart && a.HighPart == b.HighPart
}

fn query_display_config() -> HashMap<String, ConfigEntry> {
    let mut result = HashMap::new();
    let mut paths: Vec<DISPLAYCONFIG_PATH_INFO> = Vec::new();
    let mut modes: Vec<DISPLAYCONFIG_MODE_INFO> = Vec::new();
    let mut ok = false;
    for _ in 0..3 {
        let (mut path_count, mut mode_count) = (0u32, 0u32);
        // SAFETY: both pointers refer to live u32 locals.
        let status = unsafe {
            GetDisplayConfigBufferSizes(
                QDC_ONLY_ACTIVE_PATHS,
                &raw mut path_count,
                &raw mut mode_count,
            )
        };
        if status.0 != 0 {
            return result;
        }
        paths = vec![DISPLAYCONFIG_PATH_INFO::default(); path_count as usize];
        modes = vec![DISPLAYCONFIG_MODE_INFO::default(); mode_count as usize];
        // SAFETY: the arrays hold `path_count` / `mode_count` elements and the counts are passed
        // by pointer so the call can shrink them to the number actually written.
        let status = unsafe {
            QueryDisplayConfig(
                QDC_ONLY_ACTIVE_PATHS,
                &raw mut path_count,
                paths.as_mut_ptr(),
                &raw mut mode_count,
                modes.as_mut_ptr(),
                None,
            )
        };
        if status.0 == 0 {
            paths.truncate(path_count as usize);
            modes.truncate(mode_count as usize);
            ok = true;
            break;
        }
        if status.0 != ERROR_INSUFFICIENT_BUFFER {
            return result;
        }
    }
    if !ok {
        return result;
    }

    for path in &paths {
        let mut source = DISPLAYCONFIG_SOURCE_DEVICE_NAME::default();
        source.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME;
        source.header.size = size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32;
        source.header.adapterId = path.sourceInfo.adapterId;
        source.header.id = path.sourceInfo.id;
        // SAFETY: `source` is a fully initialised request packet whose header carries its size.
        if unsafe { DisplayConfigGetDeviceInfo(&raw mut source.header) } != 0 {
            continue;
        }
        let gdi_name = wide_to_string(&source.viewGdiDeviceName).to_uppercase();
        if gdi_name.is_empty() || result.contains_key(&gdi_name) {
            continue;
        }

        let mut target = DISPLAYCONFIG_TARGET_DEVICE_NAME::default();
        target.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME;
        target.header.size = size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32;
        target.header.adapterId = path.targetInfo.adapterId;
        target.header.id = path.targetInfo.id;
        // SAFETY: as above for the target-name packet.
        let target_ok = unsafe { DisplayConfigGetDeviceInfo(&raw mut target.header) } == 0;

        let geometry = modes
            .iter()
            .find(|m| {
                m.infoType == DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE
                    && m.id == path.sourceInfo.id
                    && luid_eq(m.adapterId, path.sourceInfo.adapterId)
            })
            .map(|m| {
                // SAFETY: `infoType` is SOURCE, so the `sourceMode` union member is active.
                let s = unsafe { m.Anonymous.sourceMode };
                (s.position.x, s.position.y, s.width, s.height)
            });

        result.insert(
            gdi_name,
            ConfigEntry {
                friendly_name: if target_ok {
                    wide_to_string(&target.monitorFriendlyDeviceName)
                } else {
                    String::new()
                },
                device_path: if target_ok {
                    wide_to_string(&target.monitorDevicePath)
                } else {
                    String::new()
                },
                geometry,
            },
        );
    }
    result
}

fn scale_factor(monitor: HMONITOR) -> f32 {
    let (mut dpi_x, mut dpi_y) = (0u32, 0u32);
    // SAFETY: both pointers refer to live u32 locals; an invalid handle yields an error.
    match unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &raw mut dpi_x, &raw mut dpi_y) } {
        Ok(()) if dpi_x > 0 => dpi_x as f32 / 96.0,
        _ => 1.0,
    }
}

fn fallback_name(gdi_name: &str) -> String {
    let digits: String = gdi_name.chars().filter(char::is_ascii_digit).collect();
    if digits.is_empty() {
        "Display".to_owned()
    } else {
        format!("Display {digits}")
    }
}

/// Enumerates every output attached to the desktop. The primary monitor comes first, the rest
/// follow from left to right, then top to bottom.
pub(crate) fn enumerate() -> Result<Vec<MonitorEntry>, CaptureError> {
    // A fresh factory every time: an old one keeps reporting the topology it was created with.
    // SAFETY: plain COM factory creation.
    let factory: IDXGIFactory1 =
        unsafe { CreateDXGIFactory1() }.map_err(|e| map_err("CreateDXGIFactory1", &e))?;
    let config = query_display_config();

    let mut entries: Vec<MonitorEntry> = Vec::new();
    let mut seen: Vec<HMONITOR> = Vec::new();
    let mut adapter_index = 0;
    // SAFETY: enumeration calls on live COM objects; they fail with DXGI_ERROR_NOT_FOUND at the end.
    while let Ok(adapter) = unsafe { factory.EnumAdapters1(adapter_index) } {
        adapter_index += 1;
        let mut output_index = 0;
        // SAFETY: as above.
        while let Ok(output) = unsafe { adapter.EnumOutputs(output_index) } {
            output_index += 1;
            // SAFETY: `output` is a live COM object.
            let Ok(desc) = (unsafe { output.GetDesc() }) else {
                continue;
            };
            if !desc.AttachedToDesktop.as_bool() || seen.contains(&desc.Monitor) {
                continue;
            }
            seen.push(desc.Monitor);

            let mut mi = MONITORINFOEXW::default();
            mi.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
            // SAFETY: `cbSize` announces the EX layout, so the pointer cast to MONITORINFO is valid
            // for the whole structure.
            let have_info = unsafe {
                GetMonitorInfoW(
                    desc.Monitor,
                    std::ptr::from_mut(&mut mi).cast::<MONITORINFO>(),
                )
            }
            .as_bool();
            let primary = have_info && mi.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0;

            let gdi_name = wide_to_string(&desc.DeviceName);
            let cfg = config.get(&gdi_name.to_uppercase());
            let rect = desc.DesktopCoordinates;
            let (x, y, width, height) = cfg.and_then(|c| c.geometry).unwrap_or((
                rect.left,
                rect.top,
                (rect.right - rect.left).max(0) as u32,
                (rect.bottom - rect.top).max(0) as u32,
            ));

            let id_source = cfg
                .map(|c| c.device_path.as_str())
                .filter(|p| !p.is_empty())
                .unwrap_or(&gdi_name);
            let name = cfg
                .map(|c| c.friendly_name.trim())
                .filter(|n| !n.is_empty())
                .map_or_else(|| fallback_name(&gdi_name), str::to_owned);

            entries.push(MonitorEntry {
                info: MonitorInfo {
                    id: stable_id(id_source),
                    name,
                    x,
                    y,
                    width,
                    height,
                    primary,
                    scale_factor: scale_factor(desc.Monitor),
                },
                adapter: adapter.clone(),
                output,
            });
        }
    }

    entries.sort_by_key(|e| (!e.info.primary, e.info.x, e.info.y));
    Ok(entries)
}

/// Finds a monitor by id; `None` selects the primary monitor.
pub(crate) fn find(monitor_id: Option<&str>) -> Result<MonitorEntry, CaptureError> {
    let mut entries = enumerate()?;
    let index = match monitor_id {
        Some(id) => entries.iter().position(|e| e.info.id == id),
        // Enumeration sorts the primary first, so index 0 is the fallback when none is flagged.
        None => (!entries.is_empty()).then_some(0),
    };
    index
        .map(|i| entries.swap_remove(i))
        .ok_or(CaptureError::MonitorNotFound)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_strings_stop_at_nul() {
        let mut buf = [0u16; 8];
        for (i, c) in "ab".encode_utf16().enumerate() {
            buf[i] = c;
        }
        assert_eq!(wide_to_string(&buf), "ab");
        assert_eq!(wide_to_string(&[]), "");
        assert_eq!(wide_to_string(&[104, 105]), "hi");
    }

    #[test]
    fn fallback_name_uses_display_number() {
        assert_eq!(fallback_name("\\\\.\\DISPLAY3"), "Display 3");
        assert_eq!(fallback_name("weird"), "Display");
    }
}
