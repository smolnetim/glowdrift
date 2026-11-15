//! Worker-side state machine of the Hue light.
//!
//! A [`Worker`] is driven by explicit inputs (`apply`, `step`) and an explicit clock. `step`
//! performs at most one network action (a connect, a state read or one PUT) and returns when it
//! wants to be called again, so shutdown and new colours are noticed between requests. All I/O
//! happens on the worker thread, except that stopping a send session restores the lights it
//! changed before returning.
//!
//! # Send sessions
//!
//! A send session starts when a light is first driven after the light was enabled or its
//! target changed. Just before the first PUT to a light its state is read and kept. The
//! states are PUT back when the light is disabled, the bridge is forgotten, the target becomes
//! empty or the worker shuts down, and for lights that leave the target when the target or the
//! extra lights change. A session survives connection losses and reconnects, so the kept
//! states are never overwritten with the app's own output, and each state is restored once.
//!
//! # Refreshing
//!
//! A refresh request carries a ticket. While a session is open the worker re-reads the bridge
//! inventory over it, at most once per [`Timing::refresh_gap`], and keeps the connection and
//! the kept states; lights that left the target are restored. Without a session the next
//! connect reads the inventory anyway. The newest ticket handled is reported by
//! [`Worker::refreshed`]; tickets that arrive while the gap has not passed wait for it
//! together.

use std::time::{Duration, Instant};

use glowdrift_core::{LightPosition, Rgb};
use serde_json::{Value, json};

use super::api::{Inventory, LightInfo, LightSnapshot, LightStateReply};
use super::client::{Bridge, REQUEST_TIMEOUT};
use super::color::{self, Mapped};
use super::light::ZoneSource;
use super::pacer::{PUT_GAP, Pacer, Poll, round_interval};
use super::{BridgeCredentials, HueError};
use crate::{LightCode, LightState};

/// Transition time of the PUT that restores a light.
const RESTORE_FADE: Duration = Duration::from_millis(400);
/// Request timeout of the connection used for restoring.
const RESTORE_TIMEOUT: Duration = Duration::from_secs(2);
/// Time after which a restore stops starting new PUTs when the light is stopped.
const STOP_RESTORE_BUDGET: Duration = Duration::from_secs(5);
/// Time after which a restore stops starting new PUTs when the worker shuts down. With the
/// request timeout this stays inside the wait of `HueLight::shutdown`.
const SHUTDOWN_RESTORE_BUDGET: Duration = Duration::from_secs(2);

/// Timing knobs, overridable in tests.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Timing {
    /// Minimum gap between two light PUTs.
    pub put_gap: Duration,
    pub backoff_initial: Duration,
    pub backoff_max: Duration,
    /// Minimum gap between two reads of the bridge inventory.
    pub refresh_gap: Duration,
}

impl Default for Timing {
    fn default() -> Timing {
        Timing {
            put_gap: PUT_GAP,
            backoff_initial: Duration::from_secs(1),
            backoff_max: Duration::from_secs(30),
            refresh_gap: Duration::from_secs(2),
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

/// The colours the light was last asked to show.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ColorUpdate {
    /// One colour for every light.
    Whole(Rgb),
    /// One colour per light, in the order of the lights published for `epoch`. Applies only
    /// while the worker's epoch is the same; otherwise `whole` is shown on every light.
    PerLight {
        epoch: u64,
        whole: Rgb,
        zones: Vec<Rgb>,
    },
}

/// Inputs queued for the worker since its last step.
#[derive(Debug, Default)]
pub(crate) struct Inbox {
    pub color: Option<ColorUpdate>,
    pub target: Option<Target>,
    pub extras: Option<Vec<String>>,
    pub enabled: Option<bool>,
    pub retry: bool,
    /// Newest refresh ticket.
    pub refresh: Option<u64>,
    pub shutdown: bool,
}

impl Inbox {
    pub fn has_pending(&self) -> bool {
        self.color.is_some()
            || self.target.is_some()
            || self.extras.is_some()
            || self.enabled.is_some()
            || self.retry
            || self.refresh.is_some()
            || self.shutdown
    }
}

/// A light the worker drives, as published to the handle.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TargetInfo {
    pub id: String,
    pub name: String,
    pub position: Option<LightPosition>,
    pub source: ZoneSource,
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

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// The PUT body for a command; `duration` is the transition time.
pub(crate) fn body_for(command: Command, duration: Duration) -> Value {
    let dynamics = json!({ "duration": millis(duration) });
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

/// The PUT body that puts a light back into `snapshot`: its on/off state, brightness, and
/// either its colour temperature (when it was in that mode) or its colour.
pub(crate) fn restore_body(snapshot: &LightSnapshot, duration: Duration) -> Value {
    let mut body = json!({
        "on": { "on": snapshot.on },
        "dynamics": { "duration": millis(duration) },
    });
    if let Some(brightness) = snapshot.brightness {
        body["dimming"] = json!({ "brightness": brightness });
    }
    if let Some(mirek) = snapshot.mirek {
        body["color_temperature"] = json!({ "mirek": mirek });
    } else if let Some((x, y)) = snapshot.xy {
        body["color"] = json!({ "xy": { "x": x, "y": y } });
    }
    body
}

/// The colour the light at `index` of the target should show.
fn color_for(colors: Option<&ColorUpdate>, epoch: u64, index: usize) -> Option<Rgb> {
    match colors? {
        ColorUpdate::Whole(color) => Some(*color),
        ColorUpdate::PerLight {
            epoch: for_epoch,
            whole,
            zones,
        } => Some(if *for_epoch == epoch {
            zones.get(index).copied().unwrap_or(*whole)
        } else {
            *whole
        }),
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

/// Whether the state of a light was read in the current send session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SnapState {
    Unknown,
    Taken,
    /// Reading failed; the light is driven but cannot be restored.
    Failed,
}

struct Member {
    info: LightInfo,
    source: ZoneSource,
    position: Option<LightPosition>,
    /// The colour this light should show.
    want: Option<Rgb>,
    /// The command this light was last given successfully.
    last: Option<Command>,
    snap: SnapState,
}

impl Member {
    fn target_info(&self) -> TargetInfo {
        TargetInfo {
            id: self.info.id.clone(),
            name: self.info.name.clone(),
            position: self.position,
            source: self.source,
        }
    }
}

struct Session {
    creds: BridgeCredentials,
    bridge: Bridge,
    /// Entertainment lights in channel order, then extra lights in the order given.
    members: Vec<Member>,
    pacer: Pacer,
}

/// The states of the lights changed in the current send session.
struct Held {
    creds: BridgeCredentials,
    snapshots: Vec<(String, LightSnapshot)>,
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
    extras: Vec<String>,
    colors: Option<ColorUpdate>,
    session: Option<Session>,
    /// States to restore when the send session ends.
    held: Option<Held>,
    /// The lights of the last successful resolve; `None` while the target is empty or changed.
    resolved: Option<Vec<TargetInfo>>,
    /// Counts changes of the order of the ids in `resolved`, which decide what a per-light
    /// colour list addresses.
    epoch: u64,
    /// Counts changes of `resolved`.
    revision: u64,
    /// Newest refresh ticket not yet handled.
    refresh_pending: Option<u64>,
    /// Newest refresh ticket handled.
    refresh_done: u64,
    /// When the pending refresh may run, if the gap has not passed.
    refresh_wait: Option<Instant>,
    /// When the inventory was last read.
    last_inventory: Option<Instant>,
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
            extras: Vec::new(),
            colors: None,
            session: None,
            held: None,
            resolved: None,
            epoch: 0,
            revision: 0,
            refresh_pending: None,
            refresh_done: 0,
            refresh_wait: None,
            last_inventory: None,
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

    /// The lights driven by the last successful resolve with two counters: the epoch changes
    /// when the lights or their order do, the revision whenever anything about them does.
    pub fn resolved(&self) -> (u64, u64, Option<&[TargetInfo]>) {
        (self.epoch, self.revision, self.resolved.as_deref())
    }

    /// The newest refresh ticket that has been handled.
    pub fn refreshed(&self) -> u64 {
        self.refresh_done
    }

    fn set_resolved(&mut self, resolved: Option<Vec<TargetInfo>>) {
        if resolved == self.resolved {
            return;
        }
        let same_order = match (&self.resolved, &resolved) {
            (Some(old), Some(new)) => {
                old.len() == new.len() && old.iter().zip(new).all(|(a, b)| a.id == b.id)
            }
            _ => false,
        };
        if !same_order {
            self.epoch += 1;
        }
        self.revision += 1;
        self.resolved = resolved;
    }

    /// Applies queued inputs. Returns `true` on shutdown.
    pub fn apply(&mut self, inbox: Inbox) -> bool {
        if inbox.shutdown {
            self.session = None;
            self.end_send_session(SHUTDOWN_RESTORE_BUDGET);
            self.status = Status::new(LightState::Disabled, None);
            self.refresh_done = self
                .refresh_done
                .max(inbox.refresh.or(self.refresh_pending).unwrap_or(0));
            self.refresh_pending = None;
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
            if self
                .held
                .as_ref()
                .is_some_and(|h| Some(&h.creds) != target.creds.as_ref())
            {
                self.session = None;
                self.end_send_session(STOP_RESTORE_BUDGET);
            }
            self.target = target;
            self.set_resolved(None);
            self.reset_connection();
        }
        if let Some(extras) = inbox.extras
            && extras != self.extras
        {
            self.extras = extras;
            self.reset_connection();
        }
        if inbox.retry && self.session.is_none() {
            self.reset_connection();
        }
        if let Some(ticket) = inbox.refresh {
            self.refresh_pending = Some(self.refresh_pending.map_or(ticket, |t| t.max(ticket)));
        }
        if let Some(color) = inbox.color {
            self.colors = Some(color);
            self.refresh_wants();
        }
        false
    }

    /// Recomputes what each light of the session should show and marks changed lights stale.
    fn refresh_wants(&mut self) {
        let Some(session) = &mut self.session else {
            return;
        };
        for (index, member) in session.members.iter_mut().enumerate() {
            let want = color_for(self.colors.as_ref(), self.epoch, index);
            if member.want != want {
                member.want = want;
                session.pacer.touch_one(index);
            }
        }
    }

    fn reset_connection(&mut self) {
        self.session = None;
        self.sticky = None;
        self.retry_at = None;
        self.backoff.reset();
        self.outage_logged = false;
        self.put_failure_logged = None;
    }

    /// Puts the lights changed in the send session back and ends the session. Lights that
    /// cannot be reached are logged and skipped; the states are dropped either way.
    fn end_send_session(&mut self, budget: Duration) {
        let Some(held) = self.held.take() else {
            return;
        };
        let bridge = Bridge::new(
            &held.creds.address,
            Some(&held.creds.cert_sha256),
            Some(&held.creds.app_key),
            RESTORE_TIMEOUT,
        );
        match bridge {
            Ok(bridge) => restore_lights(&bridge, held.snapshots, self.timing.put_gap, budget),
            Err(err) => log::warn!("could not restore the Hue lights: {err}"),
        }
    }

    /// Performs at most one network action; returns when to step again, `None` to sleep until
    /// the next input.
    pub fn step(&mut self, now: Instant) -> Option<Instant> {
        self.refresh_wait = None;
        let next = self.step_inner(now);
        if let Some(at) = self.refresh_wait {
            return Some(next.map_or(at, |next| next.min(at)));
        }
        if let Some(ticket) = self.refresh_pending.take() {
            self.refresh_done = self.refresh_done.max(ticket);
        }
        next
    }

    fn step_inner(&mut self, now: Instant) -> Option<Instant> {
        if !self.enabled {
            self.session = None;
            self.end_send_session(STOP_RESTORE_BUDGET);
            self.status = Status::new(LightState::Disabled, None);
            return None;
        }
        if self.target.creds.is_none() {
            self.session = None;
            self.end_send_session(STOP_RESTORE_BUDGET);
            self.status = Status::coded(
                LightState::Unavailable,
                "Not paired with a Hue bridge",
                LightCode::NotPaired,
            );
            return None;
        }
        if self.target.area_id.is_none() && self.extras.is_empty() {
            self.session = None;
            self.end_send_session(STOP_RESTORE_BUDGET);
            self.set_resolved(None);
            self.status = Status::coded(
                LightState::Unavailable,
                "Choose a Hue area or add a light",
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
        if self.refresh_pending.is_some() {
            match self.last_inventory {
                Some(last) if last + self.timing.refresh_gap > now => {
                    self.refresh_wait = Some(last + self.timing.refresh_gap);
                }
                _ => return self.refresh_inventory(now),
            }
        }
        self.send_next(now)
    }

    fn connect(&mut self, now: Instant) -> Option<Instant> {
        let creds = self.target.creds.clone()?;
        let area = self.target.area_id.clone();
        self.last_inventory = Some(now);
        match open_session(&creds, area.as_deref(), &self.extras, self.timing.put_gap) {
            Ok(mut session) => {
                if self.outage_logged {
                    log::info!("Hue bridge reachable again");
                }
                self.outage_logged = false;
                self.adopt(&mut session);
                if self.colors.is_none() {
                    self.backoff.reset();
                }
                self.status = Status {
                    device_count: u32::try_from(session.members.len()).unwrap_or(u32::MAX),
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

    /// Joins a freshly opened session to the send session in progress: publishes its lights,
    /// restores the held lights that are no longer driven and gives the lights their colours.
    fn adopt(&mut self, session: &mut Session) {
        self.set_resolved(Some(
            session.members.iter().map(Member::target_info).collect(),
        ));
        if self
            .held
            .as_ref()
            .is_some_and(|held| held.creds != session.creds)
        {
            self.end_send_session(STOP_RESTORE_BUDGET);
        }
        self.restore_departed(&session.bridge, &session.members);
        if let Some(held) = &self.held {
            for member in &mut session.members {
                if held.snapshots.iter().any(|(id, _)| *id == member.info.id) {
                    member.snap = SnapState::Taken;
                }
            }
        }
        for (index, member) in session.members.iter_mut().enumerate() {
            member.want = color_for(self.colors.as_ref(), self.epoch, index);
            if member.want.is_some() {
                session.pacer.touch_one(index);
            }
        }
    }

    /// Puts back the held lights that are not among `members` and stops holding them.
    fn restore_departed(&mut self, bridge: &Bridge, members: &[Member]) {
        let Some(held) = &mut self.held else {
            return;
        };
        let (kept, departed): (Vec<_>, Vec<_>) = std::mem::take(&mut held.snapshots)
            .into_iter()
            .partition(|(id, _)| members.iter().any(|m| m.info.id == *id));
        held.snapshots = kept;
        if !departed.is_empty() {
            restore_lights(bridge, departed, self.timing.put_gap, STOP_RESTORE_BUDGET);
        }
    }

    /// Reads the inventory over the open session and applies it: lights, names, positions and
    /// gamuts change in place, lights that left the target are restored and the connection,
    /// the kept states and the colours already sent stay.
    fn refresh_inventory(&mut self, now: Instant) -> Option<Instant> {
        self.last_inventory = Some(now);
        let mut session = self.session.take()?;
        let inventory = match Inventory::fetch(&session.bridge) {
            Ok(inventory) => inventory,
            Err(
                err @ (HueError::Network(_)
                | HueError::Unauthorized
                | HueError::CertificateMismatch),
            ) => return self.connection_failed(&err, now),
            Err(err) => {
                log::warn!("could not refresh the Hue lights: {err}");
                self.session = Some(session);
                return Some(now);
            }
        };
        let mut members = resolve_members(
            &inventory,
            self.target.area_id.as_deref(),
            &self.extras,
            false,
        );
        if members.is_empty() {
            self.on_connect_error(&HueError::AreaNotFound, now);
            return self.retry_at;
        }
        for member in &mut members {
            if let Some(old) = session.members.iter().find(|o| o.info.id == member.info.id) {
                member.snap = old.snap;
                member.want = old.want;
                if old.info == member.info {
                    member.last = old.last;
                }
            }
        }
        self.restore_departed(&session.bridge, &members);
        let same_order = members.len() == session.members.len()
            && members
                .iter()
                .zip(&session.members)
                .all(|(a, b)| a.info.id == b.info.id);
        session.members = members;
        if !same_order {
            session.pacer.set_lights(session.members.len());
        }
        self.set_resolved(Some(
            session.members.iter().map(Member::target_info).collect(),
        ));
        self.status.device_count = u32::try_from(session.members.len()).unwrap_or(u32::MAX);
        self.session = Some(session);
        self.refresh_wants();
        Some(now)
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
                    "The selected Hue area or lights were not found",
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

    /// Handles a failure of a request that says something about the whole connection.
    fn connection_failed(&mut self, err: &HueError, now: Instant) -> Option<Instant> {
        if matches!(err, HueError::Network(_)) {
            self.on_connect_error(err, Instant::now());
            self.retry_at
        } else {
            self.on_connect_error(err, now);
            None
        }
    }

    fn send_next(&mut self, now: Instant) -> Option<Instant> {
        let session = self.session.as_mut()?;
        let index = match session.pacer.poll(now) {
            Poll::Idle => return None,
            Poll::WaitUntil(at) => return Some(at),
            Poll::Send(index) => index,
        };
        let count = session.members.len();
        let member = &mut session.members[index];
        let Some(want) = member.want else {
            session.pacer.skipped(index);
            return Some(now);
        };
        let command = command_for(&member.info, want);
        if member.last == Some(command) {
            session.pacer.skipped(index);
            return Some(now);
        }
        let id = member.info.id.clone();
        if member.snap == SnapState::Unknown {
            match read_snapshot(&session.bridge, &id) {
                Ok(snapshot) => {
                    member.snap = SnapState::Taken;
                    self.held
                        .get_or_insert_with(|| Held {
                            creds: session.creds.clone(),
                            snapshots: Vec::new(),
                        })
                        .snapshots
                        .push((id.clone(), snapshot));
                }
                Err(
                    err @ (HueError::Network(_)
                    | HueError::Unauthorized
                    | HueError::CertificateMismatch),
                ) => return self.connection_failed(&err, now),
                Err(err) => {
                    member.snap = SnapState::Failed;
                    log::warn!("could not read the state of Hue light {id}: {err}");
                }
            }
        }
        let duration = round_interval(self.timing.put_gap, count);
        let body = body_for(command, duration);
        session.pacer.attempted(index, now);
        match session.bridge.put_v2("light", &id, &body) {
            Ok(()) => {
                member.last = Some(command);
                self.backoff.reset();
                self.put_failure_logged = None;
                self.outage_logged = false;
            }
            Err(
                err @ (HueError::Network(_)
                | HueError::Unauthorized
                | HueError::CertificateMismatch),
            ) => return self.connection_failed(&err, now),
            Err(err) => {
                member.last = None;
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

fn read_snapshot(bridge: &Bridge, id: &str) -> Result<LightSnapshot, HueError> {
    let replies: Vec<LightStateReply> = bridge.get_v2(&format!("light/{id}"))?;
    replies
        .first()
        .and_then(LightStateReply::snapshot)
        .ok_or_else(|| HueError::Parse("the light reported no state".to_owned()))
}

/// PUTs each light back into its state, at most one PUT per `gap`, and stops starting new ones
/// after `budget` or when the bridge cannot be reached. Lights that fail are logged and
/// skipped.
fn restore_lights(
    bridge: &Bridge,
    lights: Vec<(String, LightSnapshot)>,
    gap: Duration,
    budget: Duration,
) {
    let started = Instant::now();
    let mut last_put: Option<Instant> = None;
    for (id, snapshot) in lights {
        if let Some(at) = last_put {
            std::thread::sleep(gap.saturating_sub(at.elapsed()));
        }
        if started.elapsed() >= budget {
            log::warn!("out of time restoring Hue light {id}; leaving it as it is");
            continue;
        }
        last_put = Some(Instant::now());
        match bridge.put_v2("light", &id, &restore_body(&snapshot, RESTORE_FADE)) {
            Ok(()) => {}
            Err(err @ (HueError::Network(_) | HueError::CertificateMismatch)) => {
                log::warn!("could not restore the Hue lights: {err}");
                return;
            }
            Err(err) => log::warn!("could not restore Hue light {id}: {err}"),
        }
    }
}

/// The lights to drive: the entertainment area (when one is chosen and found) followed by the
/// extra lights. Extra lights the bridge does not know are skipped, and logged when
/// `log_missing` is set.
fn resolve_members(
    inventory: &Inventory,
    area_id: Option<&str>,
    extras: &[String],
    log_missing: bool,
) -> Vec<Member> {
    let mut members: Vec<Member> = Vec::new();
    let new_member = |info: LightInfo, source, position| Member {
        info,
        source,
        position,
        want: None,
        last: None,
        snap: SnapState::Unknown,
    };
    if let Some(area_id) = area_id {
        match inventory.area_lights(area_id) {
            Some(lights) if !lights.is_empty() => {
                members.extend(
                    lights.into_iter().map(|(info, position)| {
                        new_member(info, ZoneSource::Entertainment, position)
                    }),
                );
            }
            _ if log_missing => {
                log::warn!("the selected Hue area was not found or has no lights");
            }
            _ => {}
        }
    }
    for id in extras {
        if members.iter().any(|m| m.info.id == *id) {
            continue;
        }
        match inventory.light(id) {
            Some(info) => members.push(new_member(info, ZoneSource::Extra, None)),
            None if log_missing => {
                log::warn!("Hue light {id} was not found on the bridge; skipping it");
            }
            None => {}
        }
    }
    members
}

/// Connects and resolves the lights to drive; having no light at all is
/// [`HueError::AreaNotFound`].
fn open_session(
    creds: &BridgeCredentials,
    area_id: Option<&str>,
    extras: &[String],
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
    let inventory = Inventory::fetch(&bridge)?;
    let members = resolve_members(&inventory, area_id, extras, true);
    if members.is_empty() {
        return Err(HueError::AreaNotFound);
    }
    let mut pacer = Pacer::new(put_gap);
    pacer.set_lights(members.len());
    Ok(Session {
        creds: creds.clone(),
        bridge,
        members,
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
            name: "L".to_owned(),
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
            name: "W".to_owned(),
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
            name: "P".to_owned(),
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
    fn restore_bodies_put_back_the_active_colour_mode() {
        let fade = Duration::from_millis(400);
        let xy = LightSnapshot {
            on: true,
            brightness: Some(42.5),
            xy: Some((0.31, 0.32)),
            mirek: None,
        };
        assert_eq!(
            restore_body(&xy, fade),
            json!({
                "on": {"on": true},
                "dimming": {"brightness": 42.5},
                "color": {"xy": {"x": 0.31, "y": 0.32}},
                "dynamics": {"duration": 400}
            })
        );
        // Colour-temperature mode wins over a stale xy value.
        let ct = LightSnapshot {
            mirek: Some(300),
            ..xy.clone()
        };
        assert_eq!(
            restore_body(&ct, fade),
            json!({
                "on": {"on": true},
                "dimming": {"brightness": 42.5},
                "color_temperature": {"mirek": 300},
                "dynamics": {"duration": 400}
            })
        );
        let off = LightSnapshot {
            on: false,
            brightness: None,
            xy: None,
            mirek: None,
        };
        assert_eq!(
            restore_body(&off, fade),
            json!({"on": {"on": false}, "dynamics": {"duration": 400}})
        );
    }

    #[test]
    fn per_light_colours_apply_only_to_their_epoch() {
        let whole = Rgb::new(1, 1, 1);
        let zones = vec![Rgb::new(2, 2, 2), Rgb::new(3, 3, 3)];
        let update = ColorUpdate::PerLight {
            epoch: 4,
            whole,
            zones: zones.clone(),
        };
        assert_eq!(color_for(Some(&update), 4, 1), Some(zones[1]));
        // A light beyond the list and a list of another epoch show the whole colour.
        assert_eq!(color_for(Some(&update), 4, 2), Some(whole));
        assert_eq!(color_for(Some(&update), 5, 0), Some(whole));
        assert_eq!(
            color_for(Some(&ColorUpdate::Whole(whole)), 9, 3),
            Some(whole)
        );
        assert_eq!(color_for(None, 0, 0), None);
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
