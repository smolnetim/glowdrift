//! Corsair iCUE light via the official iCUE SDK (v4, shared access).
//!
//! [`IcueLight`] owns a worker thread. `set_color` only stores the latest colour and wakes the
//! worker; the worker talks to iCUE (connect, session changes, device enumeration, colour
//! writes at up to ~30 Hz) and releases everything again on disable or shutdown, so the user's
//! own iCUE lighting returns immediately.
//!
//! Access is shared, not exclusive: no control is requested, and the client layer priority is
//! [`controller::LAYER_PRIORITY`], above iCUE's own profile and level with other shared clients.
//! The SDK library (`iCUESDK.x64_2019.dll`) is loaded at run time, and only from the
//! executable's directory, its `resources` subdirectory and directories passed to
//! [`IcueLight::with_dll_dirs`]; `PATH` and the working directory are never searched.

mod controller;
#[allow(unsafe_code)]
mod ffi;
mod probe;
pub(crate) mod sdk;

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use glowdrift_core::Rgb;

use self::controller::{Controller, Status};
use self::ffi::FfiSdk;
use self::sdk::{EventSink, Sdk, SdkEvent};
use crate::{Light, LightError, LightState, LightStatus};

pub use self::probe::{ProbeDevice, ProbeReport, probe};

/// Light id reported by [`IcueLight`].
pub const ICUE_LIGHT_ID: &str = "icue";

/// How long [`Light::shutdown`] waits for the worker before abandoning it.
const SHUTDOWN_WAIT: Duration = Duration::from_secs(3);

/// The directories the SDK library may be loaded from: `extra`, then the executable's
/// `resources` subdirectory, then the executable's directory.
fn library_dirs(mut extra: Vec<PathBuf>) -> Vec<PathBuf> {
    if let Some(exe_dir) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(std::path::Path::to_path_buf))
    {
        extra.push(exe_dir.join("resources"));
        extra.push(exe_dir);
    }
    extra
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Debug, Default)]
struct Inbox {
    color: Option<Rgb>,
    enabled: Option<bool>,
    events: VecDeque<SdkEvent>,
    retry: bool,
    shutdown: bool,
}

impl Inbox {
    fn has_pending(&self) -> bool {
        self.color.is_some()
            || self.enabled.is_some()
            || !self.events.is_empty()
            || self.retry
            || self.shutdown
    }
}

/// State shared between the [`IcueLight`] handle, the SDK callbacks and the worker.
struct Shared {
    inbox: Mutex<Inbox>,
    wake: Condvar,
    status: Mutex<Status>,
}

impl Shared {
    fn push_event(&self, event: SdkEvent) {
        let mut inbox = lock(&self.inbox);
        if event == SdkEvent::DevicesChanged && inbox.events.back() == Some(&event) {
            return;
        }
        inbox.events.push_back(event);
        drop(inbox);
        self.wake.notify_one();
    }

    fn update(&self, change: impl FnOnce(&mut Inbox)) {
        change(&mut lock(&self.inbox));
        self.wake.notify_one();
    }
}

/// Drives iCUE-controlled devices with a single colour.
pub struct IcueLight {
    shared: Arc<Shared>,
    enabled: bool,
    last_color: Option<Rgb>,
    thread: Option<JoinHandle<()>>,
    done: Receiver<()>,
}

impl std::fmt::Debug for IcueLight {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IcueLight")
            .field("enabled", &self.enabled)
            .field("status", &*lock(&self.shared.status))
            .finish_non_exhaustive()
    }
}

impl IcueLight {
    /// Creates an enabled light that looks for the SDK library next to the executable.
    pub fn new() -> IcueLight {
        IcueLight::with_dll_dirs(Vec::new())
    }

    /// Like [`IcueLight::new`], additionally looking for the SDK library in `dirs` (for
    /// example the application's bundled resources directory).
    pub fn with_dll_dirs(dirs: Vec<PathBuf>) -> IcueLight {
        IcueLight::spawn(FfiSdk::new(library_dirs(dirs)))
    }

    fn spawn<S: Sdk + 'static>(sdk: S) -> IcueLight {
        let shared = Arc::new(Shared {
            inbox: Mutex::new(Inbox {
                enabled: Some(true),
                ..Inbox::default()
            }),
            wake: Condvar::new(),
            status: Mutex::new(Status {
                state: LightState::Connecting,
                detail: Some("Starting".to_owned()),
                device_count: 0,
                code: None,
            }),
        });
        let (done_tx, done) = mpsc::channel();
        let worker_shared = Arc::clone(&shared);
        let thread = std::thread::Builder::new()
            .name("glowdrift-icue".to_owned())
            .spawn(move || run_worker(&worker_shared, sdk, &done_tx))
            .map_err(|err| {
                log::error!("could not start the iCUE worker: {err}");
                *lock(&shared.status) = Status {
                    state: LightState::Error,
                    detail: Some(format!("could not start the iCUE worker: {err}")),
                    device_count: 0,
                    code: None,
                };
            })
            .ok();
        IcueLight {
            shared,
            enabled: true,
            last_color: None,
            thread,
            done,
        }
    }
}

impl Default for IcueLight {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for IcueLight {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl Light for IcueLight {
    fn id(&self) -> &str {
        ICUE_LIGHT_ID
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
            id: ICUE_LIGHT_ID.to_owned(),
            kind: "icue".to_owned(),
            name: "Corsair iCUE".to_owned(),
            enabled: self.enabled,
            state,
            detail,
            device_count,
            code,
        }
    }

    fn retry(&mut self) {
        self.shared.update(|inbox| inbox.retry = true);
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
                log::warn!("iCUE worker did not stop within {SHUTDOWN_WAIT:?}; detaching it");
            }
        }
    }
}

/// Worker thread body: applies queued inputs to the controller and sleeps until the next
/// deadline or wake-up.
fn run_worker<S: Sdk>(shared: &Arc<Shared>, sdk: S, done: &Sender<()>) {
    let sink_shared = Arc::clone(shared);
    let sink: EventSink = Arc::new(move |event| sink_shared.push_event(event));
    let mut controller = Controller::new(sdk, sink);

    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut deadline: Option<Instant> = None;
        loop {
            let batch = wait_for_work(shared, deadline);
            let now = Instant::now();
            if batch.shutdown {
                controller.shutdown();
                *lock(&shared.status) = controller.status().clone();
                break;
            }
            if let Some(enabled) = batch.enabled {
                controller.set_enabled(enabled);
            }
            for event in batch.events {
                controller.on_event(event, now);
            }
            if batch.retry {
                controller.retry_now();
            }
            if let Some(color) = batch.color {
                controller.set_color(color);
            }
            deadline = controller.tick(Instant::now());
            *lock(&shared.status) = controller.status().clone();
        }
    }));
    if outcome.is_err() {
        log::error!("iCUE worker panicked");
        *lock(&shared.status) = Status {
            state: LightState::Error,
            detail: Some("The iCUE worker stopped unexpectedly".to_owned()),
            device_count: 0,
            code: None,
        };
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
mod tests;
