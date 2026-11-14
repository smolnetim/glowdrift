//! The smoothing rate follows the lights and the preview: tick counts per second match the rule,
//! nothing ticks while nothing consumes the colour, and the smoothed colour converges in the same
//! wall time at every rate.
//!
//! The engine runs on a synthetic clock advanced one millisecond per `Engine::step`. Ticks are
//! counted as changes of the shared smoothed colour, which moves on every tick while it eases.

use std::sync::atomic::{AtomicU8, AtomicU32, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use glowdrift_capture::CaptureError;
use glowdrift_core::{Config, FrameView, IcueConfig, LightsConfig, NormRect, Rgb};
use glowdrift_desktop_lib::engine::{Engine, EngineCommand, EngineEvents, EngineStatus};
use glowdrift_desktop_lib::frame_source::{FrameSource, OpenSource};
use glowdrift_desktop_lib::lights_manager::LightsManager;
use glowdrift_lights::icue::ICUE_LIGHT_ID;
use glowdrift_lights::{Light, LightError, LightState, LightStatus};

const STEP: Duration = Duration::from_millis(1);
/// Slow enough that the 8-bit smoothed colour changes on every tick for the first seconds.
const SMOOTHING_MS: u32 = 2000;

/// A uniform grey frame whose level the test controls.
struct Grey {
    level: Arc<AtomicU8>,
    pixels: Vec<u8>,
}

impl FrameSource for Grey {
    fn grab(
        &mut self,
        _region: NormRect,
        _timeout: Duration,
    ) -> Result<Option<FrameView<'_>>, CaptureError> {
        let level = self.level.load(Ordering::Relaxed);
        for pixel in self.pixels.as_chunks_mut::<4>().0 {
            *pixel = [level, level, level, 255];
        }
        Ok(Some(FrameView::new(&self.pixels, 4, 4, 16)))
    }
}

/// A light with a declared rate limit that counts the colours it receives.
struct RateLight {
    interval: Option<Duration>,
    sends: Arc<AtomicU32>,
}

impl Light for RateLight {
    fn id(&self) -> &str {
        ICUE_LIGHT_ID
    }
    fn status(&self) -> LightStatus {
        LightStatus {
            id: ICUE_LIGHT_ID.to_owned(),
            kind: "icue".to_owned(),
            name: "Rate".to_owned(),
            enabled: true,
            state: LightState::Connected,
            detail: None,
            device_count: 1,
            code: None,
        }
    }
    fn set_enabled(&mut self, _enabled: bool) {}
    fn min_send_interval(&self) -> Option<Duration> {
        self.interval
    }
    fn set_color(&mut self, _color: Rgb) -> Result<(), LightError> {
        self.sends.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
    fn shutdown(&mut self) {}
}

struct Quiet;

impl EngineEvents for Quiet {
    fn color(&self, _target: Rgb, _smoothed: Rgb) {}
    fn status(&self, _status: &EngineStatus) {}
}

fn lights_config(enabled: bool) -> LightsConfig {
    LightsConfig {
        icue: IcueConfig { enabled },
    }
}

fn config(light_enabled: bool) -> Config {
    Config {
        capture_fps: 20,
        smoothing_ms: SMOOTHING_MS,
        lights: lights_config(light_enabled),
        ..Config::default()
    }
}

struct Rig {
    engine: Engine<Quiet>,
    shared: Arc<Mutex<EngineStatus>>,
    level: Arc<AtomicU8>,
    sends: Arc<AtomicU32>,
    now: Instant,
    start: Instant,
}

impl Rig {
    /// `interval` is the light's declared limit; the light is wanted when `light_enabled`.
    fn new(interval: Option<Duration>, light_enabled: bool) -> Rig {
        let level = Arc::new(AtomicU8::new(0));
        let sends = Arc::new(AtomicU32::new(0));
        let lights = LightsManager::new(
            vec![Box::new(RateLight {
                interval,
                sends: Arc::clone(&sends),
            })],
            &lights_config(light_enabled),
            |_| {},
        );
        let open: OpenSource = {
            let level = Arc::clone(&level);
            Box::new(move |_, _| {
                Ok(Box::new(Grey {
                    level: Arc::clone(&level),
                    pixels: vec![0; 64],
                }))
            })
        };
        let (_commands, rx) = mpsc::channel();
        let shared = Arc::new(Mutex::new(EngineStatus::default()));
        let engine = Engine::new(
            rx,
            Arc::clone(&shared),
            lights,
            Quiet,
            config(light_enabled),
            open,
        );
        let start = Instant::now();
        Rig {
            engine,
            shared,
            level,
            sends,
            now: start,
            start,
        }
    }

    fn with_preview(mut self) -> Rig {
        self.command(EngineCommand::SetPreviewEnabled(true));
        self
    }

    fn command(&mut self, command: EngineCommand) {
        assert!(self.engine.handle(command));
    }

    fn smoothed(&self) -> String {
        self.shared.lock().unwrap().smoothed.clone()
    }

    fn red(&self) -> u8 {
        u8::from_str_radix(&self.smoothed()[1..3], 16).unwrap()
    }

    fn step(&mut self) {
        self.engine.step(self.now);
        self.now += STEP;
    }

    fn elapsed(&self) -> Duration {
        self.now.duration_since(self.start)
    }

    /// Steps for `span`, returning how many times the smoothed colour changed.
    fn run(&mut self, span: Duration) -> u32 {
        let mut changes = 0;
        let mut last = self.smoothed();
        for _ in 0..span.as_millis() {
            self.step();
            let current = self.smoothed();
            if current != last {
                changes += 1;
                last = current;
            }
        }
        changes
    }

    /// Settles on black, then switches the picture to white and returns the number of smoothed
    /// colour changes over the following second.
    fn ticks_in_the_second_after_a_flip(&mut self) -> u32 {
        self.level.store(0, Ordering::Relaxed);
        self.run(Duration::from_millis(400));
        self.level.store(255, Ordering::Relaxed);
        self.run(Duration::from_secs(1))
    }
}

fn ms(value: u64) -> Duration {
    Duration::from_millis(value)
}

fn assert_near(actual: u32, expected: u32, what: &str) {
    assert!(
        actual.abs_diff(expected) <= 2,
        "{what}: {actual} ticks in one second, expected about {expected}"
    );
}

#[test]
fn ticks_at_the_rate_of_the_fastest_light() {
    assert_near(
        Rig::new(Some(ms(33)), true).ticks_in_the_second_after_a_flip(),
        30,
        "light at 33 ms",
    );
    assert_near(
        Rig::new(None, true).ticks_in_the_second_after_a_flip(),
        60,
        "light without a limit",
    );
    assert_near(
        Rig::new(Some(ms(100)), true).ticks_in_the_second_after_a_flip(),
        10,
        "light at 100 ms",
    );
}

#[test]
fn preview_ticks_at_twenty_hertz_unless_a_light_is_faster() {
    assert_near(
        Rig::new(Some(ms(33)), false)
            .with_preview()
            .ticks_in_the_second_after_a_flip(),
        20,
        "preview only",
    );
    assert_near(
        Rig::new(Some(ms(33)), true)
            .with_preview()
            .ticks_in_the_second_after_a_flip(),
        30,
        "preview and a 33 ms light",
    );
    assert_near(
        Rig::new(Some(ms(100)), true)
            .with_preview()
            .ticks_in_the_second_after_a_flip(),
        20,
        "preview and a 100 ms light",
    );
}

#[test]
fn does_not_tick_while_nothing_consumes_the_colour() {
    let mut rig = Rig::new(Some(ms(33)), false);
    // The smoothed colour follows the target at once, in a single change.
    assert_eq!(rig.ticks_in_the_second_after_a_flip(), 1);
    assert_eq!(rig.smoothed(), "#ffffff");
    assert_eq!(rig.shared.lock().unwrap().target, "#ffffff");
    assert_eq!(rig.sends.load(Ordering::Relaxed), 0);
}

#[test]
fn enabling_or_disabling_a_consumer_continues_from_the_current_picture() {
    let mut rig = Rig::new(Some(ms(33)), false);
    rig.ticks_in_the_second_after_a_flip();
    assert_eq!(rig.smoothed(), "#ffffff");

    // A newly enabled light is sent the current colour and eases from there.
    rig.command(EngineCommand::ApplyConfig(Box::new(config(true))));
    assert_eq!(rig.sends.load(Ordering::Relaxed), 1);
    rig.level.store(0, Ordering::Relaxed);
    assert_near(rig.run(Duration::from_secs(1)), 30, "enabled light");
    assert!(rig.red() > 0, "still easing towards black");

    // Disabling it stops the ticking and lands on the target.
    rig.command(EngineCommand::ApplyConfig(Box::new(config(false))));
    assert_eq!(rig.smoothed(), "#000000");
    rig.level.store(255, Ordering::Relaxed);
    assert_eq!(rig.run(Duration::from_secs(1)), 1);
    assert_eq!(rig.smoothed(), "#ffffff");

    // The preview alone brings the ticking back, at its own rate.
    rig.command(EngineCommand::SetPreviewEnabled(true));
    rig.level.store(0, Ordering::Relaxed);
    assert_near(rig.run(Duration::from_secs(1)), 20, "enabled preview");

    // Turning it off mid-ease lands on the target and goes quiet.
    rig.level.store(255, Ordering::Relaxed);
    rig.run(ms(100));
    rig.command(EngineCommand::SetPreviewEnabled(false));
    assert_eq!(rig.smoothed(), "#ffffff");
    assert_eq!(rig.run(Duration::from_secs(1)), 0);
}

/// Time from the picture flipping to white until the smoothed red channel reaches 200.
fn time_to_reach_200(mut rig: Rig) -> Duration {
    rig.level.store(0, Ordering::Relaxed);
    rig.run(ms(400));
    rig.level.store(255, Ordering::Relaxed);
    let flipped = rig.elapsed();
    for _ in 0..8000 {
        rig.step();
        if rig.red() >= 200 {
            return rig.elapsed().saturating_sub(flipped);
        }
    }
    panic!("never reached the target");
}

#[test]
fn converges_in_the_same_wall_time_at_every_rate() {
    let times = [
        time_to_reach_200(Rig::new(None, true)),
        time_to_reach_200(Rig::new(Some(ms(33)), true)),
        time_to_reach_200(Rig::new(Some(ms(100)), true)),
        time_to_reach_200(Rig::new(Some(ms(33)), false).with_preview()),
    ];
    // A time constant of 2 s puts 200/255 (0.58 in linear light) at about 1.73 s, plus up to
    // one capture period for the flip to be seen.
    for time in times {
        assert!(
            time > ms(1650) && time < ms(1900),
            "reached 200 after {time:?}, expected about 1730-1780 ms"
        );
    }
    let fastest = times.iter().min().unwrap();
    let slowest = times.iter().max().unwrap();
    assert!(
        slowest.saturating_sub(*fastest) <= ms(110),
        "rates disagree on convergence time: {times:?}"
    );
}
