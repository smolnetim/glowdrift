//! Worker-side state machine for the iCUE output.
//!
//! A [`Controller`] is driven by explicit inputs (`set_enabled`, `set_color`, `on_event`,
//! `tick`) and an explicit clock, so connection handling, de-duplication and rate limiting are
//! testable without threads, time or iCUE.

use std::time::{Duration, Instant};

use glowdrift_core::Rgb;

use super::ffi::DLL_NAME;
use super::sdk::{ErrorCode, EventSink, Sdk, SdkError, SdkEvent, SessionState};
use crate::OutputState;

/// Minimum gap between colour writes (about 30 Hz).
pub(crate) const MIN_SEND_INTERVAL: Duration = Duration::from_millis(33);
/// Device-change events are coalesced for this long before devices are re-enumerated.
pub(crate) const ENUMERATE_DEBOUNCE: Duration = Duration::from_millis(250);
/// Wait before retrying after a failed write.
pub(crate) const SEND_RETRY: Duration = Duration::from_secs(1);
const BACKOFF_INITIAL: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(30);

/// Layer priority requested for this client. iCUE's own profile lighting sits at 127 and
/// shared clients default to 128, so the ambient colour is drawn above the user's profile while
/// staying well below the maximum of 255, which leaves headroom for clients that must win.
pub(crate) const LAYER_PRIORITY: u32 = 128;

/// Published connection status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Status {
    pub state: OutputState,
    pub detail: Option<String>,
    pub device_count: u32,
}

impl Status {
    fn new(state: OutputState, detail: Option<&str>) -> Status {
        Status {
            state,
            detail: detail.map(str::to_owned),
            device_count: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Fault {
    LibraryMissing,
    Error(String),
    /// Another client holds exclusive lighting control.
    Contended,
}

#[derive(Debug)]
struct Device {
    id: String,
    led_ids: Vec<u32>,
}

#[derive(Debug)]
struct Backoff {
    current: Duration,
}

impl Backoff {
    fn new() -> Backoff {
        Backoff {
            current: BACKOFF_INITIAL,
        }
    }

    fn reset(&mut self) {
        self.current = BACKOFF_INITIAL;
    }

    fn next_delay(&mut self) -> Duration {
        let delay = self.current;
        self.current = (self.current * 2).min(BACKOFF_MAX);
        delay
    }
}

#[allow(clippy::struct_excessive_bools)]
pub(crate) struct Controller<S: Sdk> {
    sdk: S,
    sink: EventSink,
    enabled: bool,
    stopped: bool,
    sdk_connected: bool,
    session: SessionState,
    fault: Option<Fault>,
    retry_at: Option<Instant>,
    backoff: Backoff,
    devices: Vec<Device>,
    enumerated: bool,
    enumerate_at: Option<Instant>,
    desired: Option<Rgb>,
    sent: Option<Rgb>,
    last_send: Option<Instant>,
    send_retry_at: Option<Instant>,
    status: Status,
}

impl<S: Sdk> std::fmt::Debug for Controller<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Controller")
            .field("enabled", &self.enabled)
            .field("session", &self.session)
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}

/// Output state and user-facing text for a session state other than `Connected`.
pub(crate) fn session_status(state: SessionState) -> (OutputState, &'static str) {
    match state {
        SessionState::Invalid | SessionState::Closed | SessionState::Connecting => {
            (OutputState::Connecting, "Connecting to iCUE")
        }
        SessionState::ConnectionLost => (OutputState::Connecting, "Reconnecting to iCUE"),
        SessionState::Timeout => (OutputState::Unavailable, "iCUE is not running"),
        SessionState::ConnectionRefused => (
            OutputState::Unavailable,
            "iCUE refused the connection: enable the iCUE SDK in iCUE settings (SDK tab)",
        ),
        SessionState::Connected => (OutputState::Connected, "Connected"),
    }
}

fn earliest(a: Option<Instant>, b: Option<Instant>) -> Option<Instant> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.min(y)),
        (x, None) => x,
        (None, y) => y,
    }
}

impl<S: Sdk> Controller<S> {
    /// Creates a disabled controller; call [`Controller::set_enabled`] to start connecting.
    pub fn new(sdk: S, sink: EventSink) -> Controller<S> {
        let mut controller = Controller {
            sdk,
            sink,
            enabled: false,
            stopped: false,
            sdk_connected: false,
            session: SessionState::Closed,
            fault: None,
            retry_at: None,
            backoff: Backoff::new(),
            devices: Vec::new(),
            enumerated: false,
            enumerate_at: None,
            desired: None,
            sent: None,
            last_send: None,
            send_retry_at: None,
            status: Status::new(OutputState::Disabled, None),
        };
        controller.refresh_status();
        controller
    }

    pub fn status(&self) -> &Status {
        &self.status
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        if self.stopped || enabled == self.enabled {
            return;
        }
        self.enabled = enabled;
        if enabled {
            self.retry_at = None;
            self.backoff.reset();
        } else {
            self.teardown();
        }
        self.refresh_status();
    }

    /// Records the latest requested colour. Writing happens in [`Controller::tick`].
    pub fn set_color(&mut self, color: Rgb) {
        self.desired = Some(color);
    }

    pub fn on_event(&mut self, event: SdkEvent, now: Instant) {
        if !self.sdk_connected {
            return;
        }
        match event {
            SdkEvent::Session { state, .. } => {
                self.session = state;
                if state == SessionState::Connected {
                    self.on_connected(now);
                } else {
                    self.drop_devices();
                }
            }
            SdkEvent::DevicesChanged => {
                if self.session == SessionState::Connected {
                    self.enumerate_at.get_or_insert(now + ENUMERATE_DEBOUNCE);
                }
            }
        }
        self.refresh_status();
    }

    /// Performs all work that is due at `now` and returns when `tick` should run next, or
    /// `None` when nothing is scheduled.
    pub fn tick(&mut self, now: Instant) -> Option<Instant> {
        if !self.enabled || self.stopped {
            return None;
        }
        if !self.sdk_connected {
            if let Some(at) = self.retry_at.filter(|at| *at > now) {
                return Some(at);
            }
            self.try_connect(now);
            self.refresh_status();
            if !self.sdk_connected {
                return self.retry_at;
            }
        }

        let mut next = None;
        if self.session == SessionState::Connected {
            if let Some(at) = self.enumerate_at
                && at <= now
            {
                self.enumerate_at = None;
                self.enumerate(now);
            }
            if self.send_due(now).is_some_and(|due| due <= now) {
                self.send(now);
            }
            next = earliest(self.enumerate_at, self.send_due(now));
        }
        self.refresh_status();
        next
    }

    /// Releases control, disconnects and permanently stops the controller.
    pub fn shutdown(&mut self) {
        self.teardown();
        self.stopped = true;
        self.refresh_status();
    }

    fn try_connect(&mut self, now: Instant) {
        match self.sdk.connect(self.sink.clone()) {
            Ok(()) => {
                self.sdk_connected = true;
                self.session = SessionState::Connecting;
                self.fault = None;
                self.retry_at = None;
                self.backoff.reset();
            }
            Err(err) => {
                log::warn!("iCUE connect failed: {err}");
                self.fault = Some(match err {
                    SdkError::LibraryMissing { .. } => Fault::LibraryMissing,
                    other => Fault::Error(other.to_string()),
                });
                self.retry_at = Some(now + self.backoff.next_delay());
            }
        }
    }

    fn on_connected(&mut self, now: Instant) {
        if let Err(err) = self.sdk.set_layer_priority(LAYER_PRIORITY) {
            log::warn!("iCUE layer priority not applied: {err}");
        }
        if let Err(err) = self.sdk.subscribe_events() {
            log::warn!("iCUE device events unavailable: {err}");
        }
        self.drop_devices();
        self.enumerate_at = Some(now);
    }

    fn drop_devices(&mut self) {
        self.devices.clear();
        self.enumerated = false;
        self.enumerate_at = None;
        self.sent = None;
        self.send_retry_at = None;
    }

    fn teardown(&mut self) {
        if self.sdk_connected {
            for device in &self.devices {
                self.sdk.release_control(&device.id);
            }
            self.sdk.disconnect();
        }
        self.sdk_connected = false;
        self.session = SessionState::Closed;
        self.fault = None;
        self.retry_at = None;
        self.drop_devices();
    }

    fn enumerate(&mut self, now: Instant) {
        let list = match self.sdk.devices() {
            Ok(list) => list,
            Err(err) => {
                self.enumeration_failed(&err, now);
                return;
            }
        };
        let mut devices = Vec::with_capacity(list.len());
        for info in list.into_iter().filter(|d| d.led_count > 0) {
            match self.sdk.led_ids(&info.id) {
                Ok(led_ids) if !led_ids.is_empty() => devices.push(Device {
                    id: info.id,
                    led_ids,
                }),
                Ok(_) => {}
                Err(err) if err.code() == Some(ErrorCode::DeviceNotFound) => {
                    log::debug!("iCUE device {} vanished during enumeration", info.model);
                }
                Err(err) => {
                    self.enumeration_failed(&err, now);
                    return;
                }
            }
        }
        self.devices = devices;
        self.enumerated = true;
        self.sent = None;
        self.send_retry_at = None;
        self.backoff.reset();
        if matches!(self.fault, Some(Fault::Error(_))) {
            self.fault = None;
        }
    }

    fn enumeration_failed(&mut self, err: &SdkError, now: Instant) {
        if err.code() == Some(ErrorCode::NotConnected) {
            // A session-state event follows and restarts enumeration.
            self.enumerated = false;
            return;
        }
        log::warn!("iCUE device enumeration failed: {err}");
        self.fault = Some(Fault::Error(err.to_string()));
        self.enumerate_at = Some(now + self.backoff.next_delay());
    }

    /// When the next colour write should happen (`now` when it is due immediately), or `None`
    /// when nothing needs writing.
    fn send_due(&self, now: Instant) -> Option<Instant> {
        let wanted = self.desired?;
        if self.session != SessionState::Connected
            || !self.enumerated
            || self.devices.is_empty()
            || self.sent == Some(wanted)
        {
            return None;
        }
        let rate_limited = self.last_send.map(|at| at + MIN_SEND_INTERVAL);
        Some(match (rate_limited, self.send_retry_at) {
            (Some(a), Some(b)) => a.max(b),
            (Some(a), None) => a,
            (None, Some(b)) => b,
            (None, None) => now,
        })
    }

    fn send(&mut self, now: Instant) {
        let Some(color) = self.desired else { return };
        let mut lost = false;
        let mut stale = false;
        let mut contended = false;
        let mut failure = None;
        for device in &self.devices {
            match self.sdk.set_leds_color(&device.id, &device.led_ids, color) {
                Ok(()) => {}
                Err(err) => match err.code() {
                    Some(ErrorCode::NotConnected) => lost = true,
                    Some(ErrorCode::DeviceNotFound) => stale = true,
                    Some(ErrorCode::NoControl) => contended = true,
                    _ => failure = Some(err.to_string()),
                },
            }
        }
        self.last_send = Some(now);

        if lost {
            // The SDK announces the new session state; devices are re-read once connected.
            self.session = SessionState::ConnectionLost;
            self.drop_devices();
            return;
        }
        if contended {
            self.fault = Some(Fault::Contended);
            self.send_retry_at = Some(now + SEND_RETRY);
            return;
        }
        if let Some(message) = failure {
            log::warn!("iCUE write failed: {message}");
            self.fault = Some(Fault::Error(message));
            self.send_retry_at = Some(now + SEND_RETRY);
            return;
        }
        if stale {
            self.enumerate_at.get_or_insert(now + ENUMERATE_DEBOUNCE);
        }
        self.sent = Some(color);
        self.send_retry_at = None;
        if matches!(self.fault, Some(Fault::Contended | Fault::Error(_))) {
            self.fault = None;
        }
    }

    fn refresh_status(&mut self) {
        self.status = self.compute_status();
    }

    fn compute_status(&self) -> Status {
        if !self.enabled {
            return Status::new(OutputState::Disabled, None);
        }
        if self.stopped {
            return Status::new(OutputState::Unavailable, Some("Stopped"));
        }
        match &self.fault {
            Some(Fault::LibraryMissing) => {
                return Status::new(
                    OutputState::Unavailable,
                    Some(&format!("iCUE SDK library ({DLL_NAME}) not found")),
                );
            }
            Some(Fault::Error(message)) => {
                return Status::new(OutputState::Error, Some(message));
            }
            Some(Fault::Contended) => {
                return Status::new(
                    OutputState::Error,
                    Some("Another application has exclusive control of the lighting"),
                );
            }
            None => {}
        }
        if self.session == SessionState::Connected {
            return if self.enumerated {
                self.connected_status()
            } else {
                Status::new(OutputState::Connecting, Some("Reading iCUE devices"))
            };
        }
        let (state, detail) = session_status(self.session);
        Status::new(state, Some(detail))
    }

    fn connected_status(&self) -> Status {
        let leds: usize = self.devices.iter().map(|d| d.led_ids.len()).sum();
        let count = self.devices.len();
        let detail = if count == 0 {
            "Connected, but iCUE reports no lighting devices".to_owned()
        } else {
            format!(
                "{count} {}, {leds} {}",
                if count == 1 { "device" } else { "devices" },
                if leds == 1 { "LED" } else { "LEDs" },
            )
        };
        Status {
            state: OutputState::Connected,
            detail: Some(detail),
            device_count: u32::try_from(count).unwrap_or(u32::MAX),
        }
    }
}
