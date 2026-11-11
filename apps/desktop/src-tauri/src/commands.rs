//! IPC commands and their payload types. Command names are `snake_case`; payloads are `camelCase`.

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use base64::Engine as _;
use glowdrift_capture::{Frame, MonitorInfo};
use glowdrift_core::Config;
use glowdrift_outputs::OutputStatus;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};

use crate::config_store::ConfigStore;
pub use crate::engine::EngineStatus;
use crate::engine::{EngineHandle, describe_capture_error};
use crate::outputs_manager::OutputsManager;
use crate::updater::{self, PendingUpdate};

/// Accepted range of `maxWidth` for thumbnails, in pixels.
const THUMBNAIL_WIDTH_RANGE: std::ops::RangeInclusive<u32> = 16..=4096;

/// Whole-monitor preview image.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Thumbnail {
    pub width: u32,
    pub height: u32,
    /// `data:image/png;base64,...`
    pub data_url: String,
}

/// Application identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub version: String,
    pub name: String,
}

/// Payload of the `engine://color` event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ColorEvent {
    pub raw: String,
    pub smoothed: String,
}

/// Event names emitted by the shell.
pub mod events {
    pub const COLOR: &str = "engine://color";
    pub const ENGINE_STATUS: &str = "engine://status";
    pub const OUTPUTS_STATUS: &str = "outputs://status";
    pub const UPDATER_READY: &str = "updater://ready";
}

/// Shared application state.
pub struct AppState {
    pub config: Mutex<Config>,
    pub store: ConfigStore,
    pub engine: EngineHandle,
    pub outputs: Arc<OutputsManager>,
    pub pending_update: Mutex<Option<PendingUpdate>>,
    /// True until the main window has been shown for the first time (and when it must be).
    pub show_on_load: AtomicBool,
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState").finish_non_exhaustive()
    }
}

impl AppState {
    /// Stops the engine (releasing outputs). Safe to call more than once.
    pub fn shutdown(&self) {
        self.engine.shutdown();
        self.outputs.shutdown();
    }

    pub fn toggle_running(&self) {
        let running = self.engine.status().running;
        self.engine.set_running(!running);
    }

    pub fn auto_update_enabled(&self) -> bool {
        lock(&self.config).auto_update
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[tauri::command]
pub fn get_config(state: State<'_, AppState>) -> Config {
    lock(&state.config).clone()
}

#[tauri::command]
pub async fn set_config(config: Config, state: State<'_, AppState>) -> Result<Config, String> {
    let config = config.validated();
    let mut stored = lock(&state.config);
    *stored = config.clone();
    state.engine.apply_config(config.clone());
    state
        .store
        .save(&config)
        .map_err(|e| format!("Settings were applied but could not be saved: {e}"))?;
    Ok(config)
}

#[tauri::command]
pub async fn list_monitors() -> Result<Vec<MonitorInfo>, String> {
    glowdrift_capture::list_monitors().map_err(|e| describe_capture_error(&e))
}

#[tauri::command]
pub async fn get_monitor_thumbnail(
    monitor_id: Option<String>,
    max_width: u32,
) -> Result<Thumbnail, String> {
    if !THUMBNAIL_WIDTH_RANGE.contains(&max_width) {
        return Err(format!(
            "maxWidth must be between {} and {} pixels.",
            THUMBNAIL_WIDTH_RANGE.start(),
            THUMBNAIL_WIDTH_RANGE.end()
        ));
    }
    let monitor_id = monitor_id.filter(|id| !id.is_empty());
    tauri::async_runtime::spawn_blocking(move || {
        let frame = glowdrift_capture::capture_thumbnail(monitor_id.as_deref(), max_width)
            .map_err(|e| describe_capture_error(&e))?;
        let data_url = png_data_url(&frame)?;
        Ok(Thumbnail {
            width: frame.width,
            height: frame.height,
            data_url,
        })
    })
    .await
    .map_err(|e| format!("Thumbnail task failed: {e}"))?
}

/// Encodes a BGRA8 frame as a PNG data URL.
fn png_data_url(frame: &Frame) -> Result<String, String> {
    let (width, height) = (frame.width as usize, frame.height as usize);
    let row_bytes = width * 4;
    let needed = height.saturating_sub(1) * frame.stride + row_bytes;
    if width == 0 || height == 0 || frame.stride < row_bytes || frame.data.len() < needed {
        return Err("The captured image was invalid.".to_owned());
    }
    let mut rgb = Vec::with_capacity(width * height * 3);
    for y in 0..height {
        let row = &frame.data[y * frame.stride..y * frame.stride + row_bytes];
        for px in row.as_chunks::<4>().0 {
            rgb.extend_from_slice(&[px[2], px[1], px[0]]);
        }
    }
    let png_bytes = encode_rgb_png(frame.width, frame.height, &rgb)
        .map_err(|e| format!("Could not encode the preview image: {e}"))?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(&png_bytes);
    Ok(format!("data:image/png;base64,{encoded}"))
}

fn encode_rgb_png(width: u32, height: u32, rgb: &[u8]) -> Result<Vec<u8>, png::EncodingError> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, width, height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::Fast);
    encoder.write_header()?.write_image_data(rgb)?;
    Ok(out)
}

#[tauri::command]
pub fn get_engine_status(state: State<'_, AppState>) -> EngineStatus {
    state.engine.status()
}

#[tauri::command]
pub fn set_engine_running(running: bool, state: State<'_, AppState>) -> EngineStatus {
    state.engine.set_running(running);
    state.engine.status()
}

#[tauri::command]
pub fn list_outputs(state: State<'_, AppState>) -> Vec<OutputStatus> {
    state.outputs.statuses()
}

#[tauri::command]
pub fn get_app_info(app: AppHandle) -> AppInfo {
    let info = app.package_info();
    AppInfo {
        version: info.version.to_string(),
        name: info.name.clone(),
    }
}

/// Installs an update that was downloaded in the background. Returns false when none is
/// pending. On success the process exits and the installer restarts the app.
#[tauri::command]
pub async fn install_pending_update(app: AppHandle) -> Result<bool, String> {
    tauri::async_runtime::spawn_blocking(move || updater::install_pending(&app))
        .await
        .map_err(|e| format!("Update task failed: {e}"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(width: u32, height: u32, stride: usize, data: Vec<u8>) -> Frame {
        Frame {
            data,
            width,
            height,
            stride,
        }
    }

    #[test]
    fn png_data_url_has_png_prefix_and_signature() {
        let data = vec![
            // row 0: blue, green (BGRA)
            255, 0, 0, 255, 0, 255, 0, 255, // row 1: red, white
            0, 0, 255, 255, 255, 255, 255, 255,
        ];
        let url = png_data_url(&frame(2, 2, 8, data)).unwrap();
        let b64 = url.strip_prefix("data:image/png;base64,").unwrap();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .unwrap();
        assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
    }

    #[test]
    fn png_data_url_respects_stride_padding() {
        let mut data = vec![0_u8; 12 * 2];
        data[0..4].copy_from_slice(&[255, 0, 0, 255]);
        let url = png_data_url(&frame(1, 2, 12, data)).unwrap();
        assert!(url.starts_with("data:image/png;base64,"));
    }

    #[test]
    fn png_data_url_rejects_short_or_empty_buffers() {
        assert!(png_data_url(&frame(2, 2, 8, vec![0; 8])).is_err());
        assert!(png_data_url(&frame(0, 0, 0, vec![])).is_err());
        assert!(png_data_url(&frame(2, 1, 4, vec![0; 8])).is_err());
    }
}
