//! The colour engine: capture -> reduce -> smooth -> lights on one dedicated thread.
//!
//! Capture runs at `captureFps`. The smoother ticks at a smoothing rate that is independent of the
//! capture rate: the fastest rate the active lights or the preview can use, never above
//! [`MIN_TICK_INTERVAL`]. It stops once the smoothed colour has reached the target colour, and does
//! not run at all while nothing consumes the colour.

use std::io;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use glowdrift_capture::CaptureError;
use glowdrift_core::{ColorAdjust, Config, LinearRgb, Rgb, Smoother, reduce};
use serde::{Deserialize, Serialize};

use crate::frame_source::{FrameSource, OpenSource, open_desktop};
use crate::lights_manager::LightsManager;
use crate::timing::{Stage, Timing};

/// Shortest gap between smoother ticks, which caps the smoothing rate at about 60 Hz.
const MIN_TICK_INTERVAL: Duration = Duration::from_micros(16_667);
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
    /// Target colour as `#rrggbb`.
    pub target: String,
    /// Smoothed colour as `#rrggbb`.
    pub smoothed: String,
}

impl Default for EngineStatus {
    fn default() -> Self {
        Self {
            running: false,
            measured_fps: 0.0,
            last_error: None,
            target: IDLE_COLOR.to_owned(),
            smoothed: IDLE_COLOR.to_owned(),
        }
    }
}

/// Receiver of engine notifications. Implementations must not block.
pub trait EngineEvents: Send + 'static {
    /// Throttled colour preview, only called while previews are enabled.
    fn color(&self, target: Rgb, smoothed: Rgb);
    /// Called when the running flag, the error, or the rounded measured capture rate changes.
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

/// Interval between captures for a capture rate, clamped to the supported range.
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

/// True when the 8-bit smoothed colour has reached the 8-bit target, so smoothing can stop ticking.
pub fn is_settled(smoothed: Rgb, target: Rgb) -> bool {
    smoothed == target
}

/// Gap between smoother ticks for the fastest active light and the preview, or `None` when neither
/// consumes the smoothed colour. `lights` is [`LightsManager::fastest_interval`].
pub fn smoothing_interval(lights: Option<Duration>, preview: bool) -> Option<Duration> {
    lights
        .into_iter()
        .chain(preview.then_some(PREVIEW_INTERVAL))
        .min()
        .map(|interval| interval.max(MIN_TICK_INTERVAL))
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

/// Meter of the achieved capture rate over fixed windows, lightly smoothed.
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
        lights: Arc<LightsManager>,
        events: impl EngineEvents,
    ) -> io::Result<Self> {
        let (tx, rx) = mpsc::channel();
        let status = Arc::new(Mutex::new(EngineStatus {
            running: true,
            ..EngineStatus::default()
        }));
        let engine = Engine::new(
            rx,
            Arc::clone(&status),
            lights,
            events,
            config,
            open_desktop(),
        );
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

    /// Stops the thread, releasing lights, and waits for it. Safe to call repeatedly.
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

/// The engine's state and loop. [`EngineHandle`] runs one on its own thread; tests drive
/// [`Engine::step`] directly on a clock they control.
#[allow(clippy::struct_excessive_bools)]
pub struct Engine<E: EngineEvents> {
    rx: Receiver<EngineCommand>,
    shared: Arc<Mutex<EngineStatus>>,
    lights: Arc<LightsManager>,
    events: E,
    config: Config,
    running: bool,
    preview: bool,

    open: OpenSource,
    capturer: Option<Box<dyn FrameSource>>,
    retry_at: Option<Instant>,
    backoff: Backoff,

    /// Gap between smoother ticks; `None` while nothing consumes the smoothed colour.
    tick_interval: Option<Duration>,
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
    /// The shared status holds an error, so clearing it has work to do.
    error_shown: bool,
    timing: Timing,
}

impl<E: EngineEvents> std::fmt::Debug for Engine<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Engine").finish_non_exhaustive()
    }
}

impl<E: EngineEvents> Engine<E> {
    pub fn new(
        rx: Receiver<EngineCommand>,
        shared: Arc<Mutex<EngineStatus>>,
        lights: Arc<LightsManager>,
        events: E,
        config: Config,
        open: OpenSource,
    ) -> Self {
        let now = Instant::now();
        lights.apply_config(&config.lights);
        let tick_interval = smoothing_interval(lights.fastest_interval(), false);
        Self {
            rx,
            shared,
            lights,
            events,
            config,
            running: true,
            preview: false,
            open,
            capturer: None,
            retry_at: None,
            backoff: Backoff::new(RECOVERY_MIN, RECOVERY_MAX),
            tick_interval,
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
            error_shown: false,
            timing: Timing::new(),
        }
    }

    fn run(mut self) {
        self.timing.begin();
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
            let pass = Timing::start_cpu();
            self.step(now);
            self.timing.record(Stage::Pass, pass);
            self.sleep_until_next(now);
            self.timing.poll();
        }
        self.timing.finish();
        self.lights.shutdown();
    }

    /// Returns false when the engine must stop.
    pub fn handle(&mut self, command: EngineCommand) -> bool {
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
                    self.refresh_tick_interval();
                }
            }
            EngineCommand::Shutdown => return false,
        }
        true
    }

    /// Runs one pass of the loop as of `now`: capture, smoothing and preview when due.
    pub fn step(&mut self, now: Instant) {
        let mark = Timing::start();
        let resend = self.lights.take_resend();
        self.timing.record(Stage::Resend, mark);
        if resend {
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
        if let Some(interval) = self.tick_interval
            && !self.settled
            && now >= self.next_tick
        {
            self.tick(now, interval);
        }
        if self.preview {
            let mark = Timing::start();
            if self.throttle.poll(now) {
                self.events.color(self.target_rgb, self.smoothed_rgb);
                self.timing.record(Stage::Preview, mark);
            }
        }
        let mark = Timing::start();
        if let Some(fps) = self.fps.poll(now) {
            lock(&self.shared).measured_fps = fps;
            self.report();
            self.timing.record(Stage::Meter, mark);
        }
    }

    fn sleep_until_next(&mut self, now: Instant) {
        let mut wake = self.next_capture.min(self.fps.deadline());
        if self.tick_interval.is_some() && !self.settled {
            wake = wake.min(self.next_tick);
        }
        if self.preview
            && let Some(deadline) = self.throttle.deadline(now)
        {
            wake = wake.min(deadline);
        }
        let wait = wake.saturating_duration_since(now).min(MAX_SLEEP);
        if !wait.is_zero() {
            let mark = Timing::start();
            thread::sleep(wait);
            self.timing.record_sleep(wait, mark);
        }
    }

    fn capture(&mut self, now: Instant) {
        if self.capturer.is_none() {
            if self.retry_at.is_some_and(|at| now < at) {
                return;
            }
            match (self.open)(self.config.monitor_id.as_deref(), CAPTURE_MAX_DIMENSION) {
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
        let mark = Timing::start_cpu();
        let grabbed = capturer.grab(self.config.region, GRAB_TIMEOUT);
        self.timing.record(Stage::Grab, mark);
        match grabbed {
            Ok(Some(frame)) => {
                self.timing.frame();
                self.fps.record_frame();
                self.backoff.reset();
                let mark = Timing::start_cpu();
                self.last_reduced = reduce(&frame, self.config.mode);
                self.timing.record(Stage::Reduce, mark);
                self.note_grab_timings();
                self.set_error(None);
                self.recompute_target(now);
            }
            Ok(None) => self.note_grab_timings(),
            Err(error) => self.on_capture_error(&error, now),
        }
    }

    /// Feeds the capturer's internal split of the last grab to the timer.
    fn note_grab_timings(&mut self) {
        if Timing::ENABLED
            && let Some(timings) = self.capturer.as_ref().and_then(|c| c.last_timings())
        {
            self.timing.grab_detail(timings);
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
        if error.is_none() && !self.error_shown {
            return;
        }
        let mark = Timing::start();
        {
            let mut shared = lock(&self.shared);
            self.error_shown = error.is_some();
            if shared.last_error == error {
                drop(shared);
                self.timing.record(Stage::Error, mark);
                return;
            }
            if let Some(message) = &error {
                log::warn!("engine error: {message}");
            }
            shared.last_error = error;
        }
        self.report();
        self.timing.record(Stage::Error, mark);
    }

    fn recompute_target(&mut self, now: Instant) {
        let mark = Timing::start();
        let adjust = ColorAdjust::new(self.config.brightness, self.config.saturation);
        let adjusted = adjust.apply(self.last_reduced);
        self.target = adjusted;
        let rgb = adjusted.to_srgb();
        self.timing.record(Stage::Target, mark);
        if self.has_target && rgb == self.target_rgb {
            return;
        }
        self.has_target = true;
        self.target_rgb = rgb;
        match self.tick_interval {
            Some(interval) => {
                if self.settled {
                    self.settled = false;
                    self.last_tick = now.checked_sub(interval).unwrap_or(now);
                    self.next_tick = now;
                }
            }
            None => self.jump_to_target(),
        }
        self.publish_colors();
    }

    fn tick(&mut self, now: Instant, interval: Duration) {
        let dt = now
            .saturating_duration_since(self.last_tick)
            .min(MAX_DT.max(interval * 2));
        self.last_tick = now;
        self.next_tick = advance_deadline(self.next_tick, now, interval);
        let mark = Timing::start();
        let time_constant = Duration::from_millis(u64::from(self.config.smoothing_ms));
        let smoothed = self.smoother.update(self.target, dt, time_constant);
        let rgb = smoothed.to_srgb();
        self.timing.record(Stage::Tick, mark);
        if rgb != self.smoothed_rgb {
            self.smoothed_rgb = rgb;
            self.publish_colors();
        }
        if self.last_sent != Some(rgb) {
            self.send_current();
        }
        self.settled = is_settled(rgb, self.target_rgb);
    }

    /// Recomputes the smoothing rate after the lights or the preview changed. When nothing
    /// consumes the colour any more, the smoothed colour jumps to the target colour, so a light
    /// enabled later starts from the current picture rather than from a stale colour.
    fn refresh_tick_interval(&mut self) {
        self.tick_interval = smoothing_interval(self.lights.fastest_interval(), self.preview);
        if self.tick_interval.is_none() && self.has_target {
            self.jump_to_target();
            self.publish_colors();
        }
    }

    /// Puts the smoother and the smoothed colour on the target colour and stops ticking.
    fn jump_to_target(&mut self) {
        let time_constant = Duration::from_millis(u64::from(self.config.smoothing_ms));
        self.smoother.reset();
        self.smoother
            .update(self.target, Duration::ZERO, time_constant);
        self.smoothed_rgb = self.target_rgb;
        self.settled = true;
    }

    fn send_current(&mut self) {
        let mark = Timing::start();
        self.lights.set_color(self.smoothed_rgb);
        self.last_sent = Some(self.smoothed_rgb);
        self.timing.record(Stage::Send, mark);
    }

    fn publish_colors(&mut self) {
        let mark = Timing::start();
        {
            let mut shared = lock(&self.shared);
            self.target_rgb.write_hex(&mut shared.target);
            self.smoothed_rgb.write_hex(&mut shared.smoothed);
        }
        if self.preview {
            self.throttle.mark();
        }
        self.timing.record(Stage::Publish, mark);
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
        let lights_changed = old.lights != config.lights;
        let adjust_changed =
            old.brightness != config.brightness || old.saturation != config.saturation;
        if lights_changed {
            self.lights.apply_config(&config.lights);
            self.last_sent = None;
            if self.has_target {
                self.send_current();
            }
        }
        self.refresh_tick_interval();
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
        self.lights.set_paused(!running);
        self.refresh_tick_interval();
        self.report();
        if self.preview {
            self.throttle.mark();
        }
    }

    fn report(&mut self) {
        let shared = lock(&self.shared);
        let fps = shared.measured_fps.round() as u32;
        let unchanged = self.reported.as_ref().is_some_and(|key| {
            key.running == shared.running && key.fps == fps && key.error == shared.last_error
        });
        if unchanged {
            return;
        }
        let snapshot = shared.clone();
        drop(shared);
        self.reported = Some(ReportKey {
            running: snapshot.running,
            fps,
            error: snapshot.last_error.clone(),
        });
        self.events.status(&snapshot);
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
    fn smoothing_interval_picks_the_fastest_consumer() {
        let ms = Duration::from_millis;
        // Nothing consumes the colour.
        assert_eq!(smoothing_interval(None, false), None);
        // One 30 Hz light, or the 20 Hz preview, alone.
        assert_eq!(smoothing_interval(Some(ms(33)), false), Some(ms(33)));
        assert_eq!(smoothing_interval(None, true), Some(PREVIEW_INTERVAL));
        // Both: the faster of the two.
        assert_eq!(smoothing_interval(Some(ms(33)), true), Some(ms(33)));
        assert_eq!(
            smoothing_interval(Some(ms(100)), true),
            Some(PREVIEW_INTERVAL)
        );
        // A light without a limit is capped at the maximum tick rate.
        assert_eq!(
            smoothing_interval(Some(Duration::ZERO), false),
            Some(MIN_TICK_INTERVAL)
        );
        assert_eq!(
            smoothing_interval(Some(Duration::ZERO), true),
            Some(MIN_TICK_INTERVAL)
        );
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
        fn color(&self, _target: Rgb, _smoothed: Rgb) {}
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
        let lights = LightsManager::new(Vec::new(), &Config::default().lights, |_| {});
        let handle =
            EngineHandle::spawn(Config::default(), lights, Recorder(Arc::clone(&seen))).unwrap();
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
    fn error_is_shown_until_the_next_frame_clears_it() {
        struct Flat(Vec<u8>);

        impl FrameSource for Flat {
            fn grab(
                &mut self,
                _region: glowdrift_core::NormRect,
                _timeout: Duration,
            ) -> Result<Option<glowdrift_core::FrameView<'_>>, CaptureError> {
                Ok(Some(glowdrift_core::FrameView::new(&self.0, 2, 2, 8)))
            }
        }

        let mut attempts = 0;
        let open: OpenSource = Box::new(move |_, _| {
            attempts += 1;
            if attempts == 1 {
                Err(CaptureError::Os("boom".into()))
            } else {
                Ok(Box::new(Flat(vec![255; 16])))
            }
        });
        let shared = Arc::new(Mutex::new(EngineStatus::default()));
        let lights = LightsManager::new(Vec::new(), &Config::default().lights, |_| {});
        let (_commands, rx) = mpsc::channel();
        let events = Recorder(Arc::default());
        let mut engine = Engine::new(
            rx,
            Arc::clone(&shared),
            lights,
            events,
            Config::default(),
            open,
        );

        let t0 = base();
        engine.step(t0);
        assert!(
            lock(&shared)
                .last_error
                .as_deref()
                .is_some_and(|e| e.contains("boom"))
        );
        engine.step(t0 + ERROR_RETRY + Duration::from_millis(1));
        assert_eq!(lock(&shared).last_error, None);
        assert_eq!(lock(&shared).target, "#ffffff");
    }

    #[test]
    fn default_status_is_idle() {
        let status = EngineStatus::default();
        assert!(!status.running);
        assert_eq!(status.target, "#000000");
    }
}
