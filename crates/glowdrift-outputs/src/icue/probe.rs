//! One-shot diagnostics: connect, read session details and devices, disconnect.

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use glowdrift_core::Rgb;

use super::controller::session_status;
use super::ffi::FfiSdk;
use super::sdk::{EventSink, Sdk, SdkError, SdkEvent, SessionState, device_type_name};
use crate::OutputError;

/// A lighting device found by [`probe`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeDevice {
    pub id: String,
    pub model: String,
    /// Device kind, e.g. `"keyboard"`.
    pub kind: String,
    pub led_count: usize,
    /// Hardware colour of the first LED at the time of the probe.
    pub first_led_color: Option<Rgb>,
}

/// Result of [`probe`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeReport {
    /// Session states in the order the SDK reported them.
    pub session_states: Vec<String>,
    pub sdk_client_version: String,
    pub sdk_server_version: String,
    pub icue_version: String,
    pub devices: Vec<ProbeDevice>,
}

/// Connects to iCUE with the same library lookup as [`super::IcueOutput`] (plus `dll_dirs`),
/// waits up to `timeout` for the session to be established, reads the devices and disconnects.
pub fn probe(dll_dirs: Vec<PathBuf>, timeout: Duration) -> Result<ProbeReport, OutputError> {
    probe_with(FfiSdk::new(super::library_dirs(dll_dirs)), timeout)
}

pub(crate) fn probe_with<S: Sdk>(
    mut sdk: S,
    timeout: Duration,
) -> Result<ProbeReport, OutputError> {
    let (tx, rx) = mpsc::channel();
    let sink: EventSink = std::sync::Arc::new(move |event| {
        let _ = tx.send(event);
    });
    sdk.connect(sink).map_err(to_output_error)?;
    let result = collect_report(&mut sdk, &rx, timeout);
    sdk.disconnect();
    result
}

fn collect_report<S: Sdk>(
    sdk: &mut S,
    events: &mpsc::Receiver<SdkEvent>,
    timeout: Duration,
) -> Result<ProbeReport, OutputError> {
    let deadline = Instant::now() + timeout;
    let mut states = Vec::new();
    let mut last_state = SessionState::Closed;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match events.recv_timeout(remaining) {
            Ok(SdkEvent::Session { state, .. }) => {
                states.push(state.to_string());
                last_state = state;
                if state == SessionState::Connected {
                    break;
                }
            }
            Ok(SdkEvent::DevicesChanged) => {}
            Err(_) => {
                let (_, text) = session_status(last_state);
                return Err(OutputError::Unavailable(format!(
                    "{text} (session states seen: {})",
                    if states.is_empty() {
                        "none".to_owned()
                    } else {
                        states.join(", ")
                    }
                )));
            }
        }
    }

    let details = sdk.session_details().map_err(to_output_error)?;
    let mut devices = Vec::new();
    for info in sdk.devices().map_err(to_output_error)? {
        let led_ids = sdk.led_ids(&info.id).map_err(to_output_error)?;
        let first_led_color = led_ids
            .first()
            .and_then(|id| sdk.led_colors(&info.id, &[*id]).ok())
            .and_then(|colors| colors.first().copied());
        devices.push(ProbeDevice {
            id: info.id,
            model: info.model,
            kind: device_type_name(info.device_type).to_owned(),
            led_count: led_ids.len(),
            first_led_color,
        });
    }
    Ok(ProbeReport {
        session_states: states,
        sdk_client_version: details.client.to_string(),
        sdk_server_version: details.server.to_string(),
        icue_version: details.host.to_string(),
        devices,
    })
}

fn to_output_error(err: SdkError) -> OutputError {
    match err {
        SdkError::LibraryMissing { .. } => OutputError::Unavailable(err.to_string()),
        other => OutputError::Failed(other.to_string()),
    }
}
