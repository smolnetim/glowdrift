//! Exercises the Hue protocol against a real bridge.
//!
//! Usage: `cargo run -p glowdrift-lights --example hue_probe -- <command> [args]`
//!
//! - `discover`: list bridges found by mDNS (or the Signify discovery service).
//! - `probe <address>`: read the public config of a bridge.
//! - `pair <address>`: press the bridge's link button, then this polls for 30 seconds and
//!   prints the credentials as `HUE_*` variable assignments for the other commands.
//! - `areas`: list rooms, zones and entertainment areas.
//! - `drive <area-id>`: send red, green, blue, white and black to the area, then stop.
//!
//! `areas` and `drive` read the credentials from the environment: `HUE_ADDRESS`,
//! `HUE_APP_KEY`, `HUE_CERT_SHA256` and optionally `HUE_CLIENT_KEY` and `HUE_BRIDGE_ID`.
//! Nothing is written to disk.

use std::time::{Duration, Instant};

use glowdrift_core::Rgb;
use glowdrift_lights::hue::{
    BridgeCredentials, HueError, HueLight, PairOutcome, discover, list_areas, pair, probe,
};
use glowdrift_lights::{Light, LightState};

const PAIR_WINDOW: Duration = Duration::from_secs(30);
const PAIR_POLL: Duration = Duration::from_secs(2);
const HOLD: Duration = Duration::from_secs(3);

fn env(name: &str) -> Result<String, String> {
    std::env::var(name).map_err(|_| format!("environment variable {name} is not set"))
}

fn credentials() -> Result<BridgeCredentials, String> {
    Ok(BridgeCredentials {
        bridge_id: env("HUE_BRIDGE_ID").unwrap_or_default(),
        address: env("HUE_ADDRESS")?,
        app_key: env("HUE_APP_KEY")?,
        client_key: env("HUE_CLIENT_KEY").unwrap_or_default(),
        cert_sha256: env("HUE_CERT_SHA256")?,
    })
}

fn run_pair(address: &str) -> Result<(), HueError> {
    let started = Instant::now();
    println!("Press the link button on the bridge now.");
    while started.elapsed() < PAIR_WINDOW {
        match pair(address, "glowdrift#hue_probe")? {
            PairOutcome::Paired(creds) => {
                println!("Paired. Set these for the other commands:");
                println!("HUE_BRIDGE_ID={}", creds.bridge_id);
                println!("HUE_ADDRESS={}", creds.address);
                println!("HUE_APP_KEY={}", creds.app_key);
                println!("HUE_CLIENT_KEY={}", creds.client_key);
                println!("HUE_CERT_SHA256={}", creds.cert_sha256);
                return Ok(());
            }
            PairOutcome::LinkButtonNotPressed => std::thread::sleep(PAIR_POLL),
        }
    }
    println!("The link button was not pressed within {PAIR_WINDOW:?}.");
    Ok(())
}

fn run_drive(creds: BridgeCredentials, area: &str) {
    let (mut light, _handle) = HueLight::new(Some(creds), Some(area.to_owned()));
    let started = Instant::now();
    while light.status().state != LightState::Connected {
        let status = light.status();
        if status.state == LightState::Error || started.elapsed() > Duration::from_secs(15) {
            println!("Not connected: {:?} {:?}", status.state, status.detail);
            light.shutdown();
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let status = light.status();
    println!(
        "Connected: {} lights, min send interval {:?}",
        status.device_count,
        light.min_send_interval()
    );
    for (name, color) in [
        ("red", Rgb::new(255, 0, 0)),
        ("green", Rgb::new(0, 255, 0)),
        ("blue", Rgb::new(0, 0, 255)),
        ("white", Rgb::new(255, 255, 255)),
        ("black", Rgb::BLACK),
    ] {
        println!("-> {name}");
        let _ = light.set_color(color);
        std::thread::sleep(HOLD);
    }
    light.shutdown();
}

fn main() {
    let mut args = std::env::args().skip(1);
    let command = args.next().unwrap_or_default();
    let argument = args.next();
    let result: Result<(), String> = match (command.as_str(), argument) {
        ("discover", _) => {
            for bridge in discover(Duration::from_secs(4)) {
                println!("{} at {} ({:?})", bridge.id, bridge.address, bridge.name);
            }
            Ok(())
        }
        ("probe", Some(address)) => probe(&address)
            .map(|b| println!("{} at {} ({:?})", b.id, b.address, b.name))
            .map_err(|e| e.to_string()),
        ("pair", Some(address)) => run_pair(&address).map_err(|e| e.to_string()),
        ("areas", _) => credentials().and_then(|creds| {
            list_areas(&creds)
                .map(|areas| {
                    for area in areas {
                        println!(
                            "{} {:?} {:?} ({} lights)",
                            area.id, area.kind, area.name, area.light_count
                        );
                    }
                })
                .map_err(|e| e.to_string())
        }),
        ("drive", Some(area)) => credentials().map(|creds| run_drive(creds, &area)),
        _ => Err("usage: hue_probe discover | probe <address> | pair <address> | areas | drive <area-id>".to_owned()),
    };
    if let Err(message) = result {
        eprintln!("{message}");
        std::process::exit(1);
    }
}
