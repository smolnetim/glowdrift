//! Connects to iCUE, lists devices, sweeps every LED through the hue wheel for four seconds,
//! releases control and re-reads the devices to show the user's lighting is back.
//!
//! Usage: `cargo run -p glowdrift-outputs --example icue_probe -- [--dll-dir <dir>]`
//! (the directory containing `iCUESDK.x64_2019.dll`; the executable's directory and its
//! `resources` subdirectory are always searched, `PATH` and the working directory never are).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use glowdrift_core::Rgb;
use glowdrift_outputs::icue::{IcueOutput, ProbeReport, probe};
use glowdrift_outputs::{Output, OutputState};

const SWEEP: Duration = Duration::from_secs(4);
const FRAME: Duration = Duration::from_millis(16);

fn hue_to_rgb(hue: f32) -> Rgb {
    let h = hue.rem_euclid(360.0) / 60.0;
    let x = 1.0 - (h % 2.0 - 1.0).abs();
    let (r, g, b) = match h as u32 {
        0 => (1.0, x, 0.0),
        1 => (x, 1.0, 0.0),
        2 => (0.0, 1.0, x),
        3 => (0.0, x, 1.0),
        4 => (x, 0.0, 1.0),
        _ => (1.0, 0.0, x),
    };
    let level = |c: f32| (c * 255.0 * 0.8).round() as u8;
    Rgb::new(level(r), level(g), level(b))
}

fn print_report(title: &str, report: &ProbeReport) {
    println!("== {title}");
    println!("session states : {}", report.session_states.join(" -> "));
    println!(
        "versions       : iCUE {}, SDK server {}, SDK client {}",
        report.icue_version, report.sdk_server_version, report.sdk_client_version
    );
    let total: usize = report.devices.iter().map(|d| d.led_count).sum();
    println!("devices        : {} ({total} LEDs)", report.devices.len());
    for d in &report.devices {
        let first = d
            .first_led_color
            .map_or_else(|| "?".to_owned(), Rgb::to_hex);
        println!(
            "  - {:<28} {:<18} {:>3} LEDs  first LED {first}",
            d.model, d.kind, d.led_count
        );
    }
}

fn main() {
    let mut dirs = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--dll-dir" {
            dirs.extend(args.next().map(PathBuf::from));
        }
    }

    let before = match probe(dirs.clone(), Duration::from_secs(10)) {
        Ok(report) => report,
        Err(err) => {
            println!("probe failed: {err}");
            std::process::exit(1);
        }
    };
    print_report("before", &before);

    let mut output = IcueOutput::with_dll_dirs(dirs.clone());
    let started = Instant::now();
    while output.status().state != OutputState::Connected {
        if started.elapsed() > Duration::from_secs(10) {
            println!("output did not connect: {:?}", output.status());
            output.shutdown();
            std::process::exit(1);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let status = output.status();
    println!(
        "== output status: {:?}, {} device(s), detail {:?} (connected after {:?})",
        status.state,
        status.device_count,
        status.detail,
        started.elapsed()
    );

    println!("== sweeping for {SWEEP:?}");
    let sweep_start = Instant::now();
    let mut frames = 0u32;
    while sweep_start.elapsed() < SWEEP {
        let t = sweep_start.elapsed().as_secs_f32() / SWEEP.as_secs_f32();
        let color = hue_to_rgb(t * 360.0);
        let _ = output.set_color(color);
        frames += 1;
        if frames % 60 == 1 {
            println!("  t={t:.2} colour {}", color.to_hex());
        }
        std::thread::sleep(FRAME);
    }
    println!("  sent {frames} colour updates (the worker writes at most ~30/s)");
    println!("  status after sweep: {:?}", output.status().state);

    output.shutdown();
    println!("== control released, SDK disconnected");
    std::thread::sleep(Duration::from_millis(750));

    match probe(dirs, Duration::from_secs(10)) {
        Ok(after) => {
            print_report("after release", &after);
            for (a, b) in before.devices.iter().zip(&after.devices) {
                let verdict = if a.first_led_color == b.first_led_color {
                    "restored"
                } else {
                    "differs (an iCUE effect may be animating it)"
                };
                println!("  {:<28} first LED {verdict}", a.model);
            }
        }
        Err(err) => println!("second probe failed: {err}"),
    }
}
