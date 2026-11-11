//! Platform-independent view of the iCUE SDK used by the worker logic.
//!
//! The worker only talks to the SDK through the [`Sdk`] trait, so its behaviour can be tested
//! with a fake.

use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use glowdrift_core::Rgb;

/// Session state reported by `CorsairConnect`'s callback (`CorsairSessionState`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    Invalid,
    /// Not connected (initial state, or the client closed the connection).
    Closed,
    Connecting,
    /// iCUE did not answer; the SDK keeps retrying.
    Timeout,
    /// iCUE refused: third-party control is disabled in iCUE's settings.
    ConnectionRefused,
    /// iCUE closed the connection (restart, or the Windows session was locked).
    ConnectionLost,
    Connected,
}

impl SessionState {
    /// Maps the raw `CorsairSessionState` value.
    pub fn from_raw(raw: i32) -> SessionState {
        match raw {
            1 => SessionState::Closed,
            2 => SessionState::Connecting,
            3 => SessionState::Timeout,
            4 => SessionState::ConnectionRefused,
            5 => SessionState::ConnectionLost,
            6 => SessionState::Connected,
            _ => SessionState::Invalid,
        }
    }
}

impl fmt::Display for SessionState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            SessionState::Invalid => "Invalid",
            SessionState::Closed => "Closed",
            SessionState::Connecting => "Connecting",
            SessionState::Timeout => "Timeout",
            SessionState::ConnectionRefused => "ConnectionRefused",
            SessionState::ConnectionLost => "ConnectionLost",
            SessionState::Connected => "Connected",
        };
        f.write_str(name)
    }
}

/// A `major.minor.patch` version (`CorsairVersion`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Version {
    pub major: i32,
    pub minor: i32,
    pub patch: i32,
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// SDK client, SDK server and iCUE versions (`CorsairSessionDetails`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SessionDetails {
    pub client: Version,
    pub server: Version,
    pub host: Version,
}

/// Asynchronous notification from the SDK. Delivered on an SDK-owned thread, so the sink must
/// only enqueue and never call back into the SDK.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SdkEvent {
    Session {
        state: SessionState,
        details: SessionDetails,
    },
    /// A device was connected or disconnected.
    DevicesChanged,
}

/// Receives [`SdkEvent`]s.
pub type EventSink = Arc<dyn Fn(SdkEvent) + Send + Sync>;

/// A lighting device as reported by `CorsairGetDevices`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    pub id: String,
    pub model: String,
    /// `CorsairDeviceType` value.
    pub device_type: u32,
    pub led_count: u32,
}

/// Name of a single `CorsairDeviceType` bit.
pub fn device_type_name(device_type: u32) -> &'static str {
    match device_type {
        0x0001 => "keyboard",
        0x0002 => "mouse",
        0x0004 => "mousemat",
        0x0008 => "headset",
        0x0010 => "headset stand",
        0x0020 => "fan LED controller",
        0x0040 => "LED controller",
        0x0080 => "memory module",
        0x0100 => "cooler",
        0x0200 => "motherboard",
        0x0400 => "graphics card",
        0x0800 => "touchbar",
        0x1000 => "game controller",
        _ => "unknown",
    }
}

/// `CorsairError` codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    NotConnected,
    NoControl,
    IncompatibleProtocol,
    InvalidArguments,
    InvalidOperation,
    DeviceNotFound,
    NotAllowed,
    Unknown(i32),
}

impl ErrorCode {
    /// Maps a non-zero `CorsairError` value.
    pub fn from_raw(raw: i32) -> ErrorCode {
        match raw {
            1 => ErrorCode::NotConnected,
            2 => ErrorCode::NoControl,
            3 => ErrorCode::IncompatibleProtocol,
            4 => ErrorCode::InvalidArguments,
            5 => ErrorCode::InvalidOperation,
            6 => ErrorCode::DeviceNotFound,
            7 => ErrorCode::NotAllowed,
            other => ErrorCode::Unknown(other),
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ErrorCode::NotConnected => f.write_str("CE_NotConnected"),
            ErrorCode::NoControl => f.write_str("CE_NoControl"),
            ErrorCode::IncompatibleProtocol => f.write_str("CE_IncompatibleProtocol"),
            ErrorCode::InvalidArguments => f.write_str("CE_InvalidArguments"),
            ErrorCode::InvalidOperation => f.write_str("CE_InvalidOperation"),
            ErrorCode::DeviceNotFound => f.write_str("CE_DeviceNotFound"),
            ErrorCode::NotAllowed => f.write_str("CE_NotAllowed"),
            ErrorCode::Unknown(v) => write!(f, "CorsairError({v})"),
        }
    }
}

/// Failure of an SDK operation.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SdkError {
    /// The SDK DLL is not in any of the permitted directories.
    #[error("iCUE SDK library not found (looked for {file} in {} location(s))", searched.len())]
    LibraryMissing {
        file: &'static str,
        searched: Vec<PathBuf>,
    },
    /// The DLL exists but could not be loaded or lacks an expected export.
    #[error("could not load the iCUE SDK library: {0}")]
    LibraryLoad(String),
    /// An SDK function returned an error code.
    #[error("{function} failed: {code}")]
    Call {
        function: &'static str,
        code: ErrorCode,
    },
}

impl SdkError {
    /// The SDK error code, if this error came from an SDK call.
    pub fn code(&self) -> Option<ErrorCode> {
        match self {
            SdkError::Call { code, .. } => Some(*code),
            _ => None,
        }
    }
}

/// The subset of the iCUE SDK the worker relies on. Blocking; called only from the worker
/// thread.
pub trait Sdk: Send {
    /// Starts a session. State changes arrive through `sink`. The SDK retries on its own while
    /// iCUE is unreachable.
    fn connect(&mut self, sink: EventSink) -> Result<(), SdkError>;
    /// Ends the session and stops event delivery. Safe to call when not connected.
    fn disconnect(&mut self);
    fn session_details(&mut self) -> Result<SessionDetails, SdkError>;
    fn set_layer_priority(&mut self, priority: u32) -> Result<(), SdkError>;
    /// Subscribes to device connection events (delivered as [`SdkEvent::DevicesChanged`]).
    fn subscribe_events(&mut self) -> Result<(), SdkError>;
    fn devices(&mut self) -> Result<Vec<DeviceInfo>, SdkError>;
    /// Unique LED identifiers (`CorsairLedLuid`) of a device.
    fn led_ids(&mut self, device: &str) -> Result<Vec<u32>, SdkError>;
    /// Sets the listed LEDs of a device to one opaque colour.
    fn set_leds_color(&mut self, device: &str, led_ids: &[u32], color: Rgb)
    -> Result<(), SdkError>;
    /// Current hardware colour of the listed LEDs.
    fn led_colors(&mut self, device: &str, led_ids: &[u32]) -> Result<Vec<Rgb>, SdkError>;
    /// Releases any control requested for a device (resets access to shared).
    fn release_control(&mut self, device: &str);
}
