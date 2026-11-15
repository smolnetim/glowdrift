//! End-to-end tests against a local HTTPS bridge with a self-signed certificate.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use glowdrift_core::Rgb;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use serde_json::{Value, json};

use super::api::fixtures;
use super::client::fingerprint;
use super::worker::Timing;
use super::*;
use crate::{Light, LightCode, LightState};

const APP_KEY: &str = "app-key-123";
const GAP: Duration = Duration::from_millis(20);
const ENTERTAINMENT_PATH: &str = "/clip/v2/resource/entertainment_configuration";

#[derive(Debug, Clone)]
struct Request {
    method: String,
    path: String,
    key: Option<String>,
    body: Value,
    at: Instant,
}

struct Mock {
    addr: SocketAddr,
    pin: String,
    requests: Arc<Mutex<Vec<Request>>>,
    link_pressed: Arc<AtomicBool>,
    /// Number of upcoming `GET .../entertainment_configuration` requests answered by dropping
    /// the connection.
    drop_inventory: Arc<AtomicU32>,
    /// Light ids whose state cannot be read.
    unreachable: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
}

impl Drop for Mock {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

fn read_request(
    reader: &mut BufReader<rustls::StreamOwned<rustls::ServerConnection, TcpStream>>,
) -> Option<Request> {
    let mut line = String::new();
    if reader.read_line(&mut line).ok()? == 0 {
        return None;
    }
    let mut parts = line.split_whitespace();
    let method = parts.next()?.to_owned();
    let path = parts.next()?.to_owned();
    let mut length = 0usize;
    let mut key = None;
    loop {
        let mut header = String::new();
        reader.read_line(&mut header).ok()?;
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        let (name, value) = header.split_once(':')?;
        match name.to_ascii_lowercase().as_str() {
            "content-length" => length = value.trim().parse().ok()?,
            "hue-application-key" => key = Some(value.trim().to_owned()),
            _ => {}
        }
    }
    let mut raw = vec![0u8; length];
    reader.read_exact(&mut raw).ok()?;
    Some(Request {
        method,
        path,
        key,
        body: serde_json::from_slice(&raw).unwrap_or(Value::Null),
        at: Instant::now(),
    })
}

impl Mock {
    fn start() -> Mock {
        let certified = rcgen::generate_simple_self_signed(vec!["127.0.0.1".to_owned()]).unwrap();
        let cert_der: CertificateDer<'static> = certified.cert.der().clone();
        let pin = fingerprint(&cert_der);
        let key = PrivateKeyDer::from(PrivatePkcs8KeyDer::from(
            certified.signing_key.serialize_der(),
        ));
        let config = Arc::new(
            rustls::ServerConfig::builder_with_provider(Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(vec![cert_der], key)
            .unwrap(),
        );
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();
        let mock = Mock {
            addr,
            pin,
            requests: Arc::default(),
            link_pressed: Arc::default(),
            drop_inventory: Arc::default(),
            unreachable: Arc::default(),
            stop: Arc::default(),
        };
        let requests = Arc::clone(&mock.requests);
        let link_pressed = Arc::clone(&mock.link_pressed);
        let drop_inventory = Arc::clone(&mock.drop_inventory);
        let unreachable = Arc::clone(&mock.unreachable);
        let stop = Arc::clone(&mock.stop);
        std::thread::spawn(move || {
            while !stop.load(Ordering::SeqCst) {
                let Ok((stream, _)) = listener.accept() else {
                    std::thread::sleep(Duration::from_millis(5));
                    continue;
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let config = Arc::clone(&config);
                let requests = Arc::clone(&requests);
                let link_pressed = Arc::clone(&link_pressed);
                let drop_inventory = Arc::clone(&drop_inventory);
                let unreachable = Arc::clone(&unreachable);
                let stop = Arc::clone(&stop);
                std::thread::spawn(move || {
                    let conn = rustls::ServerConnection::new(config).unwrap();
                    let mut reader = BufReader::new(rustls::StreamOwned::new(conn, stream));
                    while !stop.load(Ordering::SeqCst) {
                        let Some(request) = read_request(&mut reader) else {
                            return;
                        };
                        requests.lock().unwrap().push(request.clone());
                        if request.path.ends_with("/entertainment_configuration")
                            && drop_inventory.load(Ordering::SeqCst) > 0
                        {
                            drop_inventory.fetch_sub(1, Ordering::SeqCst);
                            return;
                        }
                        let (status, body) = route(&request, &link_pressed, &unreachable);
                        let response = format!(
                            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                            body.len()
                        );
                        let stream = reader.get_mut();
                        if stream.write_all(response.as_bytes()).is_err() || stream.flush().is_err()
                        {
                            return;
                        }
                    }
                });
            }
        });
        mock
    }

    fn address(&self) -> String {
        self.addr.to_string()
    }

    fn creds(&self) -> BridgeCredentials {
        BridgeCredentials {
            bridge_id: "AABBCCFFFE112233".to_owned(),
            address: self.address(),
            app_key: APP_KEY.to_owned(),
            client_key: "ck".to_owned(),
            cert_sha256: self.pin.clone(),
        }
    }

    fn puts(&self) -> Vec<Request> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.method == "PUT")
            .cloned()
            .collect()
    }

    /// Requests in arrival order as `(method, path)`.
    fn log(&self) -> Vec<(String, String)> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .map(|r| (r.method.clone(), r.path.clone()))
            .collect()
    }

    /// The PUTs that restore a light (the only ones with a 400 ms fade).
    fn restores(&self) -> Vec<Request> {
        self.puts()
            .into_iter()
            .filter(|p| p.body["dynamics"]["duration"] == json!(400))
            .collect()
    }

    fn count(&self, method: &str, path: &str) -> usize {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.method == method && r.path == path)
            .count()
    }
}

fn light_state(id: &str) -> Option<Value> {
    let lights: Value = serde_json::from_str(fixtures::LIGHTS).unwrap();
    lights["data"]
        .as_array()?
        .iter()
        .find(|l| l["id"] == id)
        .cloned()
}

fn route(
    request: &Request,
    link_pressed: &AtomicBool,
    unreachable: &Mutex<Vec<String>>,
) -> (&'static str, String) {
    let ok = |body: &str| ("200 OK", body.to_owned());
    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/api/0/config") => ok(fixtures::CONFIG),
        ("POST", "/api") => {
            if link_pressed.load(Ordering::SeqCst) {
                ok(fixtures::PAIR_OK)
            } else {
                ok(fixtures::PAIR_LINK_BUTTON)
            }
        }
        _ if request.key.as_deref() != Some(APP_KEY) => (
            "403 Forbidden",
            r#"{"errors":[{"description":"forbidden"}]}"#.to_owned(),
        ),
        ("GET", "/clip/v2/resource/entertainment_configuration") => ok(fixtures::ENTERTAINMENT),
        ("GET", "/clip/v2/resource/device") => ok(fixtures::DEVICES),
        ("GET", "/clip/v2/resource/light") => ok(fixtures::LIGHTS),
        ("GET", path) if path.starts_with("/clip/v2/resource/light/") => {
            let id = path.trim_start_matches("/clip/v2/resource/light/");
            match light_state(id) {
                Some(state) if !unreachable.lock().unwrap().iter().any(|u| u == id) => {
                    ok(&json!({"errors": [], "data": [state]}).to_string())
                }
                _ => (
                    "404 Not Found",
                    r#"{"errors":[{"description":"light unreachable"}]}"#.to_owned(),
                ),
            }
        }
        ("PUT", path) if path.starts_with("/clip/v2/resource/light/") => {
            ok(r#"{"errors":[],"data":[{"rid":"x","rtype":"light"}]}"#)
        }
        _ => (
            "404 Not Found",
            r#"{"errors":[{"description":"not found"}]}"#.to_owned(),
        ),
    }
}

fn wait_for(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn test_timing() -> Timing {
    Timing {
        put_gap: GAP,
        backoff_initial: Duration::from_millis(20),
        backoff_max: Duration::from_millis(80),
    }
}

fn light_for(mock: &Mock, area: &str) -> (HueLight, HueHandle) {
    HueLight::with_timing(Some(mock.creds()), Some(area.to_owned()), test_timing())
}

/// A light driving `area` (if any) plus the extra lights `extras`, connected to all of them.
fn light_with(mock: &Mock, area: Option<&str>, extras: &[&str]) -> (HueLight, HueHandle) {
    let (light, handle) =
        HueLight::with_timing(Some(mock.creds()), area.map(str::to_owned), test_timing());
    handle.set_extra_lights(extras.iter().map(|e| (*e).to_owned()).collect());
    wait_for("the lights to be driven", || {
        let status = light.status();
        status.state == LightState::Connected
            && handle
                .area_lights()
                .is_some_and(|lights| lights.len() == status.device_count as usize)
    });
    (light, handle)
}

fn put_path(id: &str) -> String {
    format!("/clip/v2/resource/light/{id}")
}

#[test]
fn probe_reads_the_public_config() {
    let mock = Mock::start();
    let candidate = probe(&mock.address()).unwrap();
    assert_eq!(candidate.id, "AABBCCFFFE112233");
    assert_eq!(candidate.address, mock.address());
    assert_eq!(candidate.name.as_deref(), Some("Philips hue"));
}

#[test]
fn probe_rejects_unusable_addresses() {
    for address in ["", "  ", "host/path", "user@host", "a b", "host?x=1"] {
        assert!(
            matches!(probe(address), Err(HueError::InvalidAddress(_))),
            "{address:?}"
        );
    }
}

#[test]
fn probe_of_a_closed_port_is_a_network_error() {
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    assert!(matches!(
        probe(&format!("127.0.0.1:{port}")),
        Err(HueError::Network(_))
    ));
}

#[test]
fn pairing_waits_for_the_link_button_then_pins_the_certificate() {
    let mock = Mock::start();
    assert_eq!(
        pair(&mock.address(), "glowdrift#test").unwrap(),
        PairOutcome::LinkButtonNotPressed
    );
    mock.link_pressed.store(true, Ordering::SeqCst);
    let PairOutcome::Paired(creds) = pair(&mock.address(), "glowdrift#test").unwrap() else {
        panic!("expected pairing to succeed");
    };
    assert_eq!(creds.bridge_id, "AABBCCFFFE112233");
    assert_eq!(creds.app_key, APP_KEY);
    assert_eq!(creds.client_key, "test-client-key");
    assert_eq!(creds.address, mock.address());
    assert_eq!(creds.cert_sha256, mock.pin);
    assert_eq!(creds.cert_sha256.len(), 64);

    let post = mock
        .requests
        .lock()
        .unwrap()
        .iter()
        .find(|r| r.method == "POST")
        .cloned()
        .unwrap();
    assert_eq!(
        post.body,
        json!({"devicetype": "glowdrift#test", "generateclientkey": true})
    );
}

#[test]
fn debug_output_redacts_secrets() {
    let text = format!("{:?}", Mock::start().creds());
    assert!(!text.contains(APP_KEY) && !text.contains("\"ck\""));
    assert!(text.contains("<redacted>"));
}

#[test]
fn pinned_certificate_is_accepted_and_areas_are_listed() {
    let mock = Mock::start();
    let areas = list_areas(&mock.creds()).unwrap();
    let names: Vec<_> = areas
        .iter()
        .map(|a| (a.name.as_str(), a.kind, a.light_count))
        .collect();
    assert_eq!(
        names,
        vec![
            ("Cinema", AreaKind::Entertainment, 2),
            ("TV", AreaKind::Entertainment, 2),
        ]
    );
    let requests = mock.requests.lock().unwrap();
    assert!(requests.iter().all(|r| r.key.as_deref() == Some(APP_KEY)));
}

#[test]
fn pin_comparison_ignores_case() {
    let mock = Mock::start();
    let mut creds = mock.creds();
    creds.cert_sha256 = creds.cert_sha256.to_uppercase();
    assert!(list_areas(&creds).is_ok());
}

#[test]
fn a_different_certificate_is_rejected() {
    let mock = Mock::start();
    let mut creds = mock.creds();
    creds.cert_sha256 = "0".repeat(64);
    assert!(matches!(
        list_areas(&creds),
        Err(HueError::CertificateMismatch)
    ));
    assert!(
        mock.requests.lock().unwrap().is_empty(),
        "no request may reach a mismatched peer"
    );

    // A second bridge with its own certificate does not satisfy the first one's pin.
    let other = Mock::start();
    let mut creds = other.creds();
    creds.cert_sha256 = mock.pin.clone();
    assert!(matches!(
        list_areas(&creds),
        Err(HueError::CertificateMismatch)
    ));
}

#[test]
fn a_wrong_app_key_is_unauthorized() {
    let mock = Mock::start();
    let mut creds = mock.creds();
    creds.app_key = "stale".to_owned();
    assert!(matches!(list_areas(&creds), Err(HueError::Unauthorized)));
}

#[test]
fn light_sends_gamut_clamped_colours_with_a_fade_per_light() {
    let mock = Mock::start();
    let (mut light, _handle) = light_for(&mock, "ent-1");
    light.set_color(Rgb::new(255, 255, 255)).unwrap();
    wait_for("both lights to be driven", || mock.puts().len() >= 2);

    let status = light.status();
    assert_eq!(status.state, LightState::Connected);
    assert_eq!(status.device_count, 2);
    assert_eq!(status.kind, "hue");
    assert_eq!(light.min_send_interval(), Some(GAP * 2));

    let puts = mock.puts();
    assert_eq!(puts[0].path, put_path("light-a"));
    assert_eq!(puts[1].path, put_path("light-b"));
    for put in &puts[..2] {
        assert_eq!(put.body["on"], json!({"on": true}));
        assert_eq!(put.body["dimming"]["brightness"], json!(100.0));
        assert_eq!(put.body["dynamics"]["duration"], json!(40));
        let x = put.body["color"]["xy"]["x"].as_f64().unwrap();
        let y = put.body["color"]["xy"]["y"].as_f64().unwrap();
        assert!(
            (x - 0.3127).abs() < 0.01 && (y - 0.329).abs() < 0.01,
            "{x},{y}"
        );
    }

    // Saturated blue is outside gamut B (light-b), so its xy differs from light-a's (gamut C).
    light.set_color(Rgb::new(0, 0, 255)).unwrap();
    wait_for("blue on both lights", || {
        mock.puts()
            .iter()
            .filter(|p| {
                p.body["dimming"]["brightness"] == json!(100.0)
                    && p.body["color"]["xy"]["y"].as_f64().is_some_and(|y| y < 0.1)
            })
            .count()
            >= 2
    });
    let blues: Vec<_> = mock
        .puts()
        .into_iter()
        .filter(|p| p.body["color"]["xy"]["y"].as_f64().is_some_and(|y| y < 0.1))
        .collect();
    let for_light = |id: &str| {
        blues.iter().find(|p| p.path == put_path(id)).unwrap().body["color"]["xy"].clone()
    };
    assert_ne!(for_light("light-a"), for_light("light-b"));
    light.shutdown();
}

#[test]
fn black_switches_lights_off_and_the_latest_colour_wins() {
    let mock = Mock::start();
    let (mut light, _handle) = light_with(&mock, None, &["light-c"]);
    light.set_color(Rgb::new(255, 0, 0)).unwrap();
    light.set_color(Rgb::new(0, 255, 0)).unwrap();
    light.set_color(Rgb::BLACK).unwrap();
    wait_for("off", || {
        mock.puts()
            .iter()
            .any(|p| p.body["on"] == json!({"on": false}))
    });
    let last = mock.puts().pop().unwrap();
    assert_eq!(last.path, put_path("light-c"));
    assert_eq!(last.body["on"], json!({"on": false}));
    assert_eq!(last.body["dynamics"]["duration"], json!(20));
    // light-c has no colour support: a lit colour carries brightness only.
    let before = mock.puts().len();
    light.set_color(Rgb::new(200, 0, 0)).unwrap();
    wait_for("brightness-only PUT", || mock.puts().len() > before);
    let lit = mock.puts().pop().unwrap();
    assert_eq!(lit.body["on"], json!({"on": true}));
    assert!(lit.body.get("color").is_none());
    let brightness = lit.body["dimming"]["brightness"].as_f64().unwrap();
    assert!((brightness - 78.4).abs() < 0.2, "{brightness}");
    light.shutdown();
}

#[test]
fn puts_respect_the_rate_budget_and_identical_colours_are_not_resent() {
    let mock = Mock::start();
    let (mut light, _handle) = light_for(&mock, "ent-1");
    wait_for("connected", || {
        light.status().state == LightState::Connected
    });
    for step in 0..=200u8 {
        light.set_color(Rgb::new(step, 200, 30)).unwrap();
    }
    wait_for("both lights driven", || mock.puts().len() >= 2);
    std::thread::sleep(GAP * 10);
    let puts = mock.puts();
    for pair in puts.windows(2) {
        assert!(
            pair[1].at - pair[0].at + Duration::from_millis(3) >= GAP,
            "PUTs {:?} apart",
            pair[1].at - pair[0].at
        );
    }
    // Coalescing: 201 colours offered, a handful sent.
    assert!(puts.len() <= 6, "{} PUTs", puts.len());
    let settled = puts.len();
    light.set_color(Rgb::new(200, 200, 30)).unwrap();
    std::thread::sleep(GAP * 6);
    assert_eq!(
        mock.puts().len(),
        settled,
        "an unchanged colour must not be resent"
    );
    light.shutdown();
}

#[test]
fn missing_credentials_or_area_are_reported_with_codes() {
    let (mut none, handle) = HueLight::with_timing(None, None, test_timing());
    wait_for("not paired", || {
        none.status().code == Some(LightCode::NotPaired)
    });
    assert_eq!(none.status().state, LightState::Unavailable);

    let mock = Mock::start();
    handle.set_target(Some(mock.creds()), None);
    wait_for("no area", || none.status().code == Some(LightCode::NoArea));

    handle.set_target(Some(mock.creds()), Some("ent-1".to_owned()));
    wait_for("connected", || none.status().state == LightState::Connected);
    assert_eq!(none.status().code, None);
    assert_eq!(none.status().device_count, 2);

    handle.set_target(None, None);
    wait_for("unpaired again", || {
        none.status().code == Some(LightCode::NotPaired)
    });
    none.shutdown();
}

#[test]
fn a_new_light_stays_disabled_until_enabled() {
    let mock = Mock::start();
    let (mut light, _handle) = HueLight::new(Some(mock.creds()), Some("ent-1".to_owned()));
    assert_eq!(light.status().state, LightState::Disabled);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(light.status().state, LightState::Disabled);
    assert_eq!(
        mock.requests.lock().unwrap().len(),
        0,
        "a disabled light must not contact the bridge"
    );
    light.set_enabled(true);
    wait_for("connected", || {
        light.status().state == LightState::Connected
    });
    light.shutdown();
}

#[test]
fn an_unknown_area_reports_no_area() {
    let mock = Mock::start();
    let (mut light, _handle) = light_for(&mock, "does-not-exist");
    wait_for("no area", || light.status().code == Some(LightCode::NoArea));
    assert_eq!(light.status().state, LightState::Unavailable);
    light.shutdown();
}

#[test]
fn rejected_key_is_an_error_state_without_retries() {
    let mock = Mock::start();
    let mut creds = mock.creds();
    creds.app_key = "revoked".to_owned();
    let (mut light, _handle) =
        HueLight::with_timing(Some(creds), Some("ent-1".to_owned()), test_timing());
    wait_for("error", || light.status().state == LightState::Error);
    let detail = light.status().detail.unwrap();
    assert!(detail.contains("app key"), "{detail}");
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(mock.count("GET", ENTERTAINMENT_PATH), 1, "no retry loop");

    // A retry request reconnects once more.
    light.retry();
    wait_for("second attempt", || {
        mock.count("GET", ENTERTAINMENT_PATH) == 2
    });
    light.shutdown();
}

#[test]
fn a_changed_certificate_is_an_error_state() {
    let mock = Mock::start();
    let mut creds = mock.creds();
    creds.cert_sha256 = "f".repeat(64);
    let (mut light, _handle) =
        HueLight::with_timing(Some(creds), Some("ent-1".to_owned()), test_timing());
    wait_for("error", || light.status().state == LightState::Error);
    assert!(light.status().detail.unwrap().contains("certificate"));
    assert!(mock.requests.lock().unwrap().is_empty());
    light.shutdown();
}

#[test]
fn lost_connection_recovers_with_backoff() {
    let mock = Mock::start();
    mock.drop_inventory.store(3, Ordering::SeqCst);
    let (mut light, handle) = HueLight::with_timing(Some(mock.creds()), None, test_timing());
    handle.set_extra_lights(vec!["light-c".to_owned()]);
    light.set_color(Rgb::new(10, 200, 10)).unwrap();
    wait_for("unavailable", || {
        light.status().state == LightState::Unavailable
    });
    assert!(light.status().detail.unwrap().contains("not reachable"));
    wait_for("connected after retries", || {
        light.status().state == LightState::Connected
    });
    wait_for("colour delivered after reconnect", || {
        !mock.puts().is_empty()
    });
    assert!(mock.count("GET", ENTERTAINMENT_PATH) >= 4);
    light.shutdown();
}

#[test]
fn disabling_stops_sending_and_enabling_resumes() {
    let mock = Mock::start();
    let (mut light, _handle) = light_with(&mock, None, &["light-c"]);
    light.set_color(Rgb::new(255, 0, 0)).unwrap();
    wait_for("first PUT", || !mock.puts().is_empty());
    light.set_enabled(false);
    assert_eq!(light.status().state, LightState::Disabled);
    wait_for("the restore", || !mock.restores().is_empty());
    let sent = mock.puts().len();
    light.set_color(Rgb::new(0, 0, 255)).unwrap();
    std::thread::sleep(GAP * 5);
    assert_eq!(mock.puts().len(), sent);
    light.set_enabled(true);
    wait_for("PUT after enabling", || mock.puts().len() > sent);
    // A new send session reads the state again.
    assert_eq!(mock.count("GET", &put_path("light-c")), 2);
    light.shutdown();
}

#[test]
fn shutdown_joins_the_worker_and_is_idempotent() {
    let mock = Mock::start();
    let (mut light, handle) = light_for(&mock, "ent-1");
    wait_for("connected", || {
        light.status().state == LightState::Connected
    });
    let started = Instant::now();
    light.shutdown();
    light.shutdown();
    assert!(started.elapsed() < Duration::from_secs(2));
    // The handle outlives the light without blocking or panicking.
    handle.set_target(None, None);
}

fn layout_of(light: &HueLight) -> crate::ZoneLayout {
    light.zone_layout().expect("the lights are resolved")
}

fn ids(handle: &HueHandle) -> Vec<String> {
    handle
        .area_lights()
        .unwrap_or_default()
        .into_iter()
        .map(|l| l.id)
        .collect()
}

fn restored_ids(mock: &Mock) -> Vec<String> {
    mock.restores()
        .iter()
        .map(|p| {
            p.path
                .trim_start_matches("/clip/v2/resource/light/")
                .to_owned()
        })
        .collect()
}

#[test]
fn only_the_lights_of_the_bridge_are_listed_colour_lights_first() {
    let mock = Mock::start();
    let lights = list_lights(&mock.creds(), None).unwrap();
    let listed: Vec<_> = lights
        .iter()
        .map(|l| (l.id.as_str(), l.name.as_str()))
        .collect();
    assert_eq!(
        listed,
        vec![
            ("light-b", "Desk lamp"),
            ("light-a", "Floor lamp"),
            ("light-c", "Hall bulb")
        ]
    );
    assert_eq!(
        mock.count("GET", ENTERTAINMENT_PATH),
        0,
        "listing lights needs no areas"
    );
    let mut creds = mock.creds();
    creds.cert_sha256 = "0".repeat(64);
    assert!(matches!(
        list_lights(&creds, None),
        Err(HueError::CertificateMismatch)
    ));
}

#[test]
fn lights_of_the_chosen_area_are_not_offered_as_extra_lights() {
    let mock = Mock::start();
    let creds = mock.creds();
    let outside = list_lights(&creds, Some("ent-2")).unwrap();
    let listed: Vec<_> = outside.iter().map(|l| l.id.as_str()).collect();
    assert_eq!(listed, vec!["light-b"]);
    let unknown = list_lights(&creds, Some("nope")).unwrap();
    assert_eq!(unknown.len(), 3);
}

#[test]
fn entertainment_lights_get_default_zones_from_their_positions() {
    let mock = Mock::start();
    let (mut light, handle) = light_for(&mock, "ent-2");
    wait_for("resolved", || handle.area_lights().is_some());
    let lights = handle.area_lights().unwrap();
    // Channel order: light-a (average of two channels), then light-c.
    let summary: Vec<_> = lights
        .iter()
        .map(|l| (l.id.as_str(), l.name.as_str(), l.source, l.custom))
        .collect();
    assert_eq!(
        summary,
        vec![
            ("light-a", "Floor lamp", ZoneSource::Entertainment, false),
            ("light-c", "Hall bulb", ZoneSource::Entertainment, false),
        ]
    );
    let a = lights[0].position.unwrap();
    assert!(
        (a.x + 0.6).abs() < 1e-6 && (a.z - 0.5).abs() < 1e-6,
        "{a:?}"
    );
    let positions: Vec<_> = lights.iter().map(|l| l.position).collect();
    let defaults = glowdrift_core::default_zones(&positions);
    assert_eq!(lights.iter().map(|l| l.zone).collect::<Vec<_>>(), defaults);
    let layout = layout_of(&light);
    assert_eq!(layout.zones, defaults);
    assert_eq!(light.status().device_count, 2);
    light.shutdown();
}

#[test]
fn extra_lights_follow_the_area_in_config_order_with_a_centred_box() {
    let mock = Mock::start();
    // light-a is already in the area, "ghost" is unknown: both add nothing.
    let (mut light, handle) = light_with(&mock, Some("ent-1"), &["light-c", "light-a", "ghost"]);
    assert_eq!(ids(&handle), ["light-a", "light-b", "light-c"]);
    assert_eq!(light.status().device_count, 3);
    let lights = handle.area_lights().unwrap();
    assert_eq!(lights[2].source, ZoneSource::Extra);
    assert_eq!(lights[2].position, None);
    let centred = glowdrift_core::NormRect::new(0.4, 0.0, 0.2, 1.0);
    let zone = lights[2].zone;
    assert!(
        (zone.x - centred.x).abs() < 1e-6
            && zone.y == 0.0
            && (zone.width - centred.width).abs() < 1e-6
            && zone.height == 1.0,
        "{zone:?}"
    );
    // The area has no positions, so its lights share the monitor as columns.
    assert_eq!(lights[0].zone.width, 0.5);
    assert_eq!(lights[1].zone.x, 0.5);
    assert_eq!(layout_of(&light).zones.len(), 3);

    // Extra lights alone are a valid target too.
    let (mut extras_only, only_handle) = light_with(&mock, None, &["light-b"]);
    assert_eq!(ids(&only_handle), ["light-b"]);
    extras_only.shutdown();
    light.shutdown();
}

#[test]
fn nothing_to_drive_is_no_area() {
    let mock = Mock::start();
    let (mut light, handle) = HueLight::with_timing(Some(mock.creds()), None, test_timing());
    wait_for("no area", || light.status().code == Some(LightCode::NoArea));
    handle.set_extra_lights(vec!["ghost".to_owned()]);
    wait_for("still no area", || {
        mock.count("GET", ENTERTAINMENT_PATH) >= 1 && light.status().code == Some(LightCode::NoArea)
    });
    assert_eq!(handle.area_lights(), None);
    assert_eq!(light.zone_layout(), None);
    handle.set_extra_lights(vec!["light-b".to_owned()]);
    wait_for("connected", || {
        light.status().state == LightState::Connected
    });
    assert_eq!(light.status().code, None);
    light.shutdown();
}

#[test]
fn the_layout_generation_changes_with_lights_overrides_and_extras() {
    let mock = Mock::start();
    let (mut light, handle) = HueLight::with_timing(Some(mock.creds()), None, test_timing());
    assert_eq!(light.zone_layout(), None);
    handle.set_target(Some(mock.creds()), Some("ent-1".to_owned()));
    wait_for("resolved", || light.zone_layout().is_some());
    let first = layout_of(&light);
    assert_eq!(first.zones.len(), 2);

    let zone = glowdrift_core::NormRect::new(0.1, 0.2, 0.3, 0.4);
    handle.set_zone_overrides(std::collections::BTreeMap::from([(
        "light-b".to_owned(),
        zone,
    )]));
    let overridden = layout_of(&light);
    assert!(overridden.generation > first.generation);
    assert_eq!(overridden.zones[1], zone);
    assert_eq!(overridden.zones[0], first.zones[0]);
    let lights = handle.area_lights().unwrap();
    assert!(!lights[0].custom && lights[1].custom);

    // The same overrides again change nothing.
    handle.set_zone_overrides(std::collections::BTreeMap::from([(
        "light-b".to_owned(),
        zone,
    )]));
    assert_eq!(layout_of(&light), overridden);

    handle.set_zone_overrides(std::collections::BTreeMap::new());
    let reset = layout_of(&light);
    assert!(reset.generation > overridden.generation);
    assert_eq!(reset.zones, first.zones);

    handle.set_extra_lights(vec!["light-c".to_owned()]);
    wait_for("extra light", || {
        light.zone_layout().is_some_and(|l| l.zones.len() == 3)
    });
    let with_extra = layout_of(&light);
    assert!(with_extra.generation > reset.generation);

    handle.set_target(Some(mock.creds()), Some("ent-2".to_owned()));
    wait_for("other area", || {
        light
            .zone_layout()
            .is_some_and(|l| l.generation > with_extra.generation && l.zones.len() == 2)
    });
    assert_eq!(ids(&handle), ["light-a", "light-c"]);
    light.shutdown();
}

#[test]
fn zone_colours_are_sent_per_light_and_only_changed_lights_are_resent() {
    let mock = Mock::start();
    let (mut light, handle) = light_with(&mock, Some("ent-1"), &[]);
    let generation = layout_of(&light).generation;
    let (red, blue) = (Rgb::new(255, 0, 0), Rgb::new(0, 0, 255));
    light
        .set_zone_colors(generation, Rgb::new(9, 9, 9), &[red, blue])
        .unwrap();
    wait_for("both lights", || mock.puts().len() >= 2);
    let puts = mock.puts();
    let for_light = |id: &str| {
        puts.iter()
            .find(|p| p.path == put_path(id))
            .unwrap()
            .clone()
    };
    let a = &for_light("light-a").body["color"]["xy"];
    let b = &for_light("light-b").body["color"]["xy"];
    assert!(a["x"].as_f64().unwrap() > 0.6, "light-a is red: {a}");
    assert!(b["y"].as_f64().unwrap() < 0.1, "light-b is blue: {b}");
    assert!(
        puts.iter()
            .all(|p| p.body["dynamics"]["duration"] == json!(40))
    );

    // Only the second zone changes: only light-b is sent again.
    let green = Rgb::new(0, 255, 0);
    light
        .set_zone_colors(generation, Rgb::new(9, 9, 9), &[red, green])
        .unwrap();
    wait_for("light-b update", || mock.puts().len() >= 3);
    std::thread::sleep(GAP * 6);
    let puts = mock.puts();
    assert_eq!(puts.len(), 3, "{puts:?}");
    assert_eq!(puts[2].path, put_path("light-b"));
    assert!(puts[2].body["color"]["xy"]["y"].as_f64().unwrap() > 0.5);

    // The whole colour still goes to every light.
    light.set_color(Rgb::new(255, 255, 255)).unwrap();
    wait_for("white on both", || mock.puts().len() >= 5);
    drop(handle);
    light.shutdown();
}

#[test]
fn a_stale_generation_falls_back_to_the_whole_colour_for_every_light() {
    let mock = Mock::start();
    let (mut light, _handle) = light_with(&mock, Some("ent-1"), &[]);
    let generation = layout_of(&light).generation;
    let (red, blue) = (Rgb::new(255, 0, 0), Rgb::new(0, 0, 255));
    light
        .set_zone_colors(generation + 7, Rgb::new(0, 255, 0), &[red, blue])
        .unwrap();
    wait_for("both lights", || mock.puts().len() >= 2);
    for put in mock.puts() {
        assert!(
            put.body["color"]["xy"]["y"].as_f64().unwrap() > 0.5,
            "{put:?} must be green"
        );
    }
    // A zone list of the wrong length is stale too.
    let before = mock.puts().len();
    light
        .set_zone_colors(generation, Rgb::new(255, 0, 0), &[red])
        .unwrap();
    wait_for("red on both", || mock.puts().len() >= before + 2);
    light.shutdown();
}

#[test]
fn lights_are_read_before_the_first_colour_and_restored_when_disabled() {
    let mock = Mock::start();
    let (mut light, _handle) = light_with(&mock, Some("ent-1"), &["light-c"]);
    assert_eq!(mock.puts().len(), 0, "nothing is sent before a colour");
    light.set_color(Rgb::new(255, 0, 0)).unwrap();
    wait_for("all three driven", || mock.puts().len() >= 3);
    light.set_color(Rgb::new(0, 255, 0)).unwrap();
    wait_for("the colour lights updated", || {
        ["light-a", "light-b"].iter().all(|id| {
            mock.puts()
                .iter()
                .filter(|p| p.path == put_path(id))
                .count()
                >= 2
        })
    });

    // Every light was read exactly once, before its first PUT.
    let log = mock.log();
    for id in ["light-a", "light-b", "light-c"] {
        let path = put_path(id);
        assert_eq!(mock.count("GET", &path), 1, "{id}");
        let get = log
            .iter()
            .position(|(m, p)| m == "GET" && *p == path)
            .unwrap();
        let put = log
            .iter()
            .position(|(m, p)| m == "PUT" && *p == path)
            .unwrap();
        assert!(get < put, "{id} must be read before it is changed");
    }

    light.set_enabled(false);
    wait_for("three restores", || mock.restores().len() >= 3);
    let restores = mock.restores();
    let body = |id: &str| {
        restores
            .iter()
            .find(|p| p.path == put_path(id))
            .unwrap()
            .body
            .clone()
    };
    // Colour lights go back to their colour...
    for id in ["light-a", "light-b"] {
        assert_eq!(
            body(id),
            json!({
                "on": {"on": true},
                "dimming": {"brightness": 50.0},
                "color": {"xy": {"x": 0.3, "y": 0.3}},
                "dynamics": {"duration": 400}
            }),
            "{id}"
        );
    }
    // ...a light in colour-temperature mode to its mirek.
    assert_eq!(
        body("light-c"),
        json!({
            "on": {"on": true},
            "dimming": {"brightness": 50.0},
            "color_temperature": {"mirek": 366},
            "dynamics": {"duration": 400}
        })
    );

    // Nothing is restored twice.
    light.shutdown();
    assert_eq!(mock.restores().len(), 3);
}

#[test]
fn a_light_that_cannot_be_read_is_driven_but_not_restored() {
    let mock = Mock::start();
    mock.unreachable.lock().unwrap().push("light-b".to_owned());
    let (mut light, _handle) = light_with(&mock, Some("ent-1"), &[]);
    light.set_color(Rgb::new(255, 0, 0)).unwrap();
    wait_for("both driven", || mock.puts().len() >= 2);
    light.set_enabled(false);
    wait_for("a restore", || !mock.restores().is_empty());
    std::thread::sleep(GAP * 8);
    assert_eq!(restored_ids(&mock), ["light-a"]);
    light.shutdown();
}

#[test]
fn changing_the_area_restores_only_the_lights_that_left() {
    let mock = Mock::start();
    let (mut light, handle) = light_with(&mock, Some("ent-1"), &[]);
    light.set_color(Rgb::new(255, 0, 0)).unwrap();
    wait_for("both driven", || mock.puts().len() >= 2);

    // ent-2 keeps light-a and replaces light-b with light-c.
    handle.set_target(Some(mock.creds()), Some("ent-2".to_owned()));
    wait_for("light-b restored", || !mock.restores().is_empty());
    wait_for("light-c driven", || {
        mock.puts().iter().any(|p| p.path == put_path("light-c"))
    });
    assert_eq!(restored_ids(&mock), ["light-b"]);
    // light-a keeps its first snapshot; light-c is read before its first PUT.
    assert_eq!(mock.count("GET", &put_path("light-a")), 1);
    let log = mock.log();
    let get = log
        .iter()
        .position(|(m, p)| m == "GET" && *p == put_path("light-c"))
        .unwrap();
    let put = log
        .iter()
        .position(|(m, p)| m == "PUT" && *p == put_path("light-c"))
        .unwrap();
    assert!(get < put);

    light.shutdown();
    let mut restored = restored_ids(&mock);
    restored.sort();
    assert_eq!(restored, ["light-a", "light-b", "light-c"]);
}

#[test]
fn removing_an_extra_light_restores_it() {
    let mock = Mock::start();
    let (mut light, handle) = light_with(&mock, Some("ent-1"), &["light-c"]);
    light.set_color(Rgb::new(255, 0, 0)).unwrap();
    wait_for("all driven", || mock.puts().len() >= 3);

    handle.set_extra_lights(Vec::new());
    wait_for("light-c restored", || !mock.restores().is_empty());
    assert_eq!(restored_ids(&mock), ["light-c"]);
    assert_eq!(ids(&handle), ["light-a", "light-b"]);
    light.shutdown();
    assert_eq!(mock.restores().len(), 3);
}

#[test]
fn forgetting_the_bridge_restores_every_light() {
    let mock = Mock::start();
    let (mut light, handle) = light_with(&mock, Some("ent-1"), &[]);
    light.set_color(Rgb::new(255, 0, 0)).unwrap();
    wait_for("both driven", || mock.puts().len() >= 2);

    handle.set_target(None, None);
    wait_for("both restored", || mock.restores().len() >= 2);
    wait_for("not paired", || {
        light.status().code == Some(LightCode::NotPaired)
    });
    assert_eq!(handle.area_lights(), None);
    light.shutdown();
    assert_eq!(mock.restores().len(), 2);
}

#[test]
fn shutdown_restores_before_returning() {
    let mock = Mock::start();
    let (mut light, _handle) = light_with(&mock, Some("ent-1"), &["light-c"]);
    light.set_color(Rgb::new(255, 0, 0)).unwrap();
    wait_for("all driven", || mock.puts().len() >= 3);
    let started = Instant::now();
    light.shutdown();
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(mock.restores().len(), 3);
    light.shutdown();
    assert_eq!(mock.restores().len(), 3);
}

#[test]
fn a_light_that_never_received_a_colour_is_not_restored() {
    let mock = Mock::start();
    let (mut light, _handle) = light_with(&mock, Some("ent-1"), &[]);
    light.set_enabled(false);
    wait_for("disabled", || light.status().state == LightState::Disabled);
    light.shutdown();
    assert!(mock.puts().is_empty());
}

#[test]
fn a_reconnect_within_a_session_does_not_read_the_lights_again() {
    let mock = Mock::start();
    let (mut light, handle) = light_with(&mock, Some("ent-1"), &[]);
    light.set_color(Rgb::new(255, 0, 0)).unwrap();
    wait_for("both driven", || mock.puts().len() >= 2);
    // Adding a light reconnects; the first reconnect attempt is dropped.
    mock.drop_inventory.store(1, Ordering::SeqCst);
    handle.set_extra_lights(vec!["light-c".to_owned()]);
    wait_for("light-c driven", || {
        mock.puts().iter().any(|p| p.path == put_path("light-c"))
    });
    for id in ["light-a", "light-b", "light-c"] {
        assert_eq!(mock.count("GET", &put_path(id)), 1, "{id} is read once");
    }
    assert!(mock.restores().is_empty());
    light.shutdown();
    assert_eq!(mock.restores().len(), 3);
}
