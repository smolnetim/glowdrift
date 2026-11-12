//! The colour engine: capture -> reduce -> smooth -> outputs on one dedicated thread.
//!
//! Capture runs at `captureFps`; the smoother ticks at a fixed output rate that is independent
//! of the capture rate and stops entirely once the output has reached its target.

use std::io;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use glowdrift_capture::{CaptureError, Capturer};
use glowdrift_core::{ColorAdjust, Config, LinearRgb, Rgb, Smoother, reduce};
use serde::{Deserialize, Serialize};

use crate::outputs_manager::OutputsManager;

/// Rate at which the smoother is advanced while the colour is still moving.
const TICK_INTERVAL: Duration = Duration::from_micros(16_667);
/// Longest wait for a desktop frame inside one capture attempt.
const GRAB_TIMEOUT: Duration = Duration::from_millis(2);
/// Longest side, in pixels, of the frames requested from the capturer.
const CAPTURE_MAX_DIMENSION: u32 = 128;
/// Upper bound of a single sleep, which bounds command latency.
const MAX_SLEEP: Duration = Duration::from_millis(50);
/// Largest time step handed to the smoother.
const MAX_DT: Duration = Duration::from_millis(100);
/// Minimum spacing of `engine://color` events.
const PREVIEW_INTERVAL: Duration = Duration::from_millis(50);
const FPS_WINDOW: Duration = Duration::from_secs(1);
const RECOVERY_MIN: Duration = Duration::from_millis(250);
const RECOVERY_MAX: Duration = Duration::from_secs(2);
const ERROR_RETRY: Duration = Duration::from_secs(3);
const IDLE_COLOR: &str = "#000000";

/// Engine state reported to the UI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineStatus {
    pub running: bool,
    pub measured_fps: f32,
    pub last_error: Option<String>,
    /// Unsmoothed target colour as `#rrggbb`.
    pub raw: String,
    /// Smoothed output colour as `#rrggbb`.
    pub smoothed: String,
}

impl Default for EngineStatus {
    fn default() -> Self {
        Self {
            running: false,
            measured_fps: 0.0,
            last_error: None,
            raw: IDLE_COLOR.to_owned(),
            smoothed: IDLE_COLOR.to_owned(),
        }
    }
}

/// Receiver of engine notifications. Implementations must not block.
pub trait EngineEvents: Send + 'static {
    /// Throttled colour preview, only called while previews are enabled.
    fn color(&self, raw: Rgb, smoothed: Rgb);
    /// Called when the running flag, the error, or the rounded frame rate changes.
    fn status(&self, status: &EngineStatus);
}

/// Messages accepted by the engine thread.
#[derive(Debug)]
pub enum EngineCommand {
    ApplyConfig(Box<Config>),
    SetRunning(bool),
    SetPreviewEnabled(bool),
    Shutdown,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// User-presentable description of a capture failure.
pub fn describe_capture_error(error: &CaptureError) -> String {
    match error {
        CaptureError::MonitorNotFound => "The selected monitor is not available.".to_owned(),
        CaptureError::AccessLost => "Screen capture was interrupted.".to_owned(),
        CaptureError::Unsupported => "Screen capture is not available on this system.".to_owned(),
        CaptureError::Os(detail) => format!("Screen capture failed: {detail}"),
    }
}

// ---------------------------------------------------------------------------------------------
// Pure scheduling pieces
// ---------------------------------------------------------------------------------------------

/// Interval between captures for a frame rate, clamped to the supported range.
pub fn capture_interval(fps: u32) -> Duration {
    let range = Config::CAPTURE_FPS_RANGE;
    Duration::from_secs_f64(1.0 / f64::from(fps.clamp(*range.start(), *range.end())))
}

/// Next deadline after running at `now` a task that was due at `previous`: keeps the cadence
/// when slightly late and drops missed beats when a whole interval behind.
pub fn advance_deadline(previous: Instant, now: Instant, interval: Duration) -> Instant {
    let next = previous + interval;
    if next <= now { now + interval } else { next }
}

/// True when the 8-bit output has reached the 8-bit target, so smoothing can stop ticking.
pub fn is_settled(smoothed: Rgb, target: Rgb) -> bool {
    smoothed == target
}

/// Doubling delay between recovery attempts.
#[derive(Debug, Clone)]
pub struct Backoff {
    min: Duration,
    max: Duration,
    next: Duration,
}

impl Backoff {
    pub fn new(min: Duration, max: Duration) -> Self {
        Self {
            min,
            max,
            next: min,
        }
    }

    /// Returns the delay to wait now and lengthens the following one.
    pub fn next_delay(&mut self) -> Duration {
        let delay = self.next;
        self.next = (self.next * 2).min(self.max);
        delay
    }

    pub fn reset(&mut self) {
        self.next = self.min;
    }
}

/// Rate limiter that never loses the latest value: `mark` records that something is waiting,
/// `poll` releases it once the interval has elapsed.
#[derive(Debug, Clone)]
pub struct Throttle {
    interval: Duration,
    last: Option<Instant>,
    pending: bool,
}

impl Throttle {
    pub fn new(interval: Duration) -> Self {
        Self {
            interval,
            last: None,
            pending: false,
        }
    }

    pub fn mark(&mut self) {
        self.pending = true;
    }

    pub fn clear(&mut self) {
        self.pending = false;
    }

    /// True when a marked value may be emitted now.
    pub fn poll(&mut self, now: Instant) -> bool {
        if !self.pending || self.last.is_some_and(|last| now < last + self.interval) {
            return false;
        }
        self.pending = false;
        self.last = Some(now);
        true
    }

    /// When a pending value becomes emittable.
    pub fn deadline(&self, now: Instant) -> Option<Instant> {
        self.pending
            .then(|| self.last.map_or(now, |last| last + self.interval))
    }
}

/// Frames-per-second meter over fixed windows, lightly smoothed.
#[derive(Debug, Clone)]
pub struct FpsMeter {
    window_start: Instant,
    frames: u32,
    smoothed: Option<f32>,
}

impl FpsMeter {
    pub fn new(now: Instant) -> Self {
        Self {
            window_start: now,
            frames: 0,
            smoothed: None,
        }
    }

    pub fn record_frame(&mut self) {
        self.frames += 1;
    }

    pub fn reset(&mut self, now: Instant) {
        *self = Self::new(now);
    }

    pub fn deadline(&self) -> Instant {
        self.window_start + FPS_WINDOW
    }

    /// Closes the window when it has elapsed and returns the new smoothed rate.
    pub fn poll(&mut self, now: Instant) -> Option<f32> {
        let elapsed = now.saturating_duration_since(self.window_start);
        if elapsed < FPS_WINDOW {
            return None;
        }
        let instant = self.frames as f32 / elapsed.as_secs_f32();
        let value = self
            .smoothed
            .map_or(instant, |previous| previous * 0.5 + instant * 0.5);
        self.smoothed = Some(value);
        self.window_start = now;
        self.frames = 0;
        Some(value)
    }
}

// ---------------------------------------------------------------------------------------------
// Handle
// ---------------------------------------------------------------------------------------------

/// Control surface of the engine thread.
#[derive(Debug)]
pub struct EngineHandle {
    tx: Sender<EngineCommand>,
    status: Arc<Mutex<EngineStatus>>,
    join: Mutex<Option<JoinHandle<()>>>,
}

impl EngineHandle {
    /// Starts the engine thread, running, with `config`.
    pub fn spawn(
        config: Config,
        outputs: Arc<OutputsManager>,
        events: impl EngineEvents,
    ) -> io::Result<Self> {
        let (tx, rx) = mpsc::channel();
        let status = Arc::new(Mutex::new(EngineStatus {
            running: true,
            ..EngineStatus::default()
        }));
        let engine = Engine::new(rx, Arc::clone(&status), outputs, events, config);
        let join = thread::Builder::new()
            .name("glowdrift-engine".to_owned())
            .spawn(move || engine.run())?;
        Ok(Self {
            tx,
            status,
            join: Mutex::new(Some(join)),
        })
    }

    fn send(&self, command: EngineCommand) {
        if self.tx.send(command).is_err() {
            log::debug!("engine is not running; command dropped");
        }
    }

    pub fn apply_config(&self, config: Config) {
        self.send(EngineCommand::ApplyConfig(Box::new(config)));
    }

    pub fn set_running(&self, running: bool) {
        lock(&self.status).running = running;
        self.send(EngineCommand::SetRunning(running));
    }

    pub fn set_preview_enabled(&self, enabled: bool) {
        self.send(EngineCommand::SetPreviewEnabled(enabled));
    }

    pub fn status(&self) -> EngineStatus {
        lock(&self.status).clone()
    }

    /// Stops the thread, releasing outputs, and waits for it. Safe to call repeatedly.
    pub fn shutdown(&self) {
        let Some(join) = lock(&self.join).take() else {
            return;
        };
        self.send(EngineCommand::Shutdown);
        if join.join().is_err() {
            log::error!("engine thread panicked");
        } else {
            log::info!("engine stopped");
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Engine thread
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReportKey {
    running: bool,
    fps: u32,
    error: Option<String>,
}

#[allow(clippy::struct_excessive_bools)]
struct Engine<E: EngineEvents> {
    rx: Receiver<EngineCommand>,
    shared: Arc<Mutex<EngineStatus>>,
    outputs: Arc<OutputsManager>,
    events: E,
    config: Config,
    running: bool,
    preview: bool,

    capturer: Option<Capturer>,
    retry_at: Option<Instant>,
    backoff: Backoff,

    smoother: Smoother,
    last_reduced: LinearRgb,
    has_target: bool,
    target: LinearRgb,
    target_rgb: Rgb,
    smoothed_rgb: Rgb,
    settled: bool,

    last_tick: Instant,
    next_tick: Instant,
    next_capture: Instant,
    last_sent: Option<Rgb>,

    fps: FpsMeter,
    throttle: Throttle,
    reported: Option<ReportKey>,
}

impl<E: EngineEvents> Engine<E> {
    fn new(
        rx: Receiver<EngineCommand>,
        shared: Arc<Mutex<EngineStatus>>,
        outputs: Arc<OutputsManager>,
        events: E,
        config: Config,
    ) -> Self {
        let now = Instant::now();
        outputs.apply_config(&config.outputs);
        Self {
            rx,
            shared,
            outputs,
            events,
            config,
            running: true,
            preview: false,
            capturer: None,
            retry_at: None,
            backoff: Backoff::new(RECOVERY_MIN, RECOVERY_MAX),
            smoother: Smoother::new(),
            last_reduced: LinearRgb::BLACK,
            has_target: false,
            target: LinearRgb::BLACK,
            target_rgb: Rgb::BLACK,
            smoothed_rgb: Rgb::BLACK,
            settled: true,
            last_tick: now,
            next_tick: now,
            next_capture: now,
            last_sent: None,
            fps: FpsMeter::new(now),
            throttle: Throttle::new(PREVIEW_INTERVAL),
            reported: None,
        }
    }

    fn run(mut self) {
        loop {
            let command = if self.running {
                match self.rx.try_recv() {
                    Ok(command) => Some(command),
                    Err(TryRecvError::Empty) => None,
                    Err(TryRecvError::Disconnected) => break,
                }
            } else {
                match self.rx.recv() {
                    Ok(command) => Some(command),
                    Err(_) => break,
                }
            };
            if let Some(command) = command {
                if !self.handle(command) {
                    break;
                }
                continue;
            }
            let now = Instant::now();
            self.step(now);
            self.sleep_until_next(now);
        }
        self.outputs.shutdown();
    }

    /// Returns false when the engine must stop.
    fn handle(&mut self, command: EngineCommand) -> bool {
        let now = Instant::now();
        match command {
            EngineCommand::ApplyConfig(config) => self.apply_config(*config, now),
            EngineCommand::SetRunning(running) => self.set_running(running, now),
            EngineCommand::SetPreviewEnabled(enabled) => {
                if enabled != self.preview {
                    self.preview = enabled;
                    if enabled {
                        self.throttle.mark();
                    } else {
                        self.throttle.clear();
                    }
                }
            }
            EngineCommand::Shutdown => return false,
        }
        true
    }

    fn step(&mut self, now: Instant) {
        if self.outputs.take_resend() {
            self.last_sent = None;
            if self.has_target {
                self.send_current();
            }
        }
        if now >= self.next_capture {
            self.capture(now);
            self.next_capture = advance_deadline(
                self.next_capture,
                now,
                capture_interval(self.config.capture_fps),
            );
        }
        if !self.settled && now >= self.next_tick {
            self.tick(now);
        }
        if self.preview && self.throttle.poll(now) {
            self.events.color(self.target_rgb, self.smoothed_rgb);
        }
        if let Some(fps) = self.fps.poll(now) {
            lock(&self.shared).measured_fps = fps;
            self.report();
        }
    }

    fn sleep_until_next(&self, now: Instant) {
        let mut wake = self.next_capture.min(self.fps.deadline());
        if !self.settled {
            wake = wake.min(self.next_tick);
        }
        if self.preview
            && let Some(deadline) = self.throttle.deadline(now)
        {
            wake = wake.min(deadline);
        }
        let wait = wake.saturating_duration_since(now).min(MAX_SLEEP);
        if !wait.is_zero() {
            thread::sleep(wait);
        }
    }

    fn capture(&mut self, now: Instant) {
        if self.capturer.is_none() {
            if self.retry_at.is_some_and(|at| now < at) {
                return;
            }
            match Capturer::new(self.config.monitor_id.as_deref(), CAPTURE_MAX_DIMENSION) {
                Ok(capturer) => {
                    self.capturer = Some(capturer);
                    self.retry_at = None;
                }
                Err(error) => {
                    self.on_capture_error(&error, now);
                    return;
                }
            }
        }
        let Some(capturer) = self.capturer.as_mut() else {
            return;
        };
        match capturer.grab(self.config.region, GRAB_TIMEOUT) {
            Ok(Some(frame)) => {
                self.fps.record_frame();
                self.backoff.reset();
                self.set_error(None);
                self.last_reduced = reduce(&frame.view(), self.config.mode);
                self.recompute_target(now);
            }
            Ok(None) => {}
            Err(error) => self.on_capture_error(&error, now),
        }
    }

    fn on_capture_error(&mut self, error: &CaptureError, now: Instant) {
        self.capturer = None;
        if matches!(error, CaptureError::AccessLost) {
            log::debug!("capture access lost; recreating");
            self.retry_at = Some(now + self.backoff.next_delay());
        } else {
            self.retry_at = Some(now + ERROR_RETRY);
            self.set_error(Some(describe_capture_error(error)));
        }
    }

    fn set_error(&mut self, error: Option<String>) {
        {
            let mut shared = lock(&self.shared);
            if shared.last_error == error {
                return;
            }
            if let Some(message) = &error {
                log::warn!("engine error: {message}");
            }
            shared.last_error = error;
        }
        self.report();
    }

    fn recompute_target(&mut self, now: Instant) {
        let adjust = ColorAdjust::new(self.config.brightness, self.config.saturation);
        let adjusted = adjust.apply(self.last_reduced);
        self.target = adjusted;
        let rgb = adjusted.to_srgb();
        if self.has_target && rgb == self.target_rgb {
            return;
        }
        self.has_target = true;
        self.target_rgb = rgb;
        if self.settled {
            self.settled = false;
            self.last_tick = now.checked_sub(TICK_INTERVAL).unwrap_or(now);
            self.next_tick = now;
        }
        self.publish_colors();
    }

    fn tick(&mut self, now: Instant) {
        let dt = now.saturating_duration_since(self.last_tick).min(MAX_DT);
        self.last_tick = now;
        self.next_tick = advance_deadline(self.next_tick, now, TICK_INTERVAL);
        let time_constant = Duration::from_millis(u64::from(self.config.smoothing_ms));
        let smoothed = self.smoother.update(self.target, dt, time_constant);
        let rgb = smoothed.to_srgb();
        if rgb != self.smoothed_rgb {
            self.smoothed_rgb = rgb;
            self.publish_colors();
        }
        if self.last_sent != Some(rgb) {
            self.send_current();
        }
        self.settled = is_settled(rgb, self.target_rgb);
    }

    fn send_current(&mut self) {
        self.outputs.set_color(self.smoothed_rgb);
        self.last_sent = Some(self.smoothed_rgb);
    }

    fn publish_colors(&mut self) {
        {
            let mut shared = lock(&self.shared);
            shared.raw = self.target_rgb.to_hex();
            shared.smoothed = self.smoothed_rgb.to_hex();
        }
        if self.preview {
            self.throttle.mark();
        }
    }

    fn apply_config(&mut self, new: Config, now: Instant) {
        let old = std::mem::replace(&mut self.config, new);
        let config = &self.config;
        if old.monitor_id != config.monitor_id || old.region != config.region {
            // A fresh duplication session delivers the current frame even on a static desktop.
            self.capturer = None;
            self.retry_at = None;
            self.backoff.reset();
        }
        if old.capture_fps != config.capture_fps {
            self.next_capture = now;
        }
        let outputs_changed = old.outputs != config.outputs;
        let adjust_changed =
            old.brightness != config.brightness || old.saturation != config.saturation;
        if outputs_changed {
            self.outputs.apply_config(&config.outputs);
            self.last_sent = None;
            if self.has_target {
                self.send_current();
            }
        }
        if adjust_changed && self.has_target {
            self.recompute_target(now);
        }
    }

    fn set_running(&mut self, running: bool, now: Instant) {
        if running == self.running {
            return;
        }
        self.running = running;
        self.capturer = None;
        self.retry_at = None;
        self.backoff.reset();
        self.fps.reset(now);
        self.has_target = false;
        self.smoother.reset();
        self.settled = true;
        self.last_sent = None;
        self.next_capture = now;
        self.throttle.clear();
        {
            let mut shared = lock(&self.shared);
            shared.running = running;
            shared.measured_fps = 0.0;
            if !running {
                shared.last_error = None;
            }
        }
        self.outputs.set_paused(!running);
        self.report();
        if self.preview {
            self.throttle.mark();
        }
    }

    fn report(&mut self) {
        let snapshot = lock(&self.shared).clone();
        let key = ReportKey {
            running: snapshot.running,
            fps: snapshot.measured_fps.round() as u32,
            error: snapshot.last_error.clone(),
        };
        if self.reported.as_ref() != Some(&key) {
            self.reported = Some(key);
            self.events.status(&snapshot);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Instant {
        Instant::now()
    }

    #[test]
    fn capture_interval_matches_fps_and_clamps() {
        assert_eq!(capture_interval(20), Duration::from_millis(50));
        assert_eq!(capture_interval(0), Duration::from_secs(1));
        assert_eq!(capture_interval(1000), capture_interval(60));
    }

    #[test]
    fn deadline_keeps_cadence_when_slightly_late() {
        let t0 = base();
        let interval = Duration::from_millis(50);
        let now = t0 + Duration::from_millis(3);
        assert_eq!(advance_deadline(t0, now, interval), t0 + interval);
    }

    #[test]
    fn deadline_drops_missed_beats_when_far_behind() {
        let t0 = base();
        let interval = Duration::from_millis(50);
        let now = t0 + Duration::from_millis(400);
        assert_eq!(advance_deadline(t0, now, interval), now + interval);
    }

    #[test]
    fn settled_requires_equal_bytes() {
        assert!(is_settled(Rgb::new(1, 2, 3), Rgb::new(1, 2, 3)));
        assert!(!is_settled(Rgb::new(1, 2, 3), Rgb::new(1, 2, 4)));
    }

    #[test]
    fn backoff_doubles_up_to_max_and_resets() {
        let mut backoff = Backoff::new(RECOVERY_MIN, RECOVERY_MAX);
        let delays: Vec<_> = (0..6).map(|_| backoff.next_delay()).collect();
        let millis: Vec<_> = delays.iter().map(Duration::as_millis).collect();
        assert_eq!(millis, vec![250, 500, 1000, 2000, 2000, 2000]);
        backoff.reset();
        assert_eq!(backoff.next_delay(), RECOVERY_MIN);
    }

    #[test]
    fn throttle_releases_first_mark_immediately_then_limits() {
        let t0 = base();
        let mut throttle = Throttle::new(Duration::from_millis(50));
        assert!(!throttle.poll(t0));
        throttle.mark();
        assert!(throttle.poll(t0));
        throttle.mark();
        assert!(!throttle.poll(t0 + Duration::from_millis(10)));
        assert_eq!(
            throttle.deadline(t0 + Duration::from_millis(10)),
            Some(t0 + Duration::from_millis(50))
        );
        assert!(throttle.poll(t0 + Duration::from_millis(50)));
        assert_eq!(throttle.deadline(t0 + Duration::from_millis(60)), None);
    }

    #[test]
    fn throttle_clear_drops_pending() {
        let t0 = base();
        let mut throttle = Throttle::new(Duration::from_millis(50));
        throttle.mark();
        throttle.clear();
        assert!(!throttle.poll(t0));
    }

    #[test]
    fn fps_meter_reports_per_window_and_smooths() {
        let t0 = base();
        let mut meter = FpsMeter::new(t0);
        for _ in 0..20 {
            meter.record_frame();
        }
        assert_eq!(meter.poll(t0 + Duration::from_millis(500)), None);
        let first = meter.poll(t0 + Duration::from_secs(1)).unwrap();
        assert!((first - 20.0).abs() < 0.01);

        for _ in 0..10 {
            meter.record_frame();
        }
        let second = meter.poll(t0 + Duration::from_secs(2)).unwrap();
        assert!((second - 15.0).abs() < 0.01);
        assert_eq!(meter.deadline(), t0 + Duration::from_secs(3));
    }

    #[test]
    fn describe_covers_every_variant() {
        assert!(describe_capture_error(&CaptureError::MonitorNotFound).contains("monitor"));
        assert!(describe_capture_error(&CaptureError::Os("boom".into())).contains("boom"));
        assert_ne!(describe_capture_error(&CaptureError::Unsupported), "");
        assert_ne!(describe_capture_error(&CaptureError::AccessLost), "");
    }

    struct Recorder(Arc<Mutex<Vec<EngineStatus>>>);

    impl EngineEvents for Recorder {
        fn color(&self, _raw: Rgb, _smoothed: Rgb) {}
        fn status(&self, status: &EngineStatus) {
            lock(&self.0).push(status.clone());
        }
    }

    fn wait_until(mut condition: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if condition() {
                return true;
            }
            thread::sleep(Duration::from_millis(5));
        }
        false
    }

    #[test]
    fn thread_pauses_resumes_and_shuts_down() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let outputs = OutputsManager::new(Vec::new(), &Config::default().outputs, |_| {});
        let handle =
            EngineHandle::spawn(Config::default(), outputs, Recorder(Arc::clone(&seen))).unwrap();
        assert!(handle.status().running);

        handle.set_running(false);
        assert!(wait_until(|| {
            lock(&seen).last().is_some_and(|s| !s.running)
        }));
        assert!(!handle.status().running);
        assert_eq!(handle.status().measured_fps, 0.0);

        handle.set_running(true);
        assert!(wait_until(|| {
            lock(&seen).last().is_some_and(|s| s.running)
        }));

        handle.apply_config(Config {
            capture_fps: 5,
            ..Config::default()
        });
        handle.set_preview_enabled(true);
        handle.shutdown();
        handle.shutdown();
    }

    #[test]
    fn default_status_is_idle() {
        let status = EngineStatus::default();
        assert!(!status.running);
        assert_eq!(status.raw, "#000000");
    }
}
