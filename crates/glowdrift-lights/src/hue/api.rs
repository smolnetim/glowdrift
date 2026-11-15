//! Bridge wire formats: the v1 config and pairing replies and the CLIP v2 resources needed to
//! turn an entertainment area into its lights, with positions, and to read a light's state.

use std::collections::HashMap;

use glowdrift_core::LightPosition;
use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::Value;

use super::color::{Gamut, Xy};
use super::{AreaKind, BridgeLight, HueArea, HueError};

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
pub(crate) struct Device {
    pub id: String,
    #[serde(default)]
    pub metadata: Option<Metadata>,
    #[serde(default)]
    pub services: Vec<ResourceRef>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Member {
    pub service: ResourceRef,
}

#[derive(Debug, Deserialize)]
struct PositionReply {
    x: f64,
    y: f64,
    z: f64,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Channel {
    #[serde(default)]
    position: Option<PositionReply>,
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
    owner: Option<ResourceRef>,
    #[serde(default)]
    metadata: Option<Metadata>,
    #[serde(default)]
    color: Option<ColorReply>,
    #[serde(default)]
    dimming: Option<Value>,
}

/// What the worker needs to drive one light.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LightInfo {
    pub id: String,
    pub name: String,
    /// `None` for lights without colour support.
    pub gamut: Option<Gamut>,
    pub dimmable: bool,
}

impl LightResource {
    /// `device_name` is the name of the owning device, used when the light has none itself.
    pub fn info(&self, device_name: Option<&str>) -> LightInfo {
        let name = [self.metadata.as_ref().map(|m| m.name.as_str()), device_name]
            .into_iter()
            .flatten()
            .map(str::trim)
            .find(|n| !n.is_empty())
            .unwrap_or(&self.id)
            .to_owned();
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
            name,
            gamut,
            dimmable: self.dimming.is_some(),
        }
    }
}

/// Everything fetched from a bridge to resolve areas and lights.
#[derive(Debug, Default)]
pub(crate) struct Inventory {
    pub entertainment: Vec<EntertainmentConfiguration>,
    pub devices: Vec<Device>,
    pub lights: Vec<LightResource>,
}

/// A light of an entertainment area with its position, when the area has one for it.
pub(crate) type PositionedLight = (LightInfo, Option<LightPosition>);

/// Sum of channel positions and how many were added.
#[derive(Default)]
struct PositionSum {
    x: f64,
    y: f64,
    z: f64,
    count: u32,
}

impl PositionSum {
    fn add(&mut self, p: &PositionReply) {
        self.x += p.x;
        self.y += p.y;
        self.z += p.z;
        self.count += 1;
    }

    fn average(&self) -> Option<LightPosition> {
        (self.count > 0).then(|| {
            let n = f64::from(self.count);
            LightPosition {
                x: (self.x / n) as f32,
                y: (self.y / n) as f32,
                z: (self.z / n) as f32,
            }
        })
    }
}

impl Inventory {
    /// Everything needed to resolve entertainment areas.
    pub fn fetch(bridge: &super::client::Bridge) -> Result<Inventory, HueError> {
        Ok(Inventory {
            entertainment: bridge.get_v2("entertainment_configuration")?,
            devices: bridge.get_v2("device")?,
            lights: bridge.get_v2("light")?,
        })
    }

    /// Only the devices and lights, enough to list and name single lights.
    pub fn fetch_lights(bridge: &super::client::Bridge) -> Result<Inventory, HueError> {
        Ok(Inventory {
            devices: bridge.get_v2("device")?,
            lights: bridge.get_v2("light")?,
            ..Inventory::default()
        })
    }

    fn light_ids_of_device(device: &Device) -> impl Iterator<Item = &str> {
        device
            .services
            .iter()
            .filter(|s| s.rtype == "light")
            .map(|s| s.rid.as_str())
    }

    fn known_light(&self, id: &str) -> bool {
        self.lights.iter().any(|l| l.id == id)
    }

    /// The lights of an entertainment configuration in channel order, each with the average of
    /// the positions of the channels it appears in. Lights listed by the configuration but in
    /// no channel follow, without a position.
    fn entertainment_members(
        &self,
        config: &EntertainmentConfiguration,
    ) -> Vec<(String, Option<LightPosition>)> {
        let by_service: HashMap<&str, &Device> = self
            .devices
            .iter()
            .flat_map(|d| {
                d.services
                    .iter()
                    .filter(|s| s.rtype == "entertainment")
                    .map(move |s| (s.rid.as_str(), d))
            })
            .collect();
        let devices: HashMap<&str, &Device> =
            self.devices.iter().map(|d| (d.id.as_str(), d)).collect();
        let mut order: Vec<String> = Vec::new();
        let mut sums: HashMap<String, PositionSum> = HashMap::new();
        for channel in &config.channels {
            let position = channel
                .position
                .as_ref()
                .filter(|p| p.x.is_finite() && p.y.is_finite() && p.z.is_finite());
            let mut in_channel: Vec<&str> = Vec::new();
            for member in &channel.members {
                if member.service.rtype != "entertainment" {
                    continue;
                }
                let Some(device) = by_service.get(member.service.rid.as_str()) else {
                    continue;
                };
                for id in Self::light_ids_of_device(device) {
                    if !self.known_light(id) || in_channel.contains(&id) {
                        continue;
                    }
                    in_channel.push(id);
                    if !order.iter().any(|o| o == id) {
                        order.push(id.to_owned());
                    }
                    if let Some(p) = position {
                        sums.entry(id.to_owned()).or_default().add(p);
                    }
                }
            }
        }
        for r in &config.light_services {
            let ids: Vec<&str> = match r.rtype.as_str() {
                "light" => vec![r.rid.as_str()],
                "device" => devices
                    .get(r.rid.as_str())
                    .map(|d| Self::light_ids_of_device(d).collect())
                    .unwrap_or_default(),
                _ => Vec::new(),
            };
            for id in ids {
                if self.known_light(id) && !order.iter().any(|o| o == id) {
                    order.push(id.to_owned());
                }
            }
        }
        order
            .into_iter()
            .map(|id| {
                let position = sums.get(&id).and_then(PositionSum::average);
                (id, position)
            })
            .collect()
    }

    /// The entertainment areas with their light ids, sorted by name.
    pub fn areas(&self) -> Vec<(HueArea, Vec<String>)> {
        let name_of = |m: &Option<Metadata>| m.as_ref().map(|m| m.name.clone()).unwrap_or_default();
        let mut configs: Vec<&EntertainmentConfiguration> = self.entertainment.iter().collect();
        configs.sort_by_key(|c| name_of(&c.metadata).to_lowercase());
        configs
            .into_iter()
            .map(|config| {
                let ids: Vec<String> = self
                    .entertainment_members(config)
                    .into_iter()
                    .map(|(id, _)| id)
                    .collect();
                let area = HueArea {
                    id: config.id.clone(),
                    kind: AreaKind::Entertainment,
                    name: name_of(&config.metadata),
                    light_count: u32::try_from(ids.len()).unwrap_or(u32::MAX),
                };
                (area, ids)
            })
            .collect()
    }

    fn info_of(&self, light: &LightResource) -> LightInfo {
        let device_name = light
            .owner
            .as_ref()
            .and_then(|o| self.devices.iter().find(|d| d.id == o.rid))
            .and_then(|d| d.metadata.as_ref())
            .map(|m| m.name.as_str());
        light.info(device_name)
    }

    /// One light by id, or `None` if the bridge has no such light.
    pub fn light(&self, id: &str) -> Option<LightInfo> {
        self.lights
            .iter()
            .find(|l| l.id == id)
            .map(|l| self.info_of(l))
    }

    /// The lights of one entertainment area in channel order with their positions, or `None`
    /// if no area has that id.
    pub fn area_lights(&self, area_id: &str) -> Option<Vec<PositionedLight>> {
        let config = self.entertainment.iter().find(|c| c.id == area_id)?;
        Some(
            self.entertainment_members(config)
                .into_iter()
                .filter_map(|(id, position)| self.light(&id).map(|info| (info, position)))
                .collect(),
        )
    }

    /// All lights of the bridge: colour-capable lights first, each group sorted by name.
    pub fn bridge_lights(&self) -> Vec<BridgeLight> {
        let mut lights: Vec<LightInfo> = self.lights.iter().map(|l| self.info_of(l)).collect();
        lights.sort_by_key(|l| (l.gamut.is_none(), l.name.to_lowercase(), l.id.clone()));
        lights
            .into_iter()
            .map(|l| BridgeLight {
                id: l.id,
                name: l.name,
            })
            .collect()
    }
}

/// A light's state as read before the app changes it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LightSnapshot {
    pub on: bool,
    pub brightness: Option<f64>,
    pub xy: Option<(f64, f64)>,
    /// Set only when the light is in colour-temperature mode.
    pub mirek: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct OnReply {
    on: bool,
}

#[derive(Debug, Deserialize)]
struct DimmingReply {
    brightness: f64,
}

#[derive(Debug, Deserialize)]
struct StateColorReply {
    #[serde(default)]
    xy: Option<XyReply>,
}

#[derive(Debug, Deserialize)]
struct TemperatureReply {
    #[serde(default)]
    mirek: Option<u32>,
    #[serde(default)]
    mirek_valid: bool,
}

/// `GET /clip/v2/resource/light/{id}` data: the parts a restore needs.
#[derive(Debug, Deserialize)]
pub(crate) struct LightStateReply {
    #[serde(default)]
    on: Option<OnReply>,
    #[serde(default)]
    dimming: Option<DimmingReply>,
    #[serde(default)]
    color: Option<StateColorReply>,
    #[serde(default)]
    color_temperature: Option<TemperatureReply>,
}

impl LightStateReply {
    /// `None` when the light reports no on/off state.
    pub fn snapshot(&self) -> Option<LightSnapshot> {
        let on = self.on.as_ref()?.on;
        let mirek = self
            .color_temperature
            .as_ref()
            .filter(|t| t.mirek_valid)
            .and_then(|t| t.mirek);
        Some(LightSnapshot {
            on,
            brightness: self.dimming.as_ref().map(|d| d.brightness),
            xy: self
                .color
                .as_ref()
                .and_then(|c| c.xy.as_ref())
                .map(|xy| (xy.x, xy.y)),
            mirek,
        })
    }
}

#[cfg(test)]
pub(crate) mod fixtures {
    //! CLIP v2 fixtures shared by unit and mock-bridge tests.

    pub const ENTERTAINMENT: &str = r#"{"errors":[],"data":[
      {"id":"ent-1","type":"entertainment_configuration","metadata":{"name":"TV"},
       "configuration_type":"screen","status":"inactive",
       "light_services":[{"rid":"light-a","rtype":"light"},{"rid":"light-b","rtype":"light"}]},
      {"id":"ent-2","type":"entertainment_configuration","metadata":{"name":"Cinema"},
       "configuration_type":"screen","status":"inactive",
       "light_services":[{"rid":"light-c","rtype":"light"},{"rid":"light-a","rtype":"light"}],
       "channels":[
         {"channel_id":0,"position":{"x":-0.8,"y":0.5,"z":0.25},
          "members":[{"service":{"rid":"ent-a","rtype":"entertainment"},"index":0}]},
         {"channel_id":1,"position":{"x":0.5,"y":0.0,"z":0.0},
          "members":[{"service":{"rid":"ent-c","rtype":"entertainment"},"index":0}]},
         {"channel_id":2,"position":{"x":-0.4,"y":0.5,"z":0.75},
          "members":[{"service":{"rid":"ent-a","rtype":"entertainment"},"index":1}]}]}]}"#;
    pub const DEVICES: &str = r#"{"errors":[],"data":[
      {"id":"dev-a","metadata":{"name":"Floor lamp device"},
       "services":[{"rid":"light-a","rtype":"light"},{"rid":"zb-a","rtype":"zigbee_connectivity"},
                   {"rid":"ent-a","rtype":"entertainment"}]},
      {"id":"dev-b","metadata":{"name":"Desk lamp"},"services":[{"rid":"light-b","rtype":"light"}]},
      {"id":"dev-c","metadata":{"name":"Hall device"},
       "services":[{"rid":"light-c","rtype":"light"},{"rid":"ent-c","rtype":"entertainment"}]}]}"#;
    pub const LIGHTS: &str = r#"{"errors":[],"data":[
      {"id":"light-a","owner":{"rid":"dev-a","rtype":"device"},"metadata":{"name":"Floor lamp"},
       "on":{"on":true},"dimming":{"brightness":50.0},
       "color":{"xy":{"x":0.3,"y":0.3},
         "gamut":{"red":{"x":0.6915,"y":0.3083},"green":{"x":0.17,"y":0.7},"blue":{"x":0.1532,"y":0.0475}},
         "gamut_type":"C"},
       "color_temperature":{"mirek":null,"mirek_valid":false}},
      {"id":"light-b","owner":{"rid":"dev-b","rtype":"device"},"on":{"on":true},
       "dimming":{"brightness":50.0},
       "color":{"xy":{"x":0.3,"y":0.3},"gamut_type":"B"}},
      {"id":"light-c","owner":{"rid":"dev-c","rtype":"device"},"metadata":{"name":"Hall bulb"},
       "on":{"on":true},"dimming":{"brightness":50.0},
       "color_temperature":{"mirek":366,"mirek_valid":true}}]}"#;
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
    fn only_entertainment_areas_are_listed_sorted_by_name() {
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
                    "ent-2",
                    AreaKind::Entertainment,
                    "Cinema",
                    2,
                    vec!["light-a".to_owned(), "light-c".to_owned()]
                ),
                (
                    "ent-1",
                    AreaKind::Entertainment,
                    "TV",
                    2,
                    vec!["light-a".to_owned(), "light-b".to_owned()]
                ),
            ]
        );
    }

    #[test]
    fn channel_positions_are_parsed_and_averaged_per_light() {
        let lights = inventory().area_lights("ent-2").unwrap();
        // Channel order, not `light_services` order.
        let ids: Vec<_> = lights.iter().map(|(l, _)| l.id.as_str()).collect();
        assert_eq!(ids, ["light-a", "light-c"]);
        // light-a sits in channels 0 and 2: the average of both positions.
        let a = lights[0].1.unwrap();
        assert!((a.x + 0.6).abs() < 1e-6, "{a:?}");
        assert!((a.y - 0.5).abs() < 1e-6);
        assert!((a.z - 0.5).abs() < 1e-6);
        let c = lights[1].1.unwrap();
        assert_eq!((c.x, c.y, c.z), (0.5, 0.0, 0.0));
    }

    #[test]
    fn lights_without_channels_have_no_position() {
        let lights = inventory().area_lights("ent-1").unwrap();
        assert!(lights.iter().all(|(_, position)| position.is_none()));
    }

    #[test]
    fn unplaced_channels_leave_the_position_empty() {
        let mut inv = inventory();
        inv.entertainment = serde_json::from_str::<V2Reply<EntertainmentConfiguration>>(
            r#"{"data":[{"id":"e","metadata":{"name":"X"},
                "channels":[
                  {"members":[{"service":{"rid":"ent-a","rtype":"entertainment"}}]},
                  {"members":[{"service":{"rid":"ent-c","rtype":"entertainment"}}]}]}]}"#,
        )
        .unwrap()
        .data;
        let lights = inv.area_lights("e").unwrap();
        let ids: Vec<_> = lights.iter().map(|(l, _)| l.id.as_str()).collect();
        assert_eq!(ids, ["light-a", "light-c"]);
        assert!(lights.iter().all(|(_, position)| position.is_none()));
    }

    #[test]
    fn light_gamuts_come_from_the_bridge_then_the_type_then_c() {
        let inv = inventory();
        let tv = inv.area_lights("ent-1").unwrap();
        assert_eq!(tv[0].0.gamut, Some(Gamut::C));
        assert!(tv[0].0.dimmable);
        // light-b reports only a gamut type.
        assert_eq!(tv[1].0.gamut, Some(Gamut::B));
        // light-c has no colour support.
        let cinema = inv.area_lights("ent-2").unwrap();
        assert_eq!(cinema[1].0.gamut, None);

        let unknown: LightResource = serde_json::from_str(
            r#"{"id":"x","color":{"gamut_type":"other"},"dimming":{"brightness":1}}"#,
        )
        .unwrap();
        assert_eq!(unknown.info(None).gamut, Some(Gamut::C));
        let degenerate: LightResource = serde_json::from_str(
            r#"{"id":"x","color":{"gamut":{"red":{"x":0,"y":0},"green":{"x":0,"y":0},"blue":{"x":0,"y":0}},"gamut_type":"A"}}"#,
        )
        .unwrap();
        assert_eq!(degenerate.info(None).gamut, Some(Gamut::A));
        assert!(!degenerate.info(None).dimmable);
    }

    #[test]
    fn unknown_area_is_none() {
        assert!(inventory().area_lights("nope").is_none());
    }

    #[test]
    fn light_names_fall_back_to_the_device_then_the_id() {
        let inv = inventory();
        assert_eq!(inv.light("light-a").unwrap().name, "Floor lamp");
        assert_eq!(inv.light("light-b").unwrap().name, "Desk lamp");
        let bare: LightResource = serde_json::from_str(r#"{"id":"x"}"#).unwrap();
        assert_eq!(bare.info(None).name, "x");
        assert!(inv.light("nope").is_none());
    }

    #[test]
    fn bridge_lights_list_colour_lights_first_sorted_by_name() {
        let names: Vec<_> = inventory()
            .bridge_lights()
            .into_iter()
            .map(|l| (l.id, l.name))
            .collect();
        assert_eq!(
            names,
            vec![
                ("light-b".to_owned(), "Desk lamp".to_owned()),
                ("light-a".to_owned(), "Floor lamp".to_owned()),
                ("light-c".to_owned(), "Hall bulb".to_owned()),
            ]
        );
    }

    #[test]
    fn snapshots_use_mirek_only_when_valid() {
        let ok = StatusCode::OK;
        let lights: Vec<LightStateReply> = parse_v2(ok, LIGHTS.as_bytes()).unwrap();
        let a = lights[0].snapshot().unwrap();
        assert_eq!(a.xy, Some((0.3, 0.3)));
        assert_eq!(a.mirek, None);
        assert_eq!(a.brightness, Some(50.0));
        assert!(a.on);
        let c = lights[2].snapshot().unwrap();
        assert_eq!(c.mirek, Some(366));
        assert_eq!(c.xy, None);
        let off: LightStateReply = serde_json::from_str(r#"{"on":{"on":false}}"#).unwrap();
        let off = off.snapshot().unwrap();
        assert!(!off.on && off.brightness.is_none());
        let none: LightStateReply = serde_json::from_str("{}").unwrap();
        assert!(none.snapshot().is_none());
    }

    #[test]
    fn bridge_ids_are_uppercased() {
        assert_eq!(
            normalize_bridge_id(" aabbccFFfe112233 "),
            "AABBCCFFFE112233"
        );
    }
}
