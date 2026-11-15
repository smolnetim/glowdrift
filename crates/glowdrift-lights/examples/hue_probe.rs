//! Exercises the Hue protocol against a real bridge.
//!
//! Usage: `cargo run -p glowdrift-lights --example hue_probe -- <command> [args]`
//!
//! - `discover`: list bridges found by mDNS (or the Signify discovery service).
//! - `probe <address>`: read the public config of a bridge.
//! - `pair <address> <out-file>`: press the bridge's link button, then this polls for 30
//!   seconds and writes the credentials as `HUE_*=value` lines to `<out-file>`, which must not
//!   exist yet. The file holds the keys in plain text: delete it after use. The keys are never
//!   printed.
//! - `areas`: list the entertainment areas.
//! - `lights`: list the single lights that can be added as extra lights.
//! - `drive <area-id|-> [light-id...]`: send red, green, blue, white and black to the area
//!   (`-` for none) and the extra lights, then stop; the lights are put back as they were.
//!
//! `areas`, `lights` and `drive` read the credentials `HUE_ADDRESS`, `HUE_APP_KEY`,
//! `HUE_CERT_SHA256` and optionally `HUE_CLIENT_KEY` and `HUE_BRIDGE_ID`. Each is taken from
//! the environment, or else from the file named by `--creds <file>` (anywhere on the command
//! line) or by the `HUE_CREDENTIALS_FILE` environment variable. The file holds one
//! `NAME=value` per line; blank lines and lines starting with `#` are ignored.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant};

use glowdrift_core::Rgb;
use glowdrift_lights::hue::{
    BridgeCredentials, HueError, HueLight, PairOutcome, discover, list_areas, list_lights, pair,
    probe,
};
use glowdrift_lights::{Light, LightState};

const PAIR_WINDOW: Duration = Duration::from_secs(30);
const PAIR_POLL: Duration = Duration::from_secs(2);
const HOLD: Duration = Duration::from_secs(3);

const CREDENTIALS_FILE_VAR: &str = "HUE_CREDENTIALS_FILE";
const USAGE: &str = "usage: hue_probe [--creds <file>] discover | probe <address> | pair <address> <out-file> | areas | lights | drive <area-id|-> [light-id...]";

/// The `NAME=value` entries of a credentials file. Errors name the line, never its content.
fn read_credentials_file(path: &Path) -> Result<HashMap<String, String>, String> {
    let text =
        fs::read_to_string(path).map_err(|e| format!("could not read {}: {e}", path.display()))?;
    let mut entries = HashMap::new();
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (name, value) = line
            .split_once('=')
            .ok_or_else(|| format!("{}: line {} is not NAME=value", path.display(), index + 1))?;
        entries.insert(name.trim().to_owned(), value.trim().to_owned());
    }
    Ok(entries)
}

fn lookup(file: &HashMap<String, String>, name: &str) -> Result<String, String> {
    std::env::var(name)
        .ok()
        .or_else(|| file.get(name).cloned())
        .ok_or_else(|| {
            format!("{name} is set neither in the environment nor in a credentials file")
        })
}

fn credentials(file: Option<&Path>) -> Result<BridgeCredentials, String> {
    let file = match file {
        Some(path) => read_credentials_file(path)?,
        None => HashMap::new(),
    };
    Ok(BridgeCredentials {
        bridge_id: lookup(&file, "HUE_BRIDGE_ID").unwrap_or_default(),
        address: lookup(&file, "HUE_ADDRESS")?,
        app_key: lookup(&file, "HUE_APP_KEY")?,
        client_key: lookup(&file, "HUE_CLIENT_KEY").unwrap_or_default(),
        cert_sha256: lookup(&file, "HUE_CERT_SHA256")?,
    })
}

/// Removes `--creds <file>` from the arguments and returns the file, falling back to the
/// `HUE_CREDENTIALS_FILE` environment variable.
fn take_credentials_path(args: &mut Vec<String>) -> Result<Option<String>, String> {
    if let Some(index) = args.iter().position(|a| a == "--creds") {
        if index + 1 >= args.len() {
            return Err("--creds needs a file".to_owned());
        }
        let path = args.remove(index + 1);
        args.remove(index);
        return Ok(Some(path));
    }
    Ok(std::env::var(CREDENTIALS_FILE_VAR).ok())
}

/// Creates `out_file` (it must not exist), pairs, and stores the credentials there. The file
/// is removed again unless pairing succeeded.
fn run_pair(address: &str, out_file: &Path) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(out_file)
        .map_err(|e| format!("could not create {}: {e}", out_file.display()))?;
    let outcome = pair_and_store(address, &mut file);
    drop(file);
    match outcome {
        Ok(true) => {
            println!(
                "The credentials are in {}. The file holds the keys in plain text; delete it \
                 after use.",
                out_file.display()
            );
            Ok(())
        }
        Ok(false) => {
            let _ = fs::remove_file(out_file);
            println!("The link button was not pressed within {PAIR_WINDOW:?}.");
            Ok(())
        }
        Err(message) => {
            let _ = fs::remove_file(out_file);
            Err(message)
        }
    }
}

/// Polls for the link button; on success writes the credentials to `out` and prints only
/// what is not secret. Returns whether the bridge was paired.
fn pair_and_store(address: &str, out: &mut File) -> Result<bool, String> {
    let started = Instant::now();
    println!("Press the link button on the bridge now.");
    while started.elapsed() < PAIR_WINDOW {
        match pair(address, "glowdrift#hue_probe").map_err(|e: HueError| e.to_string())? {
            PairOutcome::Paired(creds) => {
                let lines = format!(
                    "HUE_BRIDGE_ID={}\nHUE_ADDRESS={}\nHUE_APP_KEY={}\nHUE_CLIENT_KEY={}\nHUE_CERT_SHA256={}\n",
                    creds.bridge_id,
                    creds.address,
                    creds.app_key,
                    creds.client_key,
                    creds.cert_sha256
                );
                out.write_all(lines.as_bytes())
                    .and_then(|()| out.flush())
                    .map_err(|e| format!("could not write the credentials: {e}"))?;
                println!("Paired.");
                println!("bridge id: {}", creds.bridge_id);
                println!("address: {}", creds.address);
                println!("certificate pin: {}", creds.cert_sha256);
                return Ok(true);
            }
            PairOutcome::LinkButtonNotPressed => std::thread::sleep(PAIR_POLL),
        }
    }
    Ok(false)
}

fn run_drive(creds: BridgeCredentials, area: Option<String>, extras: Vec<String>) {
    let (mut light, handle) = HueLight::new(Some(creds), area);
    handle.set_extra_lights(extras);
    light.set_enabled(true);
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
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let result = take_credentials_path(&mut args).and_then(|creds_path| run(args, creds_path));
    if let Err(message) = result {
        eprintln!("{message}");
        std::process::exit(1);
    }
}

fn run(args: Vec<String>, creds_path: Option<String>) -> Result<(), String> {
    let creds_file = creds_path.as_deref().map(Path::new);
    let mut args = args.into_iter();
    let command = args.next().unwrap_or_default();
    let argument = args.next();
    match (command.as_str(), argument) {
        ("discover", _) => {
            for bridge in discover(Duration::from_secs(4)) {
                println!("{} at {} ({:?})", bridge.id, bridge.address, bridge.name);
            }
            Ok(())
        }
        ("probe", Some(address)) => probe(&address)
            .map(|b| println!("{} at {} ({:?})", b.id, b.address, b.name))
            .map_err(|e| e.to_string()),
        ("pair", Some(address)) => match args.next() {
            Some(out_file) => run_pair(&address, Path::new(&out_file)),
            None => Err(USAGE.to_owned()),
        },
        ("areas", _) => credentials(creds_file).and_then(|creds| {
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
        ("lights", _) => credentials(creds_file).and_then(|creds| {
            list_lights(&creds, None)
                .map(|lights| {
                    for light in lights {
                        println!("{} {:?}", light.id, light.name);
                    }
                })
                .map_err(|e| e.to_string())
        }),
        ("drive", Some(area)) => credentials(creds_file).map(|creds| {
            let area = (area != "-").then_some(area);
            run_drive(creds, area, args.collect());
        }),
        _ => Err(USAGE.to_owned()),
    }
}
