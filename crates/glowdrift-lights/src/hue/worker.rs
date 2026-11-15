//! Worker-side state machine of the Hue light.
//!
//! A [`Worker`] is driven by explicit inputs (`apply`, `step`) and an explicit clock. `step`
//! performs at most one network action (a connect or one PUT) and returns when it wants to be
//! called again, so shutdown and new colours are noticed between requests. All I/O happens on
//! the worker thread.

use std::time::{Duration, Instant};

use glowdrift_core::Rgb;
use serde_json::{Value, json};

use super::api::{Inventory, LightInfo};
use super::client::{Bridge, REQUEST_TIMEOUT};
use super::color::{self, Mapped};
use super::pacer::{PUT_GAP, Pacer, Poll, round_interval};
use super::{BridgeCredentials, HueError};
use crate::{LightCode, LightState};

/// Timing knobs, overridable in tests.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Timing {
    /// Minimum gap between two light PUTs.
    pub put_gap: Duration,
    pub backoff_initial: Duration,
    pub backoff_max: Duration,
}

impl Default for Timing {
    fn default() -> Timing {
        Timing {
            put_gap: PUT_GAP,
            backoff_initial: Duration::from_secs(1),
            backoff_max: Duration::from_secs(30),
        }
    }
}

/// Published connection status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Status {
    pub state: LightState,
    pub detail: Option<String>,
    pub device_count: u32,
    pub code: Option<LightCode>,
}

impl Status {
    pub fn new(state: LightState, detail: Option<&str>) -> Status {
        Status {
            state,
            detail: detail.map(str::to_owned),
            device_count: 0,
            code: None,
        }
    }

    fn coded(state: LightState, detail: &str, code: LightCode) -> Status {
        Status {
            code: Some(code),
            ..Status::new(state, Some(detail))
        }
    }
}

/// Which bridge and area to drive.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Target {
    pub creds: Option<BridgeCredentials>,
    pub area_id: Option<String>,
}

/// Inputs queued for the worker since its last step.
#[derive(Debug, Default)]
pub(crate) struct Inbox {
    pub color: Option<Rgb>,
    pub target: Option<Target>,
    pub enabled: Option<bool>,
    pub retry: bool,
    pub shutdown: bool,
}

impl Inbox {
    pub fn has_pending(&self) -> bool {
        self.color.is_some()
            || self.target.is_some()
            || self.enabled.is_some()
            || self.retry
            || self.shutdown
    }
}

/// The output for one light, quantised so near-identical colours compare equal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Command {
    Off,
    On {
        /// `xy` in ten-thousandths.
        xy: Option<(i32, i32)>,
        /// Brightness in tenths of a percent.
        brightness: Option<i32>,
    },
}

/// Brightness below which a light that cannot dim is switched off.
const ON_OFF_THRESHOLD: f64 = 10.0;

pub(crate) fn command_for(light: &LightInfo, rgb: Rgb) -> Command {
    let mapped = match &light.gamut {
        Some(gamut) => color::map_color(rgb.r, rgb.g, rgb.b, gamut),
        None => color::map_brightness(rgb.r, rgb.g, rgb.b),
    };
    match mapped {
        Mapped::Off => Command::Off,
        Mapped::On { xy, brightness } => {
            if !light.dimmable && brightness < ON_OFF_THRESHOLD {
                return Command::Off;
            }
            Command::On {
                xy: light
                    .gamut
                    .is_some()
                    .then(|| ((xy.x * 1e4).round() as i32, (xy.y * 1e4).round() as i32)),
                brightness: light.dimmable.then(|| (brightness * 10.0).round() as i32),
            }
        }
    }
}

/// The PUT body for a command; `duration` is the transition time.
pub(crate) fn body_for(command: Command, duration: Duration) -> Value {
    let dynamics = json!({ "duration": u64::try_from(duration.as_millis()).unwrap_or(u64::MAX) });
    match command {
        Command::Off => json!({ "on": { "on": false }, "dynamics": dynamics }),
        Command::On { xy, brightness } => {
            let mut body = json!({ "on": { "on": true }, "dynamics": dynamics });
            if let Some(tenths) = brightness {
                body["dimming"] = json!({ "brightness": f64::from(tenths) / 10.0 });
            }
            if let Some((x, y)) = xy {
                body["color"] =
                    json!({ "xy": { "x": f64::from(x) / 1e4, "y": f64::from(y) / 1e4 } });
            }
            body
        }
    }
}

#[derive(Debug)]
struct Backoff {
    initial: Duration,
    max: Duration,
    current: Duration,
}

impl Backoff {
    fn new(initial: Duration, max: Duration) -> Backoff {
        Backoff {
            initial,
            max,
            current: initial,
        }
    }

    fn reset(&mut self) {
        self.current = self.initial;
    }

    fn next_delay(&mut self) -> Duration {
        let delay = self.current;
        self.current = (self.current * 2).min(self.max);
        delay
    }
}

struct Session {
    bridge: Bridge,
    lights: Vec<LightInfo>,
    /// The command each light was last given successfully.
    last: Vec<Option<Command>>,
    pacer: Pacer,
}

/// Problems that persist until the user changes something.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sticky {
    Unauthorized,
    CertificateMismatch,
    Invalid,
}

pub(crate) struct Worker {
    timing: Timing,
    enabled: bool,
    target: Target,
    desired: Option<Rgb>,
    session: Option<Session>,
    sticky: Option<Sticky>,
    retry_at: Option<Instant>,
    backoff: Backoff,
    /// Set once the current outage has been logged, so a long outage logs once.
    outage_logged: bool,
    /// Last light-level failure that was logged; cleared by a successful PUT.
    put_failure_logged: Option<String>,
    status: Status,
}

impl std::fmt::Debug for Worker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Worker")
            .field("enabled", &self.enabled)
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}

impl Worker {
    pub fn new(timing: Timing) -> Worker {
        Worker {
            timing,
            enabled: false,
            target: Target::default(),
            desired: None,
            session: None,
            sticky: None,
            retry_at: None,
            backoff: Backoff::new(timing.backoff_initial, timing.backoff_max),
            outage_logged: false,
            put_failure_logged: None,
            status: Status::new(LightState::Disabled, None),
        }
    }

    pub fn status(&self) -> &Status {
        &self.status
    }

    /// Applies queued inputs. Returns `true` on shutdown.
    pub fn apply(&mut self, inbox: Inbox) -> bool {
        if inbox.shutdown {
            self.session = None;
            self.status = Status::new(LightState::Disabled, None);
            return true;
        }
        if let Some(enabled) = inbox.enabled
            && enabled != self.enabled
        {
            self.enabled = enabled;
            self.reset_connection();
        }
        if let Some(target) = inbox.target
            && target != self.target
        {
            self.target = target;
            self.reset_connection();
        }
        if inbox.retry {
            self.reset_connection();
        }
        if let Some(color) = inbox.color {
            self.desired = Some(color);
            if let Some(session) = &mut self.session {
                session.pacer.touch();
            }
        }
        false
    }

    fn reset_connection(&mut self) {
        self.session = None;
        self.sticky = None;
        self.retry_at = None;
        self.backoff.reset();
        self.outage_logged = false;
        self.put_failure_logged = None;
    }

    /// Performs at most one network action; returns when to step again, `None` to sleep until
    /// the next input.
    pub fn step(&mut self, now: Instant) -> Option<Instant> {
        if !self.enabled {
            self.session = None;
            self.status = Status::new(LightState::Disabled, None);
            return None;
        }
        if self.target.creds.is_none() {
            self.session = None;
            self.status = Status::coded(
                LightState::Unavailable,
                "Not paired with a Hue bridge",
                LightCode::NotPaired,
            );
            return None;
        }
        if self.target.area_id.is_none() {
            self.session = None;
            self.status = Status::coded(
                LightState::Unavailable,
                "Choose a Hue area",
                LightCode::NoArea,
            );
            return None;
        }
        if self.session.is_none() {
            if self.sticky.is_some() {
                return None;
            }
            if let Some(at) = self.retry_at
                && at > now
            {
                return Some(at);
            }
            return self.connect(now);
        }
        self.send_next(now)
    }

    fn connect(&mut self, now: Instant) -> Option<Instant> {
        let (Some(creds), Some(area)) = (self.target.creds.clone(), self.target.area_id.clone())
        else {
            return None;
        };
        match open_session(&creds, &area, self.timing.put_gap) {
            Ok(mut session) => {
                if self.outage_logged {
                    log::info!("Hue bridge reachable again");
                }
                self.outage_logged = false;
                if self.desired.is_none() {
                    self.backoff.reset();
                } else {
                    session.pacer.touch();
                }
                self.status = Status {
                    device_count: u32::try_from(session.lights.len()).unwrap_or(u32::MAX),
                    ..Status::new(LightState::Connected, Some("Connected"))
                };
                self.session = Some(session);
                Some(now)
            }
            Err(err) => {
                self.on_connect_error(&err, now);
                self.retry_at
            }
        }
    }

    fn on_connect_error(&mut self, err: &HueError, now: Instant) {
        match err {
            HueError::Unauthorized => {
                self.fail_sticky(Sticky::Unauthorized, err);
            }
            HueError::CertificateMismatch => {
                self.fail_sticky(Sticky::CertificateMismatch, err);
            }
            HueError::InvalidAddress(_) | HueError::InvalidCredentials(_) => {
                self.fail_sticky(Sticky::Invalid, err);
            }
            HueError::AreaNotFound => {
                self.status = Status::coded(
                    LightState::Unavailable,
                    "The selected Hue area was not found or has no lights",
                    LightCode::NoArea,
                );
                self.schedule_retry(now);
                self.log_outage(err);
            }
            HueError::Network(_) => {
                self.status = Status::new(
                    LightState::Unavailable,
                    Some("The Hue bridge is not reachable"),
                );
                self.schedule_retry(now);
                self.log_outage(err);
            }
            HueError::Bridge(_) | HueError::Parse(_) => {
                self.status = Status::new(LightState::Error, Some(&err.to_string()));
                self.schedule_retry(now);
                self.log_outage(err);
            }
        }
    }

    fn fail_sticky(&mut self, kind: Sticky, err: &HueError) {
        self.sticky = Some(kind);
        self.session = None;
        self.retry_at = None;
        let detail = match kind {
            Sticky::Unauthorized => {
                "The Hue bridge rejected the stored app key; pair with the bridge again"
            }
            Sticky::CertificateMismatch => {
                "The Hue bridge's certificate changed since pairing; pair with the bridge again"
            }
            Sticky::Invalid => "The stored Hue bridge details are invalid; pair again",
        };
        self.status = Status::new(LightState::Error, Some(detail));
        self.log_outage(err);
    }

    fn schedule_retry(&mut self, now: Instant) {
        self.session = None;
        self.retry_at = Some(now + self.backoff.next_delay());
    }

    fn log_outage(&mut self, err: &HueError) {
        if !self.outage_logged {
            self.outage_logged = true;
            log::warn!("Hue light unavailable: {err}");
        }
    }

    fn send_next(&mut self, now: Instant) -> Option<Instant> {
        let desired = self.desired?;
        let session = self.session.as_mut()?;
        let index = match session.pacer.poll(now) {
            Poll::Idle => return None,
            Poll::WaitUntil(at) => return Some(at),
            Poll::Send(index) => index,
        };
        let command = command_for(&session.lights[index], desired);
        if session.last[index] == Some(command) {
            session.pacer.skipped(index);
            return Some(now);
        }
        let duration = round_interval(self.timing.put_gap, session.lights.len());
        let body = body_for(command, duration);
        let id = session.lights[index].id.clone();
        session.pacer.attempted(index, now);
        let result = session.bridge.put_v2("light", &id, &body);
        match result {
            Ok(()) => {
                session.last[index] = Some(command);
                self.backoff.reset();
                self.put_failure_logged = None;
                self.outage_logged = false;
            }
            Err(HueError::Network(_)) => {
                let err = result.err()?;
                self.on_connect_error(&err, Instant::now());
                return self.retry_at;
            }
            Err(err @ (HueError::Unauthorized | HueError::CertificateMismatch)) => {
                self.on_connect_error(&err, now);
                return None;
            }
            Err(err) => {
                session.last[index] = None;
                let text = err.to_string();
                if self.put_failure_logged.as_deref() != Some(text.as_str()) {
                    log::warn!("Hue light {id} rejected a colour: {text}");
                    self.put_failure_logged = Some(text);
                }
            }
        }
        Some(Instant::now())
    }
}

fn open_session(
    creds: &BridgeCredentials,
    area_id: &str,
    put_gap: Duration,
) -> Result<Session, HueError> {
    if creds.cert_sha256.is_empty() || creds.app_key.is_empty() {
        return Err(HueError::InvalidCredentials(
            "missing app key or certificate pin".to_owned(),
        ));
    }
    let bridge = Bridge::new(
        &creds.address,
        Some(&creds.cert_sha256),
        Some(&creds.app_key),
        REQUEST_TIMEOUT,
    )?;
    let lights = Inventory::fetch(&bridge)?
        .area_lights(area_id)
        .filter(|lights| !lights.is_empty())
        .ok_or(HueError::AreaNotFound)?;
    let mut pacer = Pacer::new(put_gap);
    pacer.set_lights(lights.len());
    Ok(Session {
        bridge,
        last: vec![None; lights.len()],
        lights,
        pacer,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hue::color::Gamut;

    fn colour_light() -> LightInfo {
        LightInfo {
            id: "l".to_owned(),
            gamut: Some(Gamut::C),
            dimmable: true,
        }
    }

    #[test]
    fn colour_light_gets_xy_and_brightness() {
        let command = command_for(&colour_light(), Rgb::new(255, 255, 255));
        let Command::On { xy, brightness } = command else {
            panic!("white must be on");
        };
        assert_eq!(brightness, Some(1000));
        let (x, y) = xy.unwrap();
        assert!((x - 3127).abs() <= 2 && (y - 3290).abs() <= 2, "{x},{y}");
    }

    #[test]
    fn near_identical_colours_are_the_same_command() {
        let a = command_for(&colour_light(), Rgb::new(200, 100, 50));
        let b = command_for(&colour_light(), Rgb::new(200, 100, 50));
        assert_eq!(a, b);
        assert_ne!(a, command_for(&colour_light(), Rgb::new(100, 200, 50)));
    }

    #[test]
    fn black_is_off() {
        assert_eq!(command_for(&colour_light(), Rgb::BLACK), Command::Off);
    }

    #[test]
    fn dimmable_white_bulb_gets_brightness_only() {
        let light = LightInfo {
            id: "w".to_owned(),
            gamut: None,
            dimmable: true,
        };
        let Command::On { xy, brightness } = command_for(&light, Rgb::new(128, 0, 0)) else {
            panic!()
        };
        assert_eq!(xy, None);
        assert_eq!(brightness, Some(502));
    }

    #[test]
    fn plain_switch_is_on_or_off() {
        let light = LightInfo {
            id: "p".to_owned(),
            gamut: None,
            dimmable: false,
        };
        assert_eq!(command_for(&light, Rgb::new(5, 5, 5)), Command::Off);
        assert_eq!(
            command_for(&light, Rgb::new(200, 0, 0)),
            Command::On {
                xy: None,
                brightness: None
            }
        );
    }

    #[test]
    fn put_bodies() {
        let on = body_for(
            Command::On {
                xy: Some((3127, 3290)),
                brightness: Some(755),
            },
            Duration::from_millis(300),
        );
        assert_eq!(
            on,
            json!({
                "on": {"on": true},
                "dimming": {"brightness": 75.5},
                "color": {"xy": {"x": 0.3127, "y": 0.329}},
                "dynamics": {"duration": 300}
            })
        );
        assert_eq!(
            body_for(Command::Off, Duration::from_millis(100)),
            json!({"on": {"on": false}, "dynamics": {"duration": 100}})
        );
    }

    #[test]
    fn backoff_doubles_to_the_cap_and_resets() {
        let mut backoff = Backoff::new(Duration::from_secs(1), Duration::from_secs(30));
        let delays: Vec<u64> = (0..7).map(|_| backoff.next_delay().as_secs()).collect();
        assert_eq!(delays, vec![1, 2, 4, 8, 16, 30, 30]);
        backoff.reset();
        assert_eq!(backoff.next_delay(), Duration::from_secs(1));
    }

    #[test]
    fn unpaired_and_arealess_states() {
        let mut worker = Worker::new(Timing::default());
        let now = Instant::now();
        assert!(!worker.apply(Inbox {
            enabled: Some(true),
            ..Inbox::default()
        }));
        assert_eq!(worker.step(now), None);
        assert_eq!(worker.status().code, Some(LightCode::NotPaired));
        assert_eq!(worker.status().state, LightState::Unavailable);

        let creds = BridgeCredentials {
            bridge_id: "B".to_owned(),
            address: "127.0.0.1:1".to_owned(),
            app_key: "k".to_owned(),
            client_key: "c".to_owned(),
            cert_sha256: "00".to_owned(),
        };
        worker.apply(Inbox {
            target: Some(Target {
                creds: Some(creds),
                area_id: None,
            }),
            ..Inbox::default()
        });
        assert_eq!(worker.step(now), None);
        assert_eq!(worker.status().code, Some(LightCode::NoArea));
    }

    #[test]
    fn disabled_worker_reports_disabled() {
        let mut worker = Worker::new(Timing::default());
        worker.apply(Inbox {
            enabled: Some(false),
            ..Inbox::default()
        });
        assert_eq!(worker.step(Instant::now()), None);
        assert_eq!(worker.status().state, LightState::Disabled);
    }

    #[test]
    fn shutdown_is_reported() {
        let mut worker = Worker::new(Timing::default());
        assert!(worker.apply(Inbox {
            shutdown: true,
            ..Inbox::default()
        }));
    }
}
