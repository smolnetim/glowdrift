//! Per-light zones: the grab rectangle, independent zone colours, per-zone change detection and the
//! fan-out of whole and zone colours to the lights.
//!
//! The engine runs on a synthetic clock. The frame is split into a left and a right half whose
//! colours the test controls.

use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use glowdrift_capture::CaptureError;
use glowdrift_core::{Config, FrameView, HueConfig, LightsConfig, NormRect, Rgb, bounding_rect};
use glowdrift_desktop_lib::engine::{Engine, EngineCommand, EngineEvents, EngineStatus};
use glowdrift_desktop_lib::frame_source::{FrameSource, OpenSource};
use glowdrift_desktop_lib::lights_manager::LightsManager;
use glowdrift_lights::hue::HUE_LIGHT_ID;
use glowdrift_lights::icue::ICUE_LIGHT_ID;
use glowdrift_lights::{Light, LightError, LightState, LightStatus, ZoneLayout};

const RED: Rgb = Rgb::new(255, 0, 0);
const BLUE: Rgb = Rgb::new(0, 0, 255);
const STEP: Duration = Duration::from_millis(25);

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap()
}

/// A frame whose left half and right half are uniform colours.
struct Halves {
    colors: Arc<Mutex<(Rgb, Rgb)>>,
    grabs: Arc<Mutex<Vec<NormRect>>>,
    pixels: Vec<u8>,
}

impl FrameSource for Halves {
    fn grab(
        &mut self,
        region: NormRect,
        _timeout: Duration,
    ) -> Result<Option<FrameView<'_>>, CaptureError> {
        lock(&self.grabs).push(region);
        let (left, right) = *lock(&self.colors);
        for (x, pixel) in self.pixels.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let c = if x % 8 < 4 { left } else { right };
            *pixel = [c.b, c.g, c.r, 255];
        }
        Ok(Some(FrameView::new(&self.pixels, 8, 1, 32)))
    }
}

type Received = Arc<Mutex<Vec<(Option<u64>, Rgb, Vec<Rgb>)>>>;

/// A light that records every colour call; with a layout it asks for zone colours.
struct Recorder {
    id: &'static str,
    layout: Arc<Mutex<Option<ZoneLayout>>>,
    received: Received,
}

impl Light for Recorder {
    fn id(&self) -> &str {
        self.id
    }
    fn status(&self) -> LightStatus {
        LightStatus {
            id: self.id.to_owned(),
            kind: self.id.to_owned(),
            name: "Recorder".to_owned(),
            enabled: true,
            state: LightState::Connected,
            detail: None,
            device_count: 1,
            code: None,
        }
    }
    fn set_enabled(&mut self, _enabled: bool) {}
    fn min_send_interval(&self) -> Option<Duration> {
        None
    }
    fn set_color(&mut self, color: Rgb) -> Result<(), LightError> {
        lock(&self.received).push((None, color, Vec::new()));
        Ok(())
    }
    fn zone_layout(&self) -> Option<ZoneLayout> {
        lock(&self.layout).clone()
    }
    fn set_zone_colors(
        &mut self,
        generation: u64,
        whole: Rgb,
        zones: &[Rgb],
    ) -> Result<(), LightError> {
        lock(&self.received).push((Some(generation), whole, zones.to_vec()));
        Ok(())
    }
    fn shutdown(&mut self) {}
}

#[derive(Default)]
struct Events {
    zones: Arc<Mutex<Vec<Vec<Rgb>>>>,
}

impl EngineEvents for Events {
    fn color(&self, _target: Rgb, _smoothed: Rgb, zones: &[Rgb]) {
        lock(&self.zones).push(zones.to_vec());
    }
    fn status(&self, _status: &EngineStatus) {}
}

struct Rig {
    engine: Engine<Events>,
    lights: Arc<LightsManager>,
    colors: Arc<Mutex<(Rgb, Rgb)>>,
    grabs: Arc<Mutex<Vec<NormRect>>>,
    hue: Received,
    icue: Received,
    layout: Arc<Mutex<Option<ZoneLayout>>>,
    preview: Arc<Mutex<Vec<Vec<Rgb>>>>,
    now: Instant,
}

fn left_zone() -> NormRect {
    NormRect::new(0.0, 0.0, 0.5, 1.0)
}

fn right_zone() -> NormRect {
    NormRect::new(0.5, 0.0, 0.5, 1.0)
}

impl Rig {
    fn new(config: Config, zones: Option<Vec<NormRect>>) -> Rig {
        let colors = Arc::new(Mutex::new((RED, BLUE)));
        let grabs = Arc::new(Mutex::new(Vec::new()));
        let hue = Received::default();
        let icue = Received::default();
        let layout = Arc::new(Mutex::new(zones.map(|zones| ZoneLayout {
            generation: 7,
            zones,
        })));
        let lights_config = LightsConfig {
            hue: HueConfig {
                enabled: true,
                ..HueConfig::default()
            },
            ..LightsConfig::default()
        };
        let lights = LightsManager::new(
            vec![
                Box::new(Recorder {
                    id: ICUE_LIGHT_ID,
                    layout: Arc::default(),
                    received: Arc::clone(&icue),
                }),
                Box::new(Recorder {
                    id: HUE_LIGHT_ID,
                    layout: Arc::clone(&layout),
                    received: Arc::clone(&hue),
                }),
            ],
            &lights_config,
            |_| {},
        );
        let open: OpenSource = {
            let (colors, grabs) = (Arc::clone(&colors), Arc::clone(&grabs));
            Box::new(move |_, _| {
                Ok(Box::new(Halves {
                    colors: Arc::clone(&colors),
                    grabs: Arc::clone(&grabs),
                    pixels: vec![0; 32],
                }))
            })
        };
        let events = Events::default();
        let preview = Arc::clone(&events.zones);
        let (_commands, rx) = mpsc::channel();
        let shared = Arc::new(Mutex::new(EngineStatus::default()));
        let config = Config {
            lights: lights_config,
            ..config
        };
        let mut engine = Engine::new(rx, shared, Arc::clone(&lights), events, config, open);
        assert!(engine.handle(EngineCommand::SetPreviewEnabled(true)));
        Rig {
            engine,
            lights,
            colors,
            grabs,
            hue,
            icue,
            layout,
            preview,
            now: Instant::now(),
        }
    }

    fn run(&mut self, steps: u32) {
        for _ in 0..steps {
            self.engine.step(self.now);
            self.now += STEP;
        }
    }

    fn last_hue(&self) -> (Option<u64>, Rgb, Vec<Rgb>) {
        lock(&self.hue)
            .last()
            .cloned()
            .expect("hue received nothing")
    }

    fn last_grab(&self) -> NormRect {
        *lock(&self.grabs).last().expect("nothing grabbed")
    }
}

fn config() -> Config {
    Config {
        capture_fps: 20,
        smoothing_ms: 0,
        region: NormRect::new(0.25, 0.0, 0.5, 1.0),
        ..Config::default()
    }
}

#[test]
fn grabs_exactly_the_region_without_zones() {
    let mut rig = Rig::new(config(), None);
    rig.run(10);
    assert_eq!(rig.last_grab(), config().region);
    // Lights without a layout only get the whole colour.
    let (generation, whole, zones) = rig.last_hue();
    assert_eq!(generation, None);
    assert_eq!(zones, Vec::<Rgb>::new());
    assert_eq!(whole, lock(&rig.icue).last().unwrap().1);
    assert!(lock(&rig.preview).iter().all(Vec::is_empty));
}

#[test]
fn grabs_the_bounding_rectangle_of_region_and_zones() {
    let mut rig = Rig::new(config(), Some(vec![left_zone(), right_zone()]));
    rig.run(10);
    assert_eq!(
        rig.last_grab(),
        bounding_rect(&[config().region, left_zone(), right_zone()])
    );
    let narrow = NormRect::new(0.6, 0.1, 0.2, 0.2);
    let mut rig = Rig::new(config(), Some(vec![narrow]));
    rig.run(10);
    assert_eq!(rig.last_grab(), bounding_rect(&[config().region, narrow]));
}

#[test]
fn zones_get_their_own_colours_and_the_light_its_slice() {
    let mut rig = Rig::new(config(), Some(vec![left_zone(), right_zone()]));
    rig.run(10);
    let (generation, whole, zones) = rig.last_hue();
    assert_eq!(generation, Some(7));
    assert_eq!(zones, vec![RED, BLUE]);
    // The whole-region colour is the mix of the two halves.
    assert_ne!(whole, RED);
    assert_ne!(whole, BLUE);
    assert_eq!(lock(&rig.icue).last().unwrap().1, whole);
    assert!(lock(&rig.icue).iter().all(|(g, ..)| g.is_none()));
    assert_eq!(lock(&rig.preview).last().unwrap(), &vec![RED, BLUE]);
}

#[test]
fn a_change_in_only_one_zone_is_sent() {
    let mut rig = Rig::new(config(), Some(vec![left_zone(), right_zone()]));
    rig.run(10);
    let (_, whole, _) = rig.last_hue();

    // Swapping the halves leaves the whole-region colour as it is and moves both zones.
    *lock(&rig.colors) = (BLUE, RED);
    rig.run(10);
    let (_, swapped_whole, zones) = rig.last_hue();
    assert_eq!(zones, vec![BLUE, RED]);
    assert_eq!(swapped_whole, whole);

    // Nothing changes: nothing is sent.
    let sent = lock(&rig.hue).len();
    rig.run(20);
    assert_eq!(lock(&rig.hue).len(), sent);
}

#[test]
fn zones_ease_independently_with_the_same_time_constant() {
    let config = Config {
        smoothing_ms: 1000,
        ..config()
    };
    let mut rig = Rig::new(config, Some(vec![left_zone(), right_zone()]));
    rig.run(10);
    // Only the right half changes.
    *lock(&rig.colors) = (RED, RED);
    rig.run(8);
    let (_, _, zones) = rig.last_hue();
    assert_eq!(zones[0], RED);
    assert!(zones[1].r > 0 && zones[1].r < 255, "right zone is easing");
    assert!(zones[1].b > 0 && zones[1].b < 255, "right zone is easing");
    rig.run(400);
    assert_eq!(rig.last_hue().2, vec![RED, RED]);
}

#[test]
fn set_zones_command_changes_the_grab_and_the_zone_count() {
    let edge = NormRect::new(0.0, 0.0, 0.25, 1.0);
    let mut rig = Rig::new(config(), Some(vec![edge]));
    rig.run(10);
    assert_eq!(rig.last_hue().2, vec![RED]);

    // The light grows a second zone: the manager reports it and the engine follows.
    *lock(&rig.layout) = Some(ZoneLayout {
        generation: 8,
        zones: vec![left_zone(), right_zone()],
    });
    let pushed: Arc<Mutex<Vec<Vec<NormRect>>>> = Arc::default();
    {
        let pushed = Arc::clone(&pushed);
        rig.lights
            .set_zone_sink(move |zones| lock(&pushed).push(zones));
    }
    rig.lights.poll();
    assert_eq!(*lock(&pushed), vec![vec![left_zone(), right_zone()]]);
    rig.lights.poll();
    assert_eq!(
        lock(&pushed).len(),
        1,
        "unchanged zones are not pushed again"
    );

    // Before the engine has seen the new zones, a colour set of the old size is not applied to
    // the new layout.
    rig.lights.set_colors(Rgb::new(1, 2, 3), &[RED]);
    assert_eq!(rig.last_hue(), (None, Rgb::new(1, 2, 3), Vec::new()));

    assert!(
        rig.engine
            .handle(EngineCommand::SetZones(vec![left_zone(), right_zone()]))
    );
    rig.run(10);
    assert_eq!(
        rig.last_grab(),
        bounding_rect(&[config().region, left_zone(), right_zone()])
    );
    assert_eq!(rig.last_hue().0, Some(8));
    assert_eq!(rig.last_hue().2, vec![RED, BLUE]);

    // No zones: the grab is the region again.
    assert!(rig.engine.handle(EngineCommand::SetZones(Vec::new())));
    rig.run(10);
    assert_eq!(rig.last_grab(), config().region);
}

#[test]
fn a_disabled_light_contributes_no_zones() {
    let rig = Rig::new(config(), Some(vec![left_zone(), right_zone()]));
    assert_eq!(rig.lights.zone_rects(), vec![left_zone(), right_zone()]);
    rig.lights.apply_config(&LightsConfig::default());
    assert_eq!(rig.lights.zone_rects(), Vec::<NormRect>::new());
}
