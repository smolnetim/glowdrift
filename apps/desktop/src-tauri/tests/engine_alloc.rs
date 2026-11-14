//! The steady-state per-frame path of the engine must not allocate on the heap.
//!
//! A scripted frame source feeds colours that change every frame, so every pass runs the
//! reduction, the target colour, the smoother, the status publication and the send to the lights.
//! The test drives `Engine::step` on a synthetic clock. It has its own binary because it installs
//! a global allocator. Set `ALLOC_TRACE=1` to print backtraces of the first allocations of the
//! measured window.
//!
//! Delivering the preview event to the frontend is not covered: the event sink here is a counter,
//! and serialising the payload for Tauri allocates outside the engine's control.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use glowdrift_capture::CaptureError;
use glowdrift_core::{Config, FrameView, LightsConfig, NormRect, ReduceMode, Rgb};
use glowdrift_desktop_lib::engine::{Engine, EngineCommand, EngineEvents, EngineStatus};
use glowdrift_desktop_lib::frame_source::{FrameSource, OpenSource};
use glowdrift_desktop_lib::lights_manager::LightsManager;
use glowdrift_lights::icue::ICUE_LIGHT_ID;
use glowdrift_lights::{Light, LightError, LightState, LightStatus};

thread_local! {
    static ALLOCATIONS: Cell<u64> = const { Cell::new(0) };
    static TRACING: Cell<bool> = const { Cell::new(false) };
}

/// Set to print a backtrace for each of the first allocations in the measured window.
static TRACE: AtomicBool = AtomicBool::new(false);
const TRACED: u64 = 12;

/// Counts allocations made by the calling thread only, so parallel tests do not interfere.
struct Counting;

fn count(size: usize) {
    let _ = ALLOCATIONS.try_with(|n| {
        let seen = n.get();
        n.set(seen + 1);
        if seen < TRACED && TRACE.load(Ordering::Relaxed) && !TRACING.get() {
            TRACING.set(true);
            let backtrace = std::backtrace::Backtrace::force_capture();
            eprintln!(
                "allocation #{seen} of {size} bytes
{backtrace}"
            );
            TRACING.set(false);
        }
    });
}

// SAFETY: every call is forwarded unchanged to the system allocator.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count(layout.size());
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        count(layout.size());
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        count(new_size);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

fn allocations() -> u64 {
    ALLOCATIONS.with(Cell::get)
}

/// A frame source whose every frame has a different colour.
struct Scripted {
    pixels: Vec<u8>,
    width: u32,
    height: u32,
    frames: u32,
    delivered: Arc<AtomicU32>,
}

impl Scripted {
    const WIDTH: u32 = 240;
    const HEIGHT: u32 = 135;

    fn paint(&mut self) {
        let n = self.frames;
        let left = [(n * 37) as u8, (n * 91 + 40) as u8, (n * 53 + 120) as u8];
        let right = [(n * 71 + 200) as u8, (n * 29) as u8, (n * 113 + 60) as u8];
        let width = self.width as usize;
        for row in self.pixels.chunks_exact_mut(width * 4) {
            for (x, pixel) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                let [b, g, r] = if x < width / 2 { left } else { right };
                *pixel = [b, g, r, 255];
            }
        }
    }
}

impl FrameSource for Scripted {
    fn grab(
        &mut self,
        _region: NormRect,
        _timeout: Duration,
    ) -> Result<Option<FrameView<'_>>, CaptureError> {
        self.frames += 1;
        self.delivered.fetch_add(1, Ordering::Relaxed);
        self.paint();
        Ok(Some(FrameView::new(
            &self.pixels,
            self.width,
            self.height,
            self.width as usize * 4,
        )))
    }
}

/// A light that remembers how many colours it was sent.
struct CountingLight(Arc<AtomicU32>);

impl Light for CountingLight {
    fn id(&self) -> &str {
        ICUE_LIGHT_ID
    }
    fn status(&self) -> LightStatus {
        LightStatus {
            id: ICUE_LIGHT_ID.to_owned(),
            kind: "icue".to_owned(),
            name: "Counting".to_owned(),
            enabled: true,
            state: LightState::Connected,
            detail: None,
            device_count: 1,
            code: None,
        }
    }
    fn set_enabled(&mut self, _enabled: bool) {}
    fn set_color(&mut self, _color: Rgb) -> Result<(), LightError> {
        self.0.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
    fn shutdown(&mut self) {}
}

#[derive(Default)]
struct Sink {
    previews: Arc<AtomicU32>,
    statuses: Arc<AtomicU32>,
}

impl EngineEvents for Sink {
    fn color(&self, _target: Rgb, _smoothed: Rgb) {
        self.previews.fetch_add(1, Ordering::Relaxed);
    }
    fn status(&self, _status: &EngineStatus) {
        self.statuses.fetch_add(1, Ordering::Relaxed);
    }
}

struct Run {
    delivered: u32,
    sends: u32,
    previews: u32,
    allocations: u64,
}

const TICK: Duration = Duration::from_micros(16_667);
const WARMUP_STEPS: u32 = 240;
const MEASURED_STEPS: u32 = 600;

/// Warms up, then counts the allocations of `MEASURED_STEPS` further passes.
fn run(mode: ReduceMode, preview: bool) -> Run {
    let config = Config {
        mode,
        capture_fps: 20,
        ..Config::default()
    };
    let delivered = Arc::new(AtomicU32::new(0));
    let sends = Arc::new(AtomicU32::new(0));
    let lights = LightsManager::new(
        vec![Box::new(CountingLight(Arc::clone(&sends)))],
        &LightsConfig::default(),
        |_| {},
    );
    let open: OpenSource = {
        let delivered = Arc::clone(&delivered);
        Box::new(move |_, _| {
            Ok(Box::new(Scripted {
                pixels: vec![0; (Scripted::WIDTH * Scripted::HEIGHT * 4) as usize],
                width: Scripted::WIDTH,
                height: Scripted::HEIGHT,
                frames: 0,
                delivered: Arc::clone(&delivered),
            }))
        })
    };
    let sink = Sink::default();
    let previews = Arc::clone(&sink.previews);
    let (_commands, rx) = mpsc::channel();
    let shared = Arc::new(Mutex::new(EngineStatus::default()));
    let mut engine = Engine::new(rx, shared, lights, sink, config, open);
    if preview {
        assert!(engine.handle(EngineCommand::SetPreviewEnabled(true)));
    }

    let start = Instant::now();
    let mut now = start;
    for _ in 0..WARMUP_STEPS {
        engine.step(now);
        now += TICK;
    }
    let delivered_before = delivered.load(Ordering::Relaxed);
    let sends_before = sends.load(Ordering::Relaxed);
    let previews_before = previews.load(Ordering::Relaxed);

    let trace = std::env::var_os("ALLOC_TRACE").is_some();
    ALLOCATIONS.with(|n| n.set(0));
    TRACE.store(trace, Ordering::Relaxed);
    for _ in 0..MEASURED_STEPS {
        engine.step(now);
        now += TICK;
    }
    TRACE.store(false, Ordering::Relaxed);
    let allocations = allocations();
    Run {
        delivered: delivered.load(Ordering::Relaxed) - delivered_before,
        sends: sends.load(Ordering::Relaxed) - sends_before,
        previews: previews.load(Ordering::Relaxed) - previews_before,
        allocations,
    }
}

fn assert_steady_state_is_allocation_free(mode: ReduceMode, preview: bool) {
    let run = run(mode, preview);
    // The path under test actually ran.
    assert!(run.delivered >= 100, "only {} frames", run.delivered);
    assert!(run.sends >= 300, "only {} sends", run.sends);
    if preview {
        assert!(run.previews >= 100, "only {} previews", run.previews);
    } else {
        assert_eq!(run.previews, 0);
    }
    assert_eq!(
        run.allocations, 0,
        "{} heap allocations over {} frames (ALLOC_TRACE=1 shows where)",
        run.allocations, run.delivered
    );
}

#[test]
fn average_mode_does_not_allocate_per_frame() {
    assert_steady_state_is_allocation_free(ReduceMode::Average, false);
}

#[test]
fn average_mode_with_preview_does_not_allocate_per_frame() {
    assert_steady_state_is_allocation_free(ReduceMode::Average, true);
}

#[test]
fn dominant_mode_does_not_allocate_per_frame() {
    assert_steady_state_is_allocation_free(ReduceMode::Dominant, false);
}
