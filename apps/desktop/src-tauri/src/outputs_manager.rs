//! Owns the output sinks: applies the enabled flags, fans out colours, polls status.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::thread;
use std::time::Duration;

use glowdrift_core::{OutputsConfig, Rgb};
use glowdrift_outputs::icue::{ICUE_OUTPUT_ID, IcueOutput};
use glowdrift_outputs::{Output, OutputState, OutputStatus};
use tauri::{AppHandle, Manager};

const POLL_INTERVAL: Duration = Duration::from_secs(1);
const PAUSED_DETAIL: &str = "Paused";

type StatusEmitter = Box<dyn Fn(&[OutputStatus]) + Send + Sync>;

/// The `icue-sdk` folder in the app data directory, where a downloaded SDK library is stored.
pub fn icue_sdk_dir(app: &AppHandle) -> Option<PathBuf> {
    app.path()
        .app_data_dir()
        .ok()
        .map(|dir| dir.join("icue-sdk"))
}

/// Extra directories searched for the iCUE SDK runtime, in addition to the executable's
/// own directory and its `resources` subdirectory: the bundled resource directory and the
/// [`icue_sdk_dir`].
pub fn icue_dll_dirs(app: &AppHandle) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(dir) = app.path().resource_dir() {
        dirs.push(dir);
    }
    dirs.extend(icue_sdk_dir(app));
    dirs
}

/// Builds the default set of outputs.
pub fn default_outputs(app: &AppHandle) -> Vec<Box<dyn Output>> {
    vec![Box::new(IcueOutput::with_dll_dirs(icue_dll_dirs(app)))]
}

fn is_wanted(id: &str, config: &OutputsConfig) -> bool {
    match id {
        ICUE_OUTPUT_ID => config.icue.enabled,
        _ => false,
    }
}

struct Slot {
    output: Box<dyn Output>,
    wanted: bool,
    applied: Option<bool>,
    was_connected: bool,
}

impl Slot {
    fn sync_enabled(&mut self, paused: bool) {
        let effective = self.wanted && !paused;
        if self.applied != Some(effective) {
            self.output.set_enabled(effective);
            self.applied = Some(effective);
        }
    }
}

struct Inner {
    slots: Vec<Slot>,
    paused: bool,
    closed: bool,
}

/// Shared handle to all outputs. Methods never block on device I/O beyond what the
/// individual [`Output`] implementations do.
pub struct OutputsManager {
    inner: Mutex<Inner>,
    last_emitted: Mutex<Option<Vec<OutputStatus>>>,
    resend: AtomicBool,
    emit: StatusEmitter,
    poller_stop: Mutex<Option<Sender<()>>>,
}

impl std::fmt::Debug for OutputsManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OutputsManager").finish_non_exhaustive()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl OutputsManager {
    pub fn new(
        outputs: Vec<Box<dyn Output>>,
        config: &OutputsConfig,
        emit: impl Fn(&[OutputStatus]) + Send + Sync + 'static,
    ) -> Arc<Self> {
        let slots = outputs
            .into_iter()
            .map(|output| Slot {
                wanted: is_wanted(output.id(), config),
                output,
                applied: None,
                was_connected: false,
            })
            .collect();
        let manager = Arc::new(Self {
            inner: Mutex::new(Inner {
                slots,
                paused: false,
                closed: false,
            }),
            last_emitted: Mutex::new(None),
            resend: AtomicBool::new(false),
            emit: Box::new(emit),
            poller_stop: Mutex::new(None),
        });
        {
            let mut inner = lock(&manager.inner);
            let paused = inner.paused;
            for slot in &mut inner.slots {
                slot.sync_enabled(paused);
            }
        }
        manager
    }

    /// Starts the background status poller (about once per second).
    pub fn start_poller(self: &Arc<Self>) {
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        *lock(&self.poller_stop) = Some(stop_tx);
        let weak: Weak<Self> = Arc::downgrade(self);
        let spawned = thread::Builder::new()
            .name("glowdrift-outputs-poll".to_owned())
            .spawn(move || {
                while let Err(RecvTimeoutError::Timeout) = stop_rx.recv_timeout(POLL_INTERVAL) {
                    let Some(manager) = weak.upgrade() else {
                        break;
                    };
                    manager.poll();
                }
            });
        if let Err(e) = spawned {
            log::error!("cannot start the outputs poller: {e}");
        }
    }

    /// Applies the per-output enabled flags from the configuration.
    pub fn apply_config(&self, config: &OutputsConfig) {
        {
            let mut inner = lock(&self.inner);
            let paused = inner.paused;
            for slot in &mut inner.slots {
                slot.wanted = is_wanted(slot.output.id(), config);
                slot.sync_enabled(paused);
            }
        }
        self.poll();
    }

    /// While paused every output is released so normal lighting returns.
    pub fn set_paused(&self, paused: bool) {
        {
            let mut inner = lock(&self.inner);
            inner.paused = paused;
            for slot in &mut inner.slots {
                slot.sync_enabled(paused);
            }
        }
        self.poll();
    }

    /// Asks the output with `id` to retry connecting now, then publishes the new status.
    pub fn retry(&self, id: &str) {
        {
            let mut inner = lock(&self.inner);
            if inner.closed {
                return;
            }
            for slot in inner.slots.iter_mut().filter(|s| s.output.id() == id) {
                slot.output.retry();
            }
        }
        self.poll();
    }

    /// Sends a colour to every active output.
    pub fn set_color(&self, color: Rgb) {
        let mut inner = lock(&self.inner);
        if inner.paused || inner.closed {
            return;
        }
        for slot in inner.slots.iter_mut().filter(|s| s.wanted) {
            if let Err(e) = slot.output.set_color(color) {
                log::debug!("output {}: {e}", slot.output.id());
            }
        }
    }

    /// True once after an output (re)connected; the caller should resend the current colour.
    pub fn take_resend(&self) -> bool {
        self.resend.swap(false, Ordering::AcqRel)
    }

    /// Current status of every output as shown to the UI.
    pub fn statuses(&self) -> Vec<OutputStatus> {
        Self::collect(&lock(&self.inner))
    }

    fn collect(inner: &Inner) -> Vec<OutputStatus> {
        inner
            .slots
            .iter()
            .map(|slot| {
                let mut status = slot.output.status();
                if inner.paused && slot.wanted {
                    status.enabled = true;
                    status.state = OutputState::Disabled;
                    status.detail = Some(PAUSED_DETAIL.to_owned());
                }
                status
            })
            .collect()
    }

    /// Reads all statuses, flags reconnects and emits when anything changed.
    pub fn poll(&self) {
        let statuses = {
            let mut inner = lock(&self.inner);
            for slot in &mut inner.slots {
                let connected = slot.output.status().state == OutputState::Connected;
                if connected && !slot.was_connected {
                    self.resend.store(true, Ordering::Release);
                }
                slot.was_connected = connected;
            }
            Self::collect(&inner)
        };
        let mut last = lock(&self.last_emitted);
        if last.as_ref() != Some(&statuses) {
            (self.emit)(&statuses);
            *last = Some(statuses);
        }
    }

    /// Releases all devices and stops the poller. Later calls do nothing.
    pub fn shutdown(&self) {
        lock(&self.poller_stop).take();
        let mut inner = lock(&self.inner);
        if inner.closed {
            return;
        }
        inner.closed = true;
        for slot in &mut inner.slots {
            slot.output.shutdown();
            slot.applied = None;
        }
        log::info!("outputs released");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glowdrift_core::IcueConfig;

    #[derive(Default)]
    struct Shared {
        enabled: Option<bool>,
        colors: Vec<Rgb>,
        connected: bool,
        shutdowns: u32,
        retries: u32,
    }

    struct Fake(Arc<Mutex<Shared>>);

    impl Output for Fake {
        fn id(&self) -> &str {
            ICUE_OUTPUT_ID
        }
        fn status(&self) -> OutputStatus {
            let s = lock(&self.0);
            let enabled = s.enabled.unwrap_or(false);
            OutputStatus {
                id: ICUE_OUTPUT_ID.to_owned(),
                kind: "icue".to_owned(),
                name: "Fake".to_owned(),
                enabled,
                state: if !enabled {
                    OutputState::Disabled
                } else if s.connected {
                    OutputState::Connected
                } else {
                    OutputState::Connecting
                },
                detail: None,
                device_count: 0,
                code: None,
            }
        }
        fn retry(&mut self) {
            lock(&self.0).retries += 1;
        }
        fn set_enabled(&mut self, enabled: bool) {
            lock(&self.0).enabled = Some(enabled);
        }
        fn set_color(&mut self, color: Rgb) -> Result<(), glowdrift_outputs::OutputError> {
            lock(&self.0).colors.push(color);
            Ok(())
        }
        fn shutdown(&mut self) {
            lock(&self.0).shutdowns += 1;
        }
    }

    fn config(enabled: bool) -> OutputsConfig {
        OutputsConfig {
            icue: IcueConfig { enabled },
        }
    }

    fn setup(enabled: bool) -> (Arc<OutputsManager>, Arc<Mutex<Shared>>, Arc<Mutex<u32>>) {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let emitted = Arc::new(Mutex::new(0_u32));
        let counter = Arc::clone(&emitted);
        let manager = OutputsManager::new(
            vec![Box::new(Fake(Arc::clone(&shared)))],
            &config(enabled),
            move |_| *lock(&counter) += 1,
        );
        (manager, shared, emitted)
    }

    #[test]
    fn enabled_flag_is_applied_and_colors_forwarded() {
        let (manager, shared, _) = setup(true);
        assert_eq!(lock(&shared).enabled, Some(true));
        manager.set_color(Rgb::new(1, 2, 3));
        assert_eq!(lock(&shared).colors, vec![Rgb::new(1, 2, 3)]);

        manager.apply_config(&config(false));
        assert_eq!(lock(&shared).enabled, Some(false));
        manager.set_color(Rgb::new(9, 9, 9));
        assert_eq!(lock(&shared).colors.len(), 1);
    }

    #[test]
    fn pausing_releases_and_resuming_restores() {
        let (manager, shared, _) = setup(true);
        manager.set_paused(true);
        assert_eq!(lock(&shared).enabled, Some(false));
        manager.set_color(Rgb::new(5, 5, 5));
        assert_eq!(lock(&shared).colors.len(), 0);

        let status = &manager.statuses()[0];
        assert!(status.enabled);
        assert_eq!(status.state, OutputState::Disabled);
        assert_eq!(status.detail.as_deref(), Some(PAUSED_DETAIL));

        manager.set_paused(false);
        assert_eq!(lock(&shared).enabled, Some(true));
    }

    #[test]
    fn user_disabled_output_is_not_reported_as_paused() {
        let (manager, _, _) = setup(false);
        manager.set_paused(true);
        let status = &manager.statuses()[0];
        assert!(!status.enabled);
        assert_eq!(status.detail, None);
    }

    #[test]
    fn poll_emits_only_on_change_and_flags_reconnect() {
        let (manager, shared, emitted) = setup(true);
        manager.poll();
        manager.poll();
        let after_two = *lock(&emitted);
        manager.poll();
        assert_eq!(*lock(&emitted), after_two);
        assert!(!manager.take_resend());

        lock(&shared).connected = true;
        manager.poll();
        assert_eq!(*lock(&emitted), after_two + 1);
        assert!(manager.take_resend());
        assert!(!manager.take_resend());
    }

    #[test]
    fn retry_reaches_only_the_named_output_and_not_after_shutdown() {
        let (manager, shared, _) = setup(true);
        manager.retry("other");
        assert_eq!(lock(&shared).retries, 0);
        manager.retry(ICUE_OUTPUT_ID);
        assert_eq!(lock(&shared).retries, 1);
        manager.shutdown();
        manager.retry(ICUE_OUTPUT_ID);
        assert_eq!(lock(&shared).retries, 1);
    }

    #[test]
    fn shutdown_shuts_outputs_down() {
        let (manager, shared, _) = setup(true);
        manager.shutdown();
        assert_eq!(lock(&shared).shutdowns, 1);
    }
}
