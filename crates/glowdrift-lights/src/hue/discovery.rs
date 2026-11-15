//! Finding bridges: mDNS (`_hue._tcp.local`) first, the Signify discovery service when mDNS
//! finds nothing, and a direct probe of a manually entered address.

use std::net::Ipv4Addr;
use std::time::{Duration, Instant};

use mdns_sd::{ServiceDaemon, ServiceEvent};
use serde::Deserialize;

use super::api::{ConfigReply, normalize_bridge_id};
use super::client::Bridge;
use super::{BridgeCandidate, HueError};

const SERVICE_TYPE: &str = "_hue._tcp.local.";
const CLOUD_URL: &str = "https://discovery.meethue.com";
/// After the first bridge answers, wait this long for further bridges before returning.
const MDNS_GRACE: Duration = Duration::from_millis(750);
const CLOUD_TIMEOUT: Duration = Duration::from_secs(4);

/// Finds bridges on the local network, waiting at most about `timeout` for mDNS answers.
///
/// Bridges announce themselves over mDNS; only when none answers is the Signify discovery
/// service asked (which sends the caller's public address to the internet). Results are
/// deduplicated by bridge id. Blocking.
pub fn discover(timeout: Duration) -> Vec<BridgeCandidate> {
    let mut found = discover_mdns(timeout);
    if found.is_empty() {
        match discover_cloud() {
            Ok(list) => found = list,
            Err(err) => log::debug!("Hue cloud discovery failed: {err}"),
        }
    }
    dedup(found)
}

/// Reads the public `/api/0/config` of a bridge at `address` (`host` or `host:port`).
///
/// The certificate is not verified for this call; it only identifies the bridge.
pub fn probe(address: &str) -> Result<BridgeCandidate, HueError> {
    let bridge = Bridge::new(address, None, None, super::client::REQUEST_TIMEOUT)?;
    let config: ConfigReply = bridge.get_v1("/api/0/config")?;
    candidate_from_config(address, &config)
}

pub(crate) fn candidate_from_config(
    address: &str,
    config: &ConfigReply,
) -> Result<BridgeCandidate, HueError> {
    let id = normalize_bridge_id(&config.bridgeid);
    if id.is_empty() {
        return Err(HueError::Parse(
            "the device did not report a bridge id".to_owned(),
        ));
    }
    Ok(BridgeCandidate {
        id,
        address: address.trim().to_owned(),
        name: config.name.clone().filter(|n| !n.is_empty()),
    })
}

fn dedup(candidates: Vec<BridgeCandidate>) -> Vec<BridgeCandidate> {
    let mut seen = std::collections::HashSet::new();
    candidates
        .into_iter()
        .filter(|c| seen.insert(c.id.clone()))
        .collect()
}

/// Builds a candidate from an mDNS answer: the bridge id TXT value, the instance's full name
/// and its IPv4 addresses. Returns `None` without a bridge id or an IPv4 address.
pub(crate) fn candidate_from_mdns(
    fullname: &str,
    bridge_id: Option<&str>,
    mut addresses: Vec<Ipv4Addr>,
) -> Option<BridgeCandidate> {
    let id = normalize_bridge_id(bridge_id?);
    addresses.sort();
    let address = addresses.first()?.to_string();
    let name = fullname
        .split("._hue.")
        .next()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_owned);
    (!id.is_empty()).then_some(BridgeCandidate { id, address, name })
}

fn discover_mdns(timeout: Duration) -> Vec<BridgeCandidate> {
    let daemon = match ServiceDaemon::new() {
        Ok(daemon) => daemon,
        Err(err) => {
            log::debug!("mDNS unavailable: {err}");
            return Vec::new();
        }
    };
    let events = match daemon.browse(SERVICE_TYPE) {
        Ok(events) => events,
        Err(err) => {
            log::debug!("mDNS browse failed: {err}");
            let _ = daemon.shutdown();
            return Vec::new();
        }
    };
    let mut deadline = Instant::now() + timeout;
    let mut found = Vec::new();
    while let Some(wait) = deadline.checked_duration_since(Instant::now()) {
        let Ok(event) = events.recv_timeout(wait) else {
            break;
        };
        if let ServiceEvent::ServiceResolved(info) = event {
            let candidate = candidate_from_mdns(
                info.get_fullname(),
                info.get_property_val_str("bridgeid"),
                info.get_addresses_v4().into_iter().collect(),
            );
            if let Some(candidate) = candidate {
                if found.is_empty() {
                    deadline = deadline.min(Instant::now() + MDNS_GRACE);
                }
                found.push(candidate);
            }
        }
    }
    let _ = daemon.stop_browse(SERVICE_TYPE);
    let _ = daemon.shutdown();
    found
}

#[derive(Debug, Deserialize)]
struct CloudEntry {
    id: String,
    internalipaddress: String,
}

pub(crate) fn parse_cloud_list(body: &[u8]) -> Result<Vec<BridgeCandidate>, HueError> {
    let entries: Vec<CloudEntry> =
        serde_json::from_slice(body).map_err(|e| HueError::Parse(e.to_string()))?;
    Ok(entries
        .into_iter()
        .filter(|e| !e.id.trim().is_empty() && !e.internalipaddress.trim().is_empty())
        .map(|e| BridgeCandidate {
            id: normalize_bridge_id(&e.id),
            address: e.internalipaddress.trim().to_owned(),
            name: None,
        })
        .collect())
}

fn discover_cloud() -> Result<Vec<BridgeCandidate>, HueError> {
    super::ensure_tls_provider();
    let client = reqwest::blocking::Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .timeout(CLOUD_TIMEOUT)
        .user_agent(concat!("Glowdrift/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| HueError::Network(super::client::describe(e)))?;
    let describe = |e: reqwest::Error| HueError::Network(super::client::describe(e));
    let response = client.get(CLOUD_URL).send().map_err(describe)?;
    if !response.status().is_success() {
        return Err(HueError::Bridge(format!("HTTP {}", response.status())));
    }
    let body = response.bytes().map_err(describe)?;
    parse_cloud_list(&body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cloud_list_is_parsed_and_normalised() {
        let body = br#"[{"id":"001788fffe100491","internalipaddress":"192.168.1.2","port":443},
                        {"id":"","internalipaddress":"10.0.0.1"}]"#;
        let list = parse_cloud_list(body).unwrap();
        assert_eq!(
            list,
            vec![BridgeCandidate {
                id: "001788FFFE100491".to_owned(),
                address: "192.168.1.2".to_owned(),
                name: None,
            }]
        );
        assert!(matches!(parse_cloud_list(b"x"), Err(HueError::Parse(_))));
    }

    #[test]
    fn mdns_answer_becomes_a_candidate() {
        let candidate = candidate_from_mdns(
            "Hue Bridge - 688B._hue._tcp.local.",
            Some("aabbccfffe112233"),
            vec![Ipv4Addr::new(192, 168, 1, 20), Ipv4Addr::new(10, 0, 0, 5)],
        )
        .unwrap();
        assert_eq!(candidate.id, "AABBCCFFFE112233");
        assert_eq!(candidate.address, "10.0.0.5");
        assert_eq!(candidate.name.as_deref(), Some("Hue Bridge - 688B"));
    }

    #[test]
    fn mdns_answer_needs_an_id_and_an_ipv4_address() {
        let ip = vec![Ipv4Addr::new(10, 0, 0, 5)];
        assert!(candidate_from_mdns("x._hue._tcp.local.", None, ip.clone()).is_none());
        assert!(candidate_from_mdns("x._hue._tcp.local.", Some(" "), ip).is_none());
        assert!(candidate_from_mdns("x._hue._tcp.local.", Some("AB"), vec![]).is_none());
    }

    #[test]
    fn duplicates_collapse_by_id() {
        let make = |id: &str, address: &str| BridgeCandidate {
            id: id.to_owned(),
            address: address.to_owned(),
            name: None,
        };
        let out = dedup(vec![make("A", "1"), make("B", "2"), make("A", "3")]);
        assert_eq!(out, vec![make("A", "1"), make("B", "2")]);
    }

    #[test]
    fn config_without_a_bridge_id_is_rejected() {
        let config = ConfigReply {
            name: Some("x".to_owned()),
            bridgeid: String::new(),
        };
        assert!(matches!(
            candidate_from_config("1.2.3.4", &config),
            Err(HueError::Parse(_))
        ));
    }
}
