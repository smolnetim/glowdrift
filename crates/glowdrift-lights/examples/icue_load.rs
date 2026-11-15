//! Drives the real iCUE light with a colour that changes on every update, at a fixed rate, for a
//! fixed time, so the CPU cost of the light can be measured in isolation (for example per thread
//! with `QueryThreadCycleTime`). Prints the process id when connected.
//!
//! Usage: `cargo run --profile profiling -p glowdrift-lights --example icue_load -- [--dll-dir <dir>]
//! [--secs 60] [--hz 60]`

use std::path::PathBuf;
use std::time::{Duration, Instant};

use glowdrift_core::Rgb;
use glowdrift_lights::icue::IcueLight;
use glowdrift_lights::{Light, LightState};

fn colour(step: u32) -> Rgb {
    let phase = |offset: u32| ((step.wrapping_mul(7).wrapping_add(offset)) % 200 + 20) as u8;
    Rgb::new(phase(0), phase(67), phase(133))
}

fn main() {
    let mut dirs = Vec::new();
    let mut secs = 60u64;
    let mut hz = 60u64;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--dll-dir" => dirs.extend(args.next().map(PathBuf::from)),
            "--secs" => secs = args.next().and_then(|v| v.parse().ok()).unwrap_or(secs),
            "--hz" => hz = args.next().and_then(|v| v.parse().ok()).unwrap_or(hz),
            _ => {}
        }
    }

    let mut light = IcueLight::with_dll_dirs(dirs);
    let started = Instant::now();
    while light.status().state != LightState::Connected {
        if started.elapsed() > Duration::from_secs(15) {
            println!("light did not connect: {:?}", light.status());
            light.shutdown();
            std::process::exit(1);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    println!(
        "pid {} connected: {:?}",
        std::process::id(),
        light.status().detail
    );

    let interval = Duration::from_micros(1_000_000 / hz.max(1));
    let begin = Instant::now();
    let mut step = 0u32;
    let mut next = begin;
    while begin.elapsed() < Duration::from_secs(secs) {
        let _ = light.set_color(colour(step));
        step += 1;
        next += interval;
        if let Some(wait) = next.checked_duration_since(Instant::now()) {
            std::thread::sleep(wait);
        }
    }
    println!("done: {step} set_color calls in {:?}", begin.elapsed());
    light.shutdown();
}
