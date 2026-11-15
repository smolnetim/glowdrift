//! [`HueLight`]: the [`Light`] implementation and its handle. All I/O runs on one worker thread;
//! `set_color` and `set_zone_colors` only store the latest colours and wake it.
//!
//! The worker publishes the lights it drives. From those, the zone overrides and the default
//! zones, [`Shared`] keeps the effective layout, whose generation changes whenever the lights
//! or a zone do.

use std::collections::BTreeMap;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use glowdrift_core::zones::EXTRA_ZONE_WIDTH;
use glowdrift_core::{LightPosition, NormRect, Rgb, default_zones};
use serde::{Deserialize, Serialize};

use super::pacer::round_interval;
use super::worker::{ColorUpdate, Inbox, Status, Target, TargetInfo, Timing, Worker};
use super::{BridgeCredentials, HUE_LIGHT_ID};
use crate::{Light, LightError, LightState, LightStatus, ZoneLayout};

/// How long [`Light::shutdown`] waits for the worker before abandoning it. Longer than one
/// request timeout, so a request in flight finishes first.
const SHUTDOWN_WAIT: Duration = Duration::from_secs(6);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

struct Shared {
    /// Zone rectangles that replace the default zone of a light, by light id.
    zone_overrides: Mutex<BTreeMap<String, NormRect>>,
    /// Ids of single lights driven in addition to the area.
    extra_lights: Mutex<Vec<String>>,
    inbox: Mutex<Inbox>,
    wake: Condvar,
    status: Mutex<Status>,
    /// The lights the worker drives, with the epoch they were published for.
    resolved: Mutex<Resolved>,
    /// The effective layout. Always locked before `zone_overrides` and `resolved`.
    layout: Mutex<Layout>,
}

#[derive(Default)]
struct Resolved {
    epoch: u64,
    lights: Option<Vec<TargetInfo>>,
}

#[derive(Default)]
struct Layout {
    generation: u64,
    /// The epoch of the resolved lights this layout was built from.
    epoch: u64,
    lights: Option<Vec<AreaLightZone>>,
}

/// The default zone of a light without a position.
fn extra_zone() -> NormRect {
    NormRect::new((1.0 - EXTRA_ZONE_WIDTH) / 2.0, 0.0, EXTRA_ZONE_WIDTH, 1.0)
}

/// The lights with their effective zones, in the order of `lights`. Entertainment lights get
/// their default zone from `default_zones` (which spreads them evenly when none has a
/// position), extra lights the centred box.
fn effective_zones(
    lights: &[TargetInfo],
    overrides: &BTreeMap<String, NormRect>,
) -> Vec<AreaLightZone> {
    let positions: Vec<Option<LightPosition>> = lights
        .iter()
        .filter(|l| l.source == ZoneSource::Entertainment)
        .map(|l| l.position)
        .collect();
    let mut defaults = default_zones(&positions).into_iter();
    lights
        .iter()
        .map(|light| {
            let default = match light.source {
                ZoneSource::Entertainment => defaults.next().unwrap_or_else(extra_zone),
                ZoneSource::Extra => extra_zone(),
            };
            let custom = overrides.get(&light.id).copied();
            AreaLightZone {
                id: light.id.clone(),
                name: light.name.clone(),
                position: light.position,
                zone: custom.unwrap_or(default),
                custom: custom.is_some(),
                source: light.source,
            }
        })
        .collect()
}

impl Shared {
    /// Rebuilds the layout from the current overrides and resolved lights; bumps the
    /// generation when the lights were resolved again or the lights or any zone differ from
    /// before. Colours addressed to an earlier epoch are shown as the whole colour by the
    /// worker, so the new generation is what makes the engine send per-light colours again.
    fn refresh_layout(&self) {
        let mut layout = lock(&self.layout);
        let overrides = lock(&self.zone_overrides).clone();
        let (epoch, lights) = {
            let resolved = lock(&self.resolved);
            (
                resolved.epoch,
                resolved
                    .lights
                    .as_deref()
                    .map(|lights| effective_zones(lights, &overrides)),
            )
        };
        let resolved_again = layout.epoch != epoch;
        layout.epoch = epoch;
        if resolved_again || layout.lights != lights {
            layout.generation += 1;
            layout.lights = lights;
        }
    }

    fn publish_resolved(&self, epoch: u64, lights: Option<Vec<TargetInfo>>) {
        *lock(&self.resolved) = Resolved { epoch, lights };
        self.refresh_layout();
    }

    fn update(&self, change: impl FnOnce(&mut Inbox)) {
        change(&mut lock(&self.inbox));
        self.wake.notify_one();
    }
}

/// Drives the lights of one Hue entertainment area plus any extra lights, with one colour or a
/// colour per light, and restores them when it stops.
pub struct HueLight {
    shared: Arc<Shared>,
    timing: Timing,
    enabled: bool,
    last_update: Option<ColorUpdate>,
    thread: Option<JoinHandle<()>>,
    done: Receiver<()>,
}

impl std::fmt::Debug for HueLight {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HueLight")
            .field("enabled", &self.enabled)
            .field("status", &*lock(&self.shared.status))
            .finish_non_exhaustive()
    }
}

/// Where a light of [`HueHandle::area_lights`] comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ZoneSource {
    /// A light of the selected entertainment area.
    Entertainment,
    /// A single light added through [`HueHandle::set_extra_lights`].
    Extra,
}

/// A light the Hue light drives, with the screen zone it samples.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AreaLightZone {
    pub id: String,
    pub name: String,
    /// Entertainment position; `None` for extra lights and lights without one.
    pub position: Option<LightPosition>,
    /// The effective zone, as a fraction of the monitor: the override when there is one,
    /// otherwise the default for the light's position.
    pub zone: NormRect,
    /// True when `zone` is an override.
    pub custom: bool,
    pub source: ZoneSource,
}

/// Changes which bridge and area a [`HueLight`] drives, from any thread.
#[derive(Clone)]
pub struct HueHandle {
    shared: Arc<Shared>,
}

impl std::fmt::Debug for HueHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HueHandle").finish_non_exhaustive()
    }
}

impl HueHandle {
    /// Points the light at another bridge or area; `None` credentials or area report
    /// `NotPaired` or `NoArea`. Never blocks; the worker reconnects in the background.
    pub fn set_target(&self, creds: Option<BridgeCredentials>, area_id: Option<String>) {
        self.shared
            .update(|inbox| inbox.target = Some(Target { creds, area_id }));
    }
}

impl HueHandle {
    /// Replaces the zone overrides: light id to a rectangle of the monitor. Lights without an
    /// entry use their default zone. Never blocks.
    pub fn set_zone_overrides(&self, overrides: BTreeMap<String, NormRect>) {
        *lock(&self.shared.zone_overrides) = overrides;
        self.shared.refresh_layout();
    }

    /// Replaces the ids of the single bridge lights driven in addition to the area, in the
    /// order they are listed by [`HueHandle::area_lights`]. Lights the bridge does not have are
    /// skipped. Never blocks; the worker re-resolves in the background.
    pub fn set_extra_lights(&self, ids: Vec<String>) {
        lock(&self.shared.extra_lights).clone_from(&ids);
        self.shared.update(|inbox| inbox.extras = Some(ids));
    }

    /// The lights currently driven with the zone each one samples: the area's lights in channel
    /// order, then the extra lights. Performs no I/O. `None` until the lights are resolved; the
    /// last result is kept while the bridge is unreachable or the light is disabled, and
    /// dropped when the bridge or area changes.
    pub fn area_lights(&self) -> Option<Vec<AreaLightZone>> {
        lock(&self.shared.layout).lights.clone()
    }
}

impl HueLight {
    /// Creates a disabled light for `area_id` on the bridge described by `creds`; it does not
    /// contact the bridge until [`Light::set_enabled`] enables it.
    pub fn new(creds: Option<BridgeCredentials>, area_id: Option<String>) -> (HueLight, HueHandle) {
        HueLight::build(creds, area_id, Timing::default(), false)
    }

    #[cfg(test)]
    pub(crate) fn with_timing(
        creds: Option<BridgeCredentials>,
        area_id: Option<String>,
        timing: Timing,
    ) -> (HueLight, HueHandle) {
        HueLight::build(creds, area_id, timing, true)
    }

    fn build(
        creds: Option<BridgeCredentials>,
        area_id: Option<String>,
        timing: Timing,
        enabled: bool,
    ) -> (HueLight, HueHandle) {
        let shared = Arc::new(Shared {
            zone_overrides: Mutex::new(BTreeMap::new()),
            extra_lights: Mutex::new(Vec::new()),
            inbox: Mutex::new(Inbox {
                enabled: enabled.then_some(true),
                target: Some(Target { creds, area_id }),
                ..Inbox::default()
            }),
            wake: Condvar::new(),
            status: Mutex::new(if enabled {
                Status::new(LightState::Connecting, Some("Starting"))
            } else {
                Status::new(LightState::Disabled, None)
            }),
            resolved: Mutex::new(Resolved::default()),
            layout: Mutex::new(Layout::default()),
        });
        let (done_tx, done) = mpsc::channel();
        let worker_shared = Arc::clone(&shared);
        let thread = std::thread::Builder::new()
            .name("glowdrift-hue".to_owned())
            .spawn(move || run_worker(&worker_shared, timing, &done_tx))
            .map_err(|err| {
                log::error!("could not start the Hue worker: {err}");
                *lock(&shared.status) = Status::new(
                    LightState::Error,
                    Some(&format!("could not start the Hue worker: {err}")),
                );
            })
            .ok();
        let handle = HueHandle {
            shared: Arc::clone(&shared),
        };
        (
            HueLight {
                shared,
                timing,
                enabled,
                last_update: None,
                thread,
                done,
            },
            handle,
        )
    }
}

impl HueLight {
    /// Queues `update` for the worker unless it is the one sent last.
    fn send_colors(&mut self, update: ColorUpdate) {
        if self.last_update.as_ref() == Some(&update) {
            return;
        }
        self.last_update = Some(update.clone());
        self.shared.update(|inbox| inbox.color = Some(update));
    }
}

impl Drop for HueLight {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl Light for HueLight {
    fn id(&self) -> &str {
        HUE_LIGHT_ID
    }

    fn status(&self) -> LightStatus {
        let published = lock(&self.shared.status).clone();
        let (state, detail, device_count, code) = if !self.enabled {
            (LightState::Disabled, None, 0, None)
        } else if published.state == LightState::Disabled {
            // Enabled, but the worker has not processed the request yet.
            (LightState::Connecting, Some("Starting".to_owned()), 0, None)
        } else {
            (
                published.state,
                published.detail,
                published.device_count,
                published.code,
            )
        };
        LightStatus {
            id: HUE_LIGHT_ID.to_owned(),
            kind: "hue".to_owned(),
            name: "Philips Hue".to_owned(),
            enabled: self.enabled,
            state,
            detail,
            device_count,
            code,
        }
    }

    /// One refresh of every light takes `lights * 100 ms`, so a bigger area means a longer
    /// interval; 100 ms until the area's size is known.
    fn min_send_interval(&self) -> Option<Duration> {
        let lights = lock(&self.shared.status).device_count;
        Some(round_interval(self.timing.put_gap, lights as usize))
    }

    fn set_enabled(&mut self, enabled: bool) {
        if self.enabled == enabled {
            return;
        }
        self.enabled = enabled;
        self.shared.update(|inbox| inbox.enabled = Some(enabled));
    }

    fn set_color(&mut self, color: Rgb) -> Result<(), LightError> {
        self.send_colors(ColorUpdate::Whole(color));
        Ok(())
    }

    fn zone_layout(&self) -> Option<ZoneLayout> {
        let layout = lock(&self.shared.layout);
        let lights = layout.lights.as_ref()?;
        Some(ZoneLayout {
            generation: layout.generation,
            zones: lights.iter().map(|light| light.zone).collect(),
        })
    }

    fn set_zone_colors(
        &mut self,
        generation: u64,
        whole: Rgb,
        zones: &[Rgb],
    ) -> Result<(), LightError> {
        let (current, epoch) = {
            let layout = lock(&self.shared.layout);
            let matches = layout.generation == generation
                && layout
                    .lights
                    .as_ref()
                    .is_some_and(|lights| lights.len() == zones.len());
            (matches, layout.epoch)
        };
        self.send_colors(if current {
            ColorUpdate::PerLight {
                epoch,
                whole,
                zones: zones.to_vec(),
            }
        } else {
            ColorUpdate::Whole(whole)
        });
        Ok(())
    }

    fn retry(&mut self) {
        self.shared.update(|inbox| inbox.retry = true);
    }

    fn shutdown(&mut self) {
        let Some(thread) = self.thread.take() else {
            return;
        };
        self.shared.update(|inbox| inbox.shutdown = true);
        match self.done.recv_timeout(SHUTDOWN_WAIT) {
            Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                let _ = thread.join();
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                log::warn!("Hue worker did not stop within {SHUTDOWN_WAIT:?}; detaching it");
            }
        }
    }
}

fn run_worker(shared: &Arc<Shared>, timing: Timing, done: &Sender<()>) {
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut worker = Worker::new(timing);
        let mut deadline: Option<Instant> = None;
        let mut published_epoch = 0;
        loop {
            let batch = wait_for_work(shared, deadline);
            let stop = worker.apply(batch);
            if !stop {
                deadline = worker.step(Instant::now());
            }
            *lock(&shared.status) = worker.status().clone();
            let (epoch, lights) = worker.resolved();
            if epoch != published_epoch {
                published_epoch = epoch;
                shared.publish_resolved(epoch, lights.map(<[TargetInfo]>::to_vec));
            }
            if stop {
                break;
            }
        }
    }));
    if outcome.is_err() {
        log::error!("Hue worker panicked");
        *lock(&shared.status) = Status::new(
            LightState::Error,
            Some("The Hue worker stopped unexpectedly"),
        );
    }
    let _ = done.send(());
}

fn wait_for_work(shared: &Shared, deadline: Option<Instant>) -> Inbox {
    let mut inbox = lock(&shared.inbox);
    loop {
        if inbox.has_pending() {
            return std::mem::take(&mut *inbox);
        }
        match deadline {
            None => {
                inbox = shared
                    .wake
                    .wait(inbox)
                    .unwrap_or_else(PoisonError::into_inner);
            }
            Some(at) => {
                let now = Instant::now();
                if at <= now {
                    return std::mem::take(&mut *inbox);
                }
                inbox = shared
                    .wake
                    .wait_timeout(inbox, at - now)
                    .unwrap_or_else(PoisonError::into_inner)
                    .0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handle_stores_overrides_and_extra_lights_without_a_layout() {
        let (light, handle) = HueLight::new(None, None);
        handle.set_zone_overrides(BTreeMap::from([("a".to_owned(), NormRect::full())]));
        handle.set_extra_lights(vec!["b".to_owned()]);
        assert_eq!(lock(&light.shared.zone_overrides).len(), 1);
        assert_eq!(*lock(&light.shared.extra_lights), vec!["b".to_owned()]);
        assert_eq!(handle.area_lights(), None);
        assert_eq!(light.zone_layout(), None);
    }

    #[test]
    fn resolving_the_same_lights_again_changes_the_generation() {
        let (light, _handle) = HueLight::new(None, None);
        let lights = || {
            Some(vec![TargetInfo {
                id: "a".to_owned(),
                name: "Lamp".to_owned(),
                position: None,
                source: ZoneSource::Extra,
            }])
        };
        light.shared.publish_resolved(1, lights());
        let first = light.zone_layout().unwrap();
        light.shared.publish_resolved(1, lights());
        assert_eq!(light.zone_layout().unwrap(), first);
        light.shared.publish_resolved(2, lights());
        let second = light.zone_layout().unwrap();
        assert_eq!(second.zones, first.zones);
        assert_ne!(second.generation, first.generation);
    }

    #[test]
    fn area_light_zone_serialises_camel_case() {
        let zone = AreaLightZone {
            id: "a".to_owned(),
            name: "Lamp".to_owned(),
            position: Some(LightPosition {
                x: -1.0,
                y: 0.0,
                z: 0.5,
            }),
            zone: NormRect::full(),
            custom: false,
            source: ZoneSource::Extra,
        };
        let v = serde_json::to_value(&zone).unwrap();
        assert_eq!(v["source"], "extra");
        assert_eq!(v["custom"], false);
        assert_eq!(v["position"]["z"], 0.5);
        assert_eq!(v["zone"]["width"], 1.0);
        assert_eq!(
            serde_json::to_value(ZoneSource::Entertainment).unwrap(),
            "entertainment"
        );
    }
}
