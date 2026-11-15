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
    /// Number of upcoming `GET .../room` requests answered by dropping the connection.
    drop_inventory: Arc<AtomicU32>,
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
            stop: Arc::default(),
        };
        let requests = Arc::clone(&mock.requests);
        let link_pressed = Arc::clone(&mock.link_pressed);
        let drop_inventory = Arc::clone(&mock.drop_inventory);
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
                let stop = Arc::clone(&stop);
                std::thread::spawn(move || {
                    let conn = rustls::ServerConnection::new(config).unwrap();
                    let mut reader = BufReader::new(rustls::StreamOwned::new(conn, stream));
                    while !stop.load(Ordering::SeqCst) {
                        let Some(request) = read_request(&mut reader) else {
                            return;
                        };
                        requests.lock().unwrap().push(request.clone());
                        if request.path.ends_with("/room")
                            && drop_inventory.load(Ordering::SeqCst) > 0
                        {
                            drop_inventory.fetch_sub(1, Ordering::SeqCst);
                            return;
                        }
                        let (status, body) = route(&request, &link_pressed);
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

    fn count(&self, method: &str, path: &str) -> usize {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.method == method && r.path == path)
            .count()
    }
}

fn route(request: &Request, link_pressed: &AtomicBool) -> (&'static str, String) {
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
        ("GET", "/clip/v2/resource/room") => ok(fixtures::ROOMS),
        ("GET", "/clip/v2/resource/zone") => ok(fixtures::ZONES),
        ("GET", "/clip/v2/resource/entertainment_configuration") => ok(fixtures::ENTERTAINMENT),
        ("GET", "/clip/v2/resource/device") => ok(fixtures::DEVICES),
        ("GET", "/clip/v2/resource/light") => ok(fixtures::LIGHTS),
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
            ("Bedroom", AreaKind::Room, 1),
            ("Living room", AreaKind::Room, 2),
            ("Desk", AreaKind::Zone, 1),
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
    let (mut light, _handle) = light_for(&mock, "room-1");
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
    let (mut light, _handle) = light_for(&mock, "room-2");
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

    handle.set_target(Some(mock.creds()), Some("room-1".to_owned()));
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
        HueLight::with_timing(Some(creds), Some("room-1".to_owned()), test_timing());
    wait_for("error", || light.status().state == LightState::Error);
    let detail = light.status().detail.unwrap();
    assert!(detail.contains("app key"), "{detail}");
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        mock.count("GET", "/clip/v2/resource/room"),
        1,
        "no retry loop"
    );

    // A retry request reconnects once more.
    light.retry();
    wait_for("second attempt", || {
        mock.count("GET", "/clip/v2/resource/room") == 2
    });
    light.shutdown();
}

#[test]
fn a_changed_certificate_is_an_error_state() {
    let mock = Mock::start();
    let mut creds = mock.creds();
    creds.cert_sha256 = "f".repeat(64);
    let (mut light, _handle) =
        HueLight::with_timing(Some(creds), Some("room-1".to_owned()), test_timing());
    wait_for("error", || light.status().state == LightState::Error);
    assert!(light.status().detail.unwrap().contains("certificate"));
    assert!(mock.requests.lock().unwrap().is_empty());
    light.shutdown();
}

#[test]
fn lost_connection_recovers_with_backoff() {
    let mock = Mock::start();
    mock.drop_inventory.store(3, Ordering::SeqCst);
    let (mut light, _handle) = light_for(&mock, "room-2");
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
    assert!(mock.count("GET", "/clip/v2/resource/room") >= 4);
    light.shutdown();
}

#[test]
fn disabling_stops_sending_and_enabling_resumes() {
    let mock = Mock::start();
    let (mut light, _handle) = light_for(&mock, "room-2");
    light.set_color(Rgb::new(255, 0, 0)).unwrap();
    wait_for("first PUT", || !mock.puts().is_empty());
    light.set_enabled(false);
    assert_eq!(light.status().state, LightState::Disabled);
    std::thread::sleep(GAP * 3);
    let sent = mock.puts().len();
    light.set_color(Rgb::new(0, 0, 255)).unwrap();
    std::thread::sleep(GAP * 5);
    assert_eq!(mock.puts().len(), sent);
    light.set_enabled(true);
    wait_for("PUT after enabling", || mock.puts().len() > sent);
    light.shutdown();
}

#[test]
fn shutdown_joins_the_worker_and_is_idempotent() {
    let mock = Mock::start();
    let (mut light, handle) = light_for(&mock, "room-1");
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
