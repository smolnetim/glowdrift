//! Bridge wire formats: the v1 config and pairing replies and the CLIP v2 resources needed to
//! turn an area into its lights.

use std::collections::HashMap;

use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::Value;

use super::color::{Gamut, Xy};
use super::{AreaKind, HueArea, HueError};

/// `GET /api/0/config` (public part).
#[derive(Debug, Deserialize)]
pub(crate) struct ConfigReply {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub bridgeid: String,
}

/// Normalises a bridge id to the uppercase form the bridge reports.
pub(crate) fn normalize_bridge_id(id: &str) -> String {
    id.trim().to_ascii_uppercase()
}

pub(crate) enum PairReply {
    Success {
        username: String,
        client_key: String,
    },
    LinkButtonNotPressed,
}

const ERROR_LINK_BUTTON: i64 = 101;

/// Parses the reply to `POST /api`: a one-element array holding `success` or `error`.
pub(crate) fn parse_pair_reply(body: &[u8]) -> Result<PairReply, HueError> {
    let entries: Vec<Value> =
        serde_json::from_slice(body).map_err(|e| HueError::Parse(e.to_string()))?;
    let entry = entries
        .first()
        .ok_or_else(|| HueError::Parse("empty pairing reply".to_owned()))?;
    if let Some(error) = entry.get("error") {
        if error.get("type").and_then(Value::as_i64) == Some(ERROR_LINK_BUTTON) {
            return Ok(PairReply::LinkButtonNotPressed);
        }
        let description = error
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("pairing failed");
        return Err(HueError::Bridge(description.to_owned()));
    }
    let success = entry
        .get("success")
        .ok_or_else(|| HueError::Parse("pairing reply has neither success nor error".to_owned()))?;
    let text = |key: &str| success.get(key).and_then(Value::as_str).map(str::to_owned);
    let username = text("username")
        .ok_or_else(|| HueError::Parse("pairing reply has no username".to_owned()))?;
    Ok(PairReply::Success {
        username,
        client_key: text("clientkey").unwrap_or_default(),
    })
}

#[derive(Debug, Deserialize)]
struct V2Reply<T> {
    #[serde(default = "Vec::new")]
    errors: Vec<V2Error>,
    #[serde(default = "Vec::new")]
    data: Vec<T>,
}

#[derive(Debug, Deserialize)]
struct V2Error {
    #[serde(default)]
    description: String,
}

/// Interprets a CLIP v2 reply: auth failures, error bodies, then the `data` array.
pub(crate) fn parse_v2<T: for<'de> Deserialize<'de>>(
    status: StatusCode,
    body: &[u8],
) -> Result<Vec<T>, HueError> {
    if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        return Err(HueError::Unauthorized);
    }
    let reply = serde_json::from_slice::<V2Reply<T>>(body);
    if !status.is_success() {
        let detail = reply
            .ok()
            .and_then(|r| r.errors.into_iter().next())
            .map(|e| e.description)
            .filter(|d| !d.is_empty());
        return Err(HueError::Bridge(
            detail.unwrap_or_else(|| format!("HTTP {status}")),
        ));
    }
    let reply = reply.map_err(|e| HueError::Parse(e.to_string()))?;
    if let Some(error) = reply.errors.into_iter().next() {
        return Err(HueError::Bridge(error.description));
    }
    Ok(reply.data)
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ResourceRef {
    pub rid: String,
    pub rtype: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Metadata {
    #[serde(default)]
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Group {
    pub id: String,
    #[serde(default)]
    pub metadata: Option<Metadata>,
    #[serde(default)]
    pub children: Vec<ResourceRef>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Device {
    pub id: String,
    #[serde(default)]
    pub services: Vec<ResourceRef>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Member {
    pub service: ResourceRef,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Channel {
    #[serde(default)]
    pub members: Vec<Member>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct EntertainmentConfiguration {
    pub id: String,
    #[serde(default)]
    pub metadata: Option<Metadata>,
    #[serde(default)]
    pub light_services: Vec<ResourceRef>,
    #[serde(default)]
    pub channels: Vec<Channel>,
}

#[derive(Debug, Deserialize)]
struct XyReply {
    x: f64,
    y: f64,
}

#[derive(Debug, Deserialize)]
struct GamutReply {
    red: XyReply,
    green: XyReply,
    blue: XyReply,
}

#[derive(Debug, Deserialize)]
struct ColorReply {
    #[serde(default)]
    gamut: Option<GamutReply>,
    #[serde(default)]
    gamut_type: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct LightResource {
    pub id: String,
    #[serde(default)]
    color: Option<ColorReply>,
    #[serde(default)]
    dimming: Option<Value>,
}

/// What the worker needs to drive one light.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LightInfo {
    pub id: String,
    /// `None` for lights without colour support.
    pub gamut: Option<Gamut>,
    pub dimmable: bool,
}

impl LightResource {
    pub fn info(&self) -> LightInfo {
        let gamut = self.color.as_ref().map(|color| {
            color
                .gamut
                .as_ref()
                .map(|g| Gamut {
                    red: Xy::new(g.red.x, g.red.y),
                    green: Xy::new(g.green.x, g.green.y),
                    blue: Xy::new(g.blue.x, g.blue.y),
                })
                .filter(Gamut::is_valid)
                .or_else(|| color.gamut_type.as_deref().and_then(Gamut::from_type))
                .unwrap_or(Gamut::C)
        });
        LightInfo {
            id: self.id.clone(),
            gamut,
            dimmable: self.dimming.is_some(),
        }
    }
}

/// Everything fetched from a bridge to resolve areas.
#[derive(Debug, Default)]
pub(crate) struct Inventory {
    pub rooms: Vec<Group>,
    pub zones: Vec<Group>,
    pub entertainment: Vec<EntertainmentConfiguration>,
    pub devices: Vec<Device>,
    pub lights: Vec<LightResource>,
}

impl Inventory {
    pub fn fetch(bridge: &super::client::Bridge) -> Result<Inventory, HueError> {
        Ok(Inventory {
            rooms: bridge.get_v2("room")?,
            zones: bridge.get_v2("zone")?,
            entertainment: bridge.get_v2("entertainment_configuration")?,
            devices: bridge.get_v2("device")?,
            lights: bridge.get_v2("light")?,
        })
    }

    /// Light ids of a device, in service order.
    fn device_lights<'a>(
        devices: &'a HashMap<&str, &Device>,
        device_id: &str,
    ) -> impl Iterator<Item = &'a str> {
        devices
            .get(device_id)
            .into_iter()
            .flat_map(|d| d.services.iter())
            .filter(|s| s.rtype == "light")
            .map(|s| s.rid.as_str())
    }

    fn light_ids(&self, refs: &[ResourceRef]) -> Vec<String> {
        let devices: HashMap<&str, &Device> =
            self.devices.iter().map(|d| (d.id.as_str(), d)).collect();
        let mut ids: Vec<String> = Vec::new();
        for r in refs {
            match r.rtype.as_str() {
                "light" => ids.push(r.rid.clone()),
                "device" => ids.extend(Self::device_lights(&devices, &r.rid).map(str::to_owned)),
                _ => {}
            }
        }
        self.known_unique(ids)
    }

    /// Keeps ids that exist as light resources, once each, in order.
    fn known_unique(&self, ids: Vec<String>) -> Vec<String> {
        let known: std::collections::HashSet<&str> =
            self.lights.iter().map(|l| l.id.as_str()).collect();
        let mut seen = std::collections::HashSet::new();
        ids.into_iter()
            .filter(|id| known.contains(id.as_str()) && seen.insert(id.clone()))
            .collect()
    }

    fn entertainment_lights(&self, config: &EntertainmentConfiguration) -> Vec<String> {
        let mut refs: Vec<ResourceRef> = config
            .light_services
            .iter()
            .filter(|r| r.rtype == "light")
            .cloned()
            .collect();
        if refs.is_empty() {
            // Fall back to the lights owning the entertainment services in the channels.
            for member in config.channels.iter().flat_map(|c| &c.members) {
                if member.service.rtype != "entertainment" {
                    continue;
                }
                if let Some(device) = self
                    .devices
                    .iter()
                    .find(|d| d.services.iter().any(|s| s.rid == member.service.rid))
                {
                    refs.push(ResourceRef {
                        rid: device.id.clone(),
                        rtype: "device".to_owned(),
                    });
                }
            }
        }
        self.light_ids(&refs)
    }

    /// All areas with their light ids, rooms first, then zones, then entertainment
    /// configurations, each group sorted by name.
    pub fn areas(&self) -> Vec<(HueArea, Vec<String>)> {
        let mut out = Vec::new();
        let mut push = |kind: AreaKind, id: &str, name: &str, lights: Vec<String>| {
            out.push((
                HueArea {
                    id: id.to_owned(),
                    kind,
                    name: name.to_owned(),
                    light_count: u32::try_from(lights.len()).unwrap_or(u32::MAX),
                },
                lights,
            ));
        };
        let name_of = |m: &Option<Metadata>| m.as_ref().map(|m| m.name.clone()).unwrap_or_default();
        let mut groups: Vec<(AreaKind, &Group)> = self
            .rooms
            .iter()
            .map(|g| (AreaKind::Room, g))
            .chain(self.zones.iter().map(|g| (AreaKind::Zone, g)))
            .collect();
        groups.sort_by_key(|(kind, g)| (*kind as u8, name_of(&g.metadata).to_lowercase()));
        for (kind, group) in groups {
            push(
                kind,
                &group.id,
                &name_of(&group.metadata),
                self.light_ids(&group.children),
            );
        }
        let mut configs: Vec<&EntertainmentConfiguration> = self.entertainment.iter().collect();
        configs.sort_by_key(|c| name_of(&c.metadata).to_lowercase());
        for config in configs {
            push(
                AreaKind::Entertainment,
                &config.id,
                &name_of(&config.metadata),
                self.entertainment_lights(config),
            );
        }
        out
    }

    /// The lights of one area, or `None` if no area has that id.
    pub fn area_lights(&self, area_id: &str) -> Option<Vec<LightInfo>> {
        let (_, ids) = self.areas().into_iter().find(|(a, _)| a.id == area_id)?;
        let by_id: HashMap<&str, &LightResource> =
            self.lights.iter().map(|l| (l.id.as_str(), l)).collect();
        Some(
            ids.iter()
                .filter_map(|id| by_id.get(id.as_str()).map(|l| l.info()))
                .collect(),
        )
    }
}

#[cfg(test)]
pub(crate) mod fixtures {
    //! CLIP v2 fixtures shared by unit and mock-bridge tests.

    pub const ROOMS: &str = r#"{"errors":[],"data":[
      {"id":"room-1","type":"room","metadata":{"name":"Living room","archetype":"living_room"},
       "children":[{"rid":"dev-a","rtype":"device"},{"rid":"dev-b","rtype":"device"}],
       "services":[{"rid":"gl-1","rtype":"grouped_light"}]},
      {"id":"room-2","type":"room","metadata":{"name":"Bedroom"},
       "children":[{"rid":"dev-c","rtype":"device"}],"services":[]}]}"#;
    pub const ZONES: &str = r#"{"errors":[],"data":[
      {"id":"zone-1","type":"zone","metadata":{"name":"Desk"},
       "children":[{"rid":"light-b","rtype":"light"},{"rid":"light-b","rtype":"light"},
                   {"rid":"light-gone","rtype":"light"}]}]}"#;
    pub const ENTERTAINMENT: &str = r#"{"errors":[],"data":[
      {"id":"ent-1","type":"entertainment_configuration","metadata":{"name":"TV"},
       "configuration_type":"screen","status":"inactive",
       "light_services":[{"rid":"light-a","rtype":"light"},{"rid":"light-c","rtype":"light"}]}]}"#;
    pub const DEVICES: &str = r#"{"errors":[],"data":[
      {"id":"dev-a","services":[{"rid":"light-a","rtype":"light"},{"rid":"zb-a","rtype":"zigbee_connectivity"}]},
      {"id":"dev-b","services":[{"rid":"light-b","rtype":"light"}]},
      {"id":"dev-c","services":[{"rid":"light-c","rtype":"light"}]}]}"#;
    pub const LIGHTS: &str = r#"{"errors":[],"data":[
      {"id":"light-a","owner":{"rid":"dev-a","rtype":"device"},"on":{"on":true},
       "dimming":{"brightness":50.0},
       "color":{"xy":{"x":0.3,"y":0.3},
         "gamut":{"red":{"x":0.6915,"y":0.3083},"green":{"x":0.17,"y":0.7},"blue":{"x":0.1532,"y":0.0475}},
         "gamut_type":"C"}},
      {"id":"light-b","owner":{"rid":"dev-b","rtype":"device"},"on":{"on":true},
       "dimming":{"brightness":50.0},
       "color":{"xy":{"x":0.3,"y":0.3},"gamut_type":"B"}},
      {"id":"light-c","owner":{"rid":"dev-c","rtype":"device"},"on":{"on":true},
       "dimming":{"brightness":50.0}}]}"#;
    pub const PAIR_OK: &str =
        r#"[{"success":{"username":"app-key-123","clientkey":"test-client-key"}}]"#;
    pub const PAIR_LINK_BUTTON: &str =
        r#"[{"error":{"type":101,"address":"","description":"link button not pressed"}}]"#;
    pub const CONFIG: &str = r#"{"name":"Philips hue","apiversion":"1.78.0","bridgeid":"AABBCCFFFE112233","modelid":"BSB002"}"#;
}

#[cfg(test)]
mod tests {
    use super::fixtures::*;
    use super::*;

    fn inventory() -> Inventory {
        let ok = StatusCode::OK;
        Inventory {
            rooms: parse_v2(ok, ROOMS.as_bytes()).unwrap(),
            zones: parse_v2(ok, ZONES.as_bytes()).unwrap(),
            entertainment: parse_v2(ok, ENTERTAINMENT.as_bytes()).unwrap(),
            devices: parse_v2(ok, DEVICES.as_bytes()).unwrap(),
            lights: parse_v2(ok, LIGHTS.as_bytes()).unwrap(),
        }
    }

    #[test]
    fn pairing_success() {
        let PairReply::Success {
            username,
            client_key,
        } = parse_pair_reply(PAIR_OK.as_bytes()).unwrap()
        else {
            panic!("expected success");
        };
        assert_eq!(username, "app-key-123");
        assert_eq!(client_key, "test-client-key");
    }

    #[test]
    fn pairing_link_button_not_pressed() {
        assert!(matches!(
            parse_pair_reply(PAIR_LINK_BUTTON.as_bytes()),
            Ok(PairReply::LinkButtonNotPressed)
        ));
    }

    #[test]
    fn pairing_other_errors_and_garbage() {
        let other = br#"[{"error":{"type":7,"description":"invalid value"}}]"#;
        assert!(matches!(
            parse_pair_reply(other),
            Err(HueError::Bridge(d)) if d == "invalid value"
        ));
        assert!(matches!(parse_pair_reply(b"[]"), Err(HueError::Parse(_))));
        assert!(matches!(parse_pair_reply(b"nope"), Err(HueError::Parse(_))));
        assert!(matches!(
            parse_pair_reply(br#"[{"success":{}}]"#),
            Err(HueError::Parse(_))
        ));
    }

    #[test]
    fn config_reply() {
        let config: ConfigReply = serde_json::from_str(CONFIG).unwrap();
        assert_eq!(config.bridgeid, "AABBCCFFFE112233");
        assert_eq!(config.name.as_deref(), Some("Philips hue"));
    }

    #[test]
    fn v2_status_handling() {
        assert!(matches!(
            parse_v2::<Value>(StatusCode::FORBIDDEN, b"{}"),
            Err(HueError::Unauthorized)
        ));
        assert!(matches!(
            parse_v2::<Value>(StatusCode::UNAUTHORIZED, b""),
            Err(HueError::Unauthorized)
        ));
        let rate = br#"{"errors":[{"description":"too many requests"}]}"#;
        assert!(matches!(
            parse_v2::<Value>(StatusCode::TOO_MANY_REQUESTS, rate),
            Err(HueError::Bridge(d)) if d == "too many requests"
        ));
        assert!(matches!(
            parse_v2::<Value>(StatusCode::BAD_GATEWAY, b"<html>"),
            Err(HueError::Bridge(d)) if d.contains("502")
        ));
        let in_body = br#"{"errors":[{"description":"nope"}],"data":[]}"#;
        assert!(matches!(
            parse_v2::<Value>(StatusCode::OK, in_body),
            Err(HueError::Bridge(d)) if d == "nope"
        ));
        assert!(matches!(
            parse_v2::<Value>(StatusCode::OK, b"{"),
            Err(HueError::Parse(_))
        ));
    }

    #[test]
    fn areas_resolve_to_lights_in_order() {
        let areas = inventory().areas();
        let summary: Vec<_> = areas
            .iter()
            .map(|(a, ids)| {
                (
                    a.id.as_str(),
                    a.kind,
                    a.name.as_str(),
                    a.light_count,
                    ids.clone(),
                )
            })
            .collect();
        assert_eq!(
            summary,
            vec![
                (
                    "room-2",
                    AreaKind::Room,
                    "Bedroom",
                    1,
                    vec!["light-c".to_owned()]
                ),
                (
                    "room-1",
                    AreaKind::Room,
                    "Living room",
                    2,
                    vec!["light-a".to_owned(), "light-b".to_owned()]
                ),
                (
                    "zone-1",
                    AreaKind::Zone,
                    "Desk",
                    1,
                    vec!["light-b".to_owned()]
                ),
                (
                    "ent-1",
                    AreaKind::Entertainment,
                    "TV",
                    2,
                    vec!["light-a".to_owned(), "light-c".to_owned()]
                ),
            ]
        );
    }

    #[test]
    fn entertainment_falls_back_to_channel_members() {
        let mut inv = inventory();
        inv.devices[0].services.push(ResourceRef {
            rid: "ent-svc-a".to_owned(),
            rtype: "entertainment".to_owned(),
        });
        inv.entertainment = serde_json::from_str::<V2Reply<EntertainmentConfiguration>>(
            r#"{"data":[{"id":"ent-2","metadata":{"name":"Music"},
                "channels":[{"members":[{"service":{"rid":"ent-svc-a","rtype":"entertainment"}}]}]}]}"#,
        )
        .unwrap()
        .data;
        let lights = inv.area_lights("ent-2").unwrap();
        assert_eq!(lights.len(), 1);
        assert_eq!(lights[0].id, "light-a");
    }

    #[test]
    fn light_gamuts_come_from_the_bridge_then_the_type_then_c() {
        let inv = inventory();
        let lights = inv.area_lights("ent-1").unwrap();
        assert_eq!(lights[0].gamut, Some(Gamut::C));
        assert!(lights[0].dimmable);
        // light-c has no colour support.
        assert_eq!(lights[1].gamut, None);

        let living = inv.area_lights("room-1").unwrap();
        // light-b reports only a gamut type.
        assert_eq!(living[1].gamut, Some(Gamut::B));

        let unknown: LightResource = serde_json::from_str(
            r#"{"id":"x","color":{"gamut_type":"other"},"dimming":{"brightness":1}}"#,
        )
        .unwrap();
        assert_eq!(unknown.info().gamut, Some(Gamut::C));
        let degenerate: LightResource = serde_json::from_str(
            r#"{"id":"x","color":{"gamut":{"red":{"x":0,"y":0},"green":{"x":0,"y":0},"blue":{"x":0,"y":0}},"gamut_type":"A"}}"#,
        )
        .unwrap();
        assert_eq!(degenerate.info().gamut, Some(Gamut::A));
        assert!(!degenerate.info().dimmable);
    }

    #[test]
    fn unknown_area_is_none() {
        assert!(inventory().area_lights("nope").is_none());
    }

    #[test]
    fn bridge_ids_are_uppercased() {
        assert_eq!(
            normalize_bridge_id(" aabbccFFfe112233 "),
            "AABBCCFFFE112233"
        );
    }
}
