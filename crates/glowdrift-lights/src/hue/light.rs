//! [`HueLight`]: the [`Light`] implementation and its handle. All I/O runs on one worker thread;
//! `set_color` only stores the latest colour and wakes it.

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use glowdrift_core::Rgb;

use super::pacer::round_interval;
use super::worker::{Inbox, Status, Target, Timing, Worker};
use super::{BridgeCredentials, HUE_LIGHT_ID};
use crate::{Light, LightError, LightState, LightStatus};

/// How long [`Light::shutdown`] waits for the worker before abandoning it. Longer than one
/// request timeout, so a request in flight finishes first.
const SHUTDOWN_WAIT: Duration = Duration::from_secs(6);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

struct Shared {
    inbox: Mutex<Inbox>,
    wake: Condvar,
    status: Mutex<Status>,
}

impl Shared {
    fn update(&self, change: impl FnOnce(&mut Inbox)) {
        change(&mut lock(&self.inbox));
        self.wake.notify_one();
    }
}

/// Drives the lights of one Hue area with a single colour.
pub struct HueLight {
    shared: Arc<Shared>,
    timing: Timing,
    enabled: bool,
    last_color: Option<Rgb>,
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

impl HueLight {
    /// Creates an enabled light for `area_id` on the bridge described by `creds`.
    pub fn new(creds: Option<BridgeCredentials>, area_id: Option<String>) -> (HueLight, HueHandle) {
        HueLight::with_timing(creds, area_id, Timing::default())
    }

    pub(crate) fn with_timing(
        creds: Option<BridgeCredentials>,
        area_id: Option<String>,
        timing: Timing,
    ) -> (HueLight, HueHandle) {
        let shared = Arc::new(Shared {
            inbox: Mutex::new(Inbox {
                enabled: Some(true),
                target: Some(Target { creds, area_id }),
                ..Inbox::default()
            }),
            wake: Condvar::new(),
            status: Mutex::new(Status::new(LightState::Connecting, Some("Starting"))),
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
                enabled: true,
                last_color: None,
                thread,
                done,
            },
            handle,
        )
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
        if self.last_color == Some(color) {
            return Ok(());
        }
        self.last_color = Some(color);
        self.shared.update(|inbox| inbox.color = Some(color));
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
        loop {
            let batch = wait_for_work(shared, deadline);
            if worker.apply(batch) {
                *lock(&shared.status) = worker.status().clone();
                break;
            }
            deadline = worker.step(Instant::now());
            *lock(&shared.status) = worker.status().clone();
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
