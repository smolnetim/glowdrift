//! Persisted configuration schema. Serialised as camelCase JSON; unknown fields are
//! ignored and missing fields take their defaults.

use std::collections::BTreeMap;

use serde::de::IgnoredAny;
use serde::{Deserialize, Deserializer, Serialize};

use crate::geometry::NormRect;

/// Current value of [`Config::schema_version`].
pub const CONFIG_SCHEMA_VERSION: u32 = 1;

/// How a captured region is reduced to one colour.
///
/// Deserialising a value that is not a known variant name (an unknown string or a value of
/// another type) yields the default, [`ReduceMode::Average`], so a config written by a
/// newer version still loads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ReduceMode {
    #[default]
    Average,
    Dominant,
}

impl<'de> Deserialize<'de> for ReduceMode {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(rename_all = "lowercase")]
        enum Known {
            Average,
            Dominant,
        }
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Lenient {
            Known(Known),
            Unknown(IgnoredAny),
        }
        Ok(match Lenient::deserialize(deserializer)? {
            Lenient::Known(Known::Average) | Lenient::Unknown(_) => ReduceMode::Average,
            Lenient::Known(Known::Dominant) => ReduceMode::Dominant,
        })
    }
}

/// Settings for the iCUE light.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct IcueConfig {
    pub enabled: bool,
}

impl Default for IcueConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// Deserialises `T`, yielding `T::default()` for a value of any other type.
fn lenient<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Lenient<T> {
        Known(T),
        Unknown(IgnoredAny),
    }
    Ok(match Lenient::<T>::deserialize(deserializer)? {
        Lenient::Known(value) => value,
        Lenient::Unknown(_) => T::default(),
    })
}

/// Settings for the Philips Hue light. Bridge credentials are not part of the config; they
/// live in the encrypted credential store.
///
/// A field holding a value of the wrong type takes its default.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct HueConfig {
    #[serde(deserialize_with = "lenient")]
    pub enabled: bool,
    /// Identifier of the paired bridge, `None` when none is selected.
    #[serde(deserialize_with = "lenient")]
    pub bridge_id: Option<String>,
    /// Identifier of the entertainment area to drive, `None` when none is selected.
    #[serde(deserialize_with = "lenient")]
    pub area_id: Option<String>,
    /// Per-light zone overrides: Hue light id to a rectangle of the monitor. Lights
    /// without an entry use their default zone. An entry that is not a rectangle is dropped
    /// when loading.
    #[serde(deserialize_with = "zone_map")]
    pub zones: BTreeMap<String, NormRect>,
    /// Ids of single bridge lights driven in addition to the area, in order. Entries that are
    /// not strings are dropped when loading; blank and repeated ids are dropped by validation.
    #[serde(deserialize_with = "id_list")]
    pub extra_lights: Vec<String>,
}

/// Deserialises a list of ids, dropping entries that are not strings; a value that is not a
/// list yields an empty one.
fn id_list<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<String>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Entry {
        Id(String),
        Invalid(IgnoredAny),
    }
    Ok(lenient::<D, Vec<Entry>>(deserializer)?
        .into_iter()
        .filter_map(|entry| match entry {
            Entry::Id(id) => Some(id),
            Entry::Invalid(_) => None,
        })
        .collect())
}

/// Deserialises the zone map, dropping entries that are not rectangles; a value that is not
/// a map yields an empty one.
fn zone_map<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<String, NormRect>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Entry {
        Rect(NormRect),
        Invalid(IgnoredAny),
    }
    Ok(lenient::<D, BTreeMap<String, Entry>>(deserializer)?
        .into_iter()
        .filter_map(|(id, entry)| match entry {
            Entry::Rect(rect) => Some((id, rect)),
            Entry::Invalid(_) => None,
        })
        .collect())
}

impl HueConfig {
    /// Maps empty or whitespace-only identifiers to `None` and trims the rest, drops zones of
    /// blank light ids, clamps the remaining zones into the unit square, and trims, de-duplicates
    /// and drops blank extra light ids.
    fn validated(mut self) -> Self {
        fn clean(id: Option<String>) -> Option<String> {
            id.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty())
        }
        self.bridge_id = clean(self.bridge_id);
        self.area_id = clean(self.area_id);
        self.zones = std::mem::take(&mut self.zones)
            .into_iter()
            .filter(|(id, _)| !id.trim().is_empty())
            .map(|(id, rect)| (id, rect.clamped()))
            .collect();
        let mut seen = std::collections::HashSet::new();
        self.extra_lights = std::mem::take(&mut self.extra_lights)
            .into_iter()
            .map(|id| id.trim().to_owned())
            .filter(|id| !id.is_empty() && seen.insert(id.clone()))
            .collect();
        self
    }
}

/// Per-light settings.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LightsConfig {
    pub icue: IcueConfig,
    pub hue: HueConfig,
}

/// Application configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Config {
    pub schema_version: u32,
    pub onboarding_complete: bool,
    /// `None` selects the primary monitor.
    pub monitor_id: Option<String>,
    /// Captured region as fractions of the monitor.
    pub region: NormRect,
    /// Capture rate, `1..=60`.
    pub capture_fps: u32,
    pub mode: ReduceMode,
    /// Exponential smoothing time constant in milliseconds, `0..=5000`.
    pub smoothing_ms: u32,
    /// Brightness gain, `0.0..=1.0`.
    pub brightness: f32,
    /// Saturation multiplier, `0.0..=2.0`.
    pub saturation: f32,
    pub lights: LightsConfig,
    pub start_minimized: bool,
    pub auto_update: bool,
}

impl Config {
    pub const CAPTURE_FPS_RANGE: std::ops::RangeInclusive<u32> = 1..=60;
    pub const SMOOTHING_MS_MAX: u32 = 5000;
    pub const SATURATION_MAX: f32 = 2.0;

    /// Upgrades a config written by an older schema to the current one. Schema versions 0
    /// and 1 share a layout, so this currently only stamps the version; a version newer
    /// than this build keeps its known fields and is stamped as current. Upgrade steps
    /// for later schemas belong here, keyed on `schema_version`, before the stamp.
    #[must_use]
    pub fn migrate(mut self) -> Config {
        self.schema_version = CONFIG_SCHEMA_VERSION;
        self
    }

    /// Migrates to the current schema, then returns a copy with every value clamped into
    /// its valid range and the region clamped inside the monitor. NaN values fall back to
    /// their defaults.
    #[must_use]
    pub fn validated(self) -> Config {
        fn unit(v: f32, fallback: f32, max: f32) -> f32 {
            if v.is_nan() {
                fallback
            } else {
                v.clamp(0.0, max)
            }
        }
        let mut this = self.migrate();
        this.region = this.region.clamped();
        this.capture_fps = this.capture_fps.clamp(
            *Self::CAPTURE_FPS_RANGE.start(),
            *Self::CAPTURE_FPS_RANGE.end(),
        );
        this.smoothing_ms = this.smoothing_ms.min(Self::SMOOTHING_MS_MAX);
        this.brightness = unit(this.brightness, 1.0, 1.0);
        this.saturation = unit(this.saturation, 1.0, Self::SATURATION_MAX);
        if this.monitor_id.as_deref().is_some_and(str::is_empty) {
            this.monitor_id = None;
        }
        this.lights.hue = this.lights.hue.validated();
        this
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            schema_version: CONFIG_SCHEMA_VERSION,
            onboarding_complete: false,
            monitor_id: None,
            region: NormRect::full(),
            capture_fps: 20,
            mode: ReduceMode::Average,
            smoothing_ms: 500,
            brightness: 1.0,
            saturation: 1.0,
            lights: LightsConfig::default(),
            start_minimized: true,
            auto_update: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_object_yields_defaults() {
        let c: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(c, Config::default());
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let c: Config = serde_json::from_str(r#"{"captureFps":30,"futureThing":{"a":1}}"#).unwrap();
        assert_eq!(c.capture_fps, 30);
    }

    #[test]
    fn serialises_camel_case() {
        let v = serde_json::to_value(Config::default()).unwrap();
        assert_eq!(v["schemaVersion"], 1);
        assert_eq!(v["mode"], "average");
        assert_eq!(v["lights"]["icue"]["enabled"], true);
        assert_eq!(v["monitorId"], serde_json::Value::Null);
        assert_eq!(v["region"]["width"], 1.0);
    }

    #[test]
    fn validated_clamps() {
        let c = Config {
            capture_fps: 500,
            smoothing_ms: 99_999,
            brightness: 7.0,
            saturation: f32::NAN,
            region: NormRect::new(2.0, -3.0, 9.0, 0.0),
            monitor_id: Some(String::new()),
            ..Config::default()
        }
        .validated();
        assert_eq!(c.capture_fps, 60);
        assert_eq!(c.smoothing_ms, 5000);
        assert_eq!(c.brightness, 1.0);
        assert_eq!(c.saturation, 1.0);
        assert_eq!(c.monitor_id, None);
        assert_eq!(c.region, c.region.clamped());
    }

    #[test]
    fn default_is_already_valid() {
        assert_eq!(Config::default().validated(), Config::default());
    }

    #[test]
    fn unknown_enum_values_fall_back() {
        for json in [
            r#"{"mode":"rainbow"}"#,
            r#"{"mode":42}"#,
            r#"{"mode":null}"#,
            r#"{"mode":{"a":[1,2]}}"#,
        ] {
            let c: Config = serde_json::from_str(json).unwrap();
            assert_eq!(c.mode, ReduceMode::Average, "{json}");
        }
        let c: Config = serde_json::from_str(r#"{"mode":"dominant"}"#).unwrap();
        assert_eq!(c.mode, ReduceMode::Dominant);
    }

    #[test]
    fn full_round_trip() {
        let c = Config {
            onboarding_complete: true,
            monitor_id: Some(r"\\.\DISPLAY2".into()),
            region: NormRect::new(0.1, 0.2, 0.5, 0.25),
            capture_fps: 33,
            mode: ReduceMode::Dominant,
            smoothing_ms: 1234,
            brightness: 0.75,
            saturation: 1.5,
            lights: LightsConfig {
                icue: IcueConfig { enabled: false },
                hue: HueConfig {
                    enabled: true,
                    bridge_id: Some("bridge-1".into()),
                    area_id: Some("area-1".into()),
                    zones: BTreeMap::from([(
                        "light-1".to_owned(),
                        NormRect::new(0.1, 0.2, 0.3, 0.4),
                    )]),
                    extra_lights: vec!["light-2".to_owned()],
                },
            },
            start_minimized: false,
            auto_update: false,
            ..Config::default()
        };
        let json = serde_json::to_string(&c).unwrap();
        let back: Config = serde_json::from_str(&json).unwrap();
        assert_eq!(back, c);
        assert_eq!(back.validated(), c);
    }

    #[test]
    fn partial_nested_objects_default_missing_fields() {
        let c: Config = serde_json::from_str(r#"{"region":{"x":0.5},"lights":{}}"#).unwrap();
        assert_eq!(c.region, NormRect::new(0.5, 0.0, 1.0, 1.0));
        assert!(c.lights.icue.enabled);
        let c: Config =
            serde_json::from_str(r#"{"lights":{"icue":{"enabled":false,"x":1}}}"#).unwrap();
        assert!(!c.lights.icue.enabled);
    }

    #[test]
    fn validated_clamps_low_and_infinite_values() {
        let c = Config {
            capture_fps: 0,
            brightness: -2.0,
            saturation: f32::INFINITY,
            region: NormRect::new(f32::NAN, f32::INFINITY, f32::NEG_INFINITY, f32::NAN),
            ..Config::default()
        }
        .validated();
        assert_eq!(c.capture_fps, 1);
        assert_eq!(c.brightness, 0.0);
        assert_eq!(c.saturation, Config::SATURATION_MAX);
        assert_eq!(c.region, c.region.clamped());
        assert!(c.region.width >= NormRect::MIN_SIZE && c.region.height >= NormRect::MIN_SIZE);
    }

    #[test]
    fn validated_is_idempotent() {
        let once = Config {
            capture_fps: 999,
            smoothing_ms: u32::MAX,
            brightness: 3.0,
            saturation: -1.0,
            region: NormRect::new(-5.0, 7.0, 0.001, 40.0),
            ..Config::default()
        }
        .validated();
        assert_eq!(once.clone().validated(), once);
    }

    #[test]
    fn migrate_stamps_current_version() {
        for v in [0, 1, 2, 99, u32::MAX] {
            let c = Config {
                schema_version: v,
                capture_fps: 45,
                ..Config::default()
            }
            .migrate();
            assert_eq!(c.schema_version, CONFIG_SCHEMA_VERSION);
            assert_eq!(c.capture_fps, 45);
        }
        let c: Config = serde_json::from_str(r#"{"schemaVersion":0}"#).unwrap();
        assert_eq!(c.validated().schema_version, CONFIG_SCHEMA_VERSION);
    }

    #[test]
    fn rect_survives_json_round_trip_after_validation() {
        let c = Config {
            region: NormRect::new(0.3, 0.3, 0.9, 0.9),
            ..Config::default()
        }
        .validated();
        let back: Config = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn config_without_hue_loads_with_hue_defaults() {
        let c: Config = serde_json::from_str(
            r#"{"schemaVersion":1,"captureFps":30,"lights":{"icue":{"enabled":false}}}"#,
        )
        .unwrap();
        assert_eq!(c.lights.hue, HueConfig::default());
        assert!(!c.lights.hue.enabled);
        assert_eq!(c.lights.hue.bridge_id, None);
        assert_eq!(c.lights.hue.area_id, None);
        assert!(!c.lights.icue.enabled);
    }

    #[test]
    fn hue_serialises_camel_case() {
        let v = serde_json::to_value(Config::default()).unwrap();
        assert_eq!(v["lights"]["hue"]["enabled"], false);
        assert_eq!(v["lights"]["hue"]["bridgeId"], serde_json::Value::Null);
        assert_eq!(v["lights"]["hue"]["areaId"], serde_json::Value::Null);
    }

    #[test]
    fn hue_reads_camel_case_and_ignores_unknown_fields() {
        let c: Config = serde_json::from_str(
            r#"{"lights":{"hue":{"enabled":true,"bridgeId":"b","areaId":"a","future":[1]}}}"#,
        )
        .unwrap();
        assert!(c.lights.hue.enabled);
        assert_eq!(c.lights.hue.bridge_id.as_deref(), Some("b"));
        assert_eq!(c.lights.hue.area_id.as_deref(), Some("a"));
    }

    #[test]
    fn hue_fields_of_the_wrong_type_take_defaults() {
        let c: Config = serde_json::from_str(
            r#"{"capture_fps":1,"lights":{"hue":{"enabled":"yes","bridgeId":7,"areaId":{"a":1}}}}"#,
        )
        .unwrap();
        assert_eq!(c.lights.hue, HueConfig::default());
        let c: Config =
            serde_json::from_str(r#"{"lights":{"hue":{"enabled":true,"bridgeId":null}}}"#).unwrap();
        assert!(c.lights.hue.enabled);
        assert_eq!(c.lights.hue.bridge_id, None);
    }

    #[test]
    fn validated_normalises_hue_identifiers() {
        let mut c = Config::default();
        c.lights.hue.bridge_id = Some("   ".into());
        c.lights.hue.area_id = Some("  area-1 ".into());
        let c = c.validated();
        assert_eq!(c.lights.hue.bridge_id, None);
        assert_eq!(c.lights.hue.area_id.as_deref(), Some("area-1"));
        let mut c = Config::default();
        c.lights.hue.bridge_id = Some(String::new());
        assert_eq!(c.validated().lights.hue.bridge_id, None);
    }

    #[test]
    fn zones_default_to_empty_and_are_not_required() {
        assert!(Config::default().lights.hue.zones.is_empty());
        let c: Config = serde_json::from_str(r#"{"lights":{"hue":{"enabled":true}}}"#).unwrap();
        assert!(c.lights.hue.zones.is_empty());
        let v = serde_json::to_value(Config::default()).unwrap();
        assert_eq!(v["lights"]["hue"]["zones"], serde_json::json!({}));
    }

    #[test]
    fn zones_round_trip_camel_case() {
        let c: Config = serde_json::from_str(
            r#"{"lights":{"hue":{"zones":{"a":{"x":0.1,"y":0.2,"width":0.3,"height":0.4}}}}}"#,
        )
        .unwrap();
        assert_eq!(
            c.lights.hue.zones,
            BTreeMap::from([("a".to_owned(), NormRect::new(0.1, 0.2, 0.3, 0.4))])
        );
        let back: Config = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn invalid_zone_entries_are_dropped() {
        let c: Config = serde_json::from_str(
            r#"{"lights":{"hue":{"enabled":true,"zones":{
                "ok":{"x":0.5,"y":0.5,"width":0.25,"height":0.25},
                "num":3,"null":null,"text":"x","list":[1,"a"],
                "badfield":{"x":"left","width":0.5}
            }}}}"#,
        )
        .unwrap();
        assert!(c.lights.hue.enabled);
        assert_eq!(c.lights.hue.zones.keys().collect::<Vec<_>>(), vec!["ok"]);
    }

    #[test]
    fn zones_of_the_wrong_type_are_empty() {
        for json in ["[1]", r#""zones""#, "7", "null", "true"] {
            let c: Config =
                serde_json::from_str(&format!(r#"{{"lights":{{"hue":{{"zones":{json}}}}}}}"#))
                    .unwrap();
            assert!(c.lights.hue.zones.is_empty(), "{json}");
        }
    }

    #[test]
    fn validated_clamps_zones_and_drops_blank_ids() {
        let mut c = Config::default();
        c.lights.hue.zones = BTreeMap::from([
            ("a".to_owned(), NormRect::new(0.9, -1.0, 0.5, f32::NAN)),
            ("b".to_owned(), NormRect::new(0.0, 0.0, 0.0, 0.0)),
            (String::new(), NormRect::full()),
            ("  ".to_owned(), NormRect::full()),
        ]);
        let c = c.validated();
        assert_eq!(c.lights.hue.zones.len(), 2);
        for rect in c.lights.hue.zones.values() {
            assert_eq!(*rect, rect.clamped());
        }
        assert_eq!(c.lights.hue.zones["a"].height, 1.0);
        assert_eq!(c.lights.hue.zones["b"].width, NormRect::MIN_SIZE);
        assert_eq!(c.clone().validated(), c);
    }

    #[test]
    fn extra_lights_default_to_empty_and_serialise_camel_case() {
        assert_eq!(
            Config::default().lights.hue.extra_lights,
            Vec::<String>::new()
        );
        let v = serde_json::to_value(Config::default()).unwrap();
        assert_eq!(v["lights"]["hue"]["extraLights"], serde_json::json!([]));
        let c: Config =
            serde_json::from_str(r#"{"lights":{"hue":{"extraLights":["a","b"]}}}"#).unwrap();
        assert_eq!(c.lights.hue.extra_lights, vec!["a", "b"]);
    }

    #[test]
    fn invalid_extra_light_entries_are_dropped() {
        let c: Config = serde_json::from_str(
            r#"{"lights":{"hue":{"extraLights":["a",3,null,{"x":1},["b"],"c"]}}}"#,
        )
        .unwrap();
        assert_eq!(c.lights.hue.extra_lights, vec!["a", "c"]);
        for json in [r#""a""#, "7", "null", r#"{"a":1}"#] {
            let c: Config = serde_json::from_str(&format!(
                r#"{{"lights":{{"hue":{{"extraLights":{json}}}}}}}"#
            ))
            .unwrap();
            assert!(c.lights.hue.extra_lights.is_empty(), "{json}");
        }
    }

    #[test]
    fn validated_cleans_extra_lights() {
        let mut c = Config::default();
        c.lights.hue.extra_lights = ["a", " b ", "", "  ", "a", "b", "c"]
            .map(str::to_owned)
            .to_vec();
        let c = c.validated();
        assert_eq!(c.lights.hue.extra_lights, vec!["a", "b", "c"]);
        assert_eq!(c.clone().validated(), c);
    }
}
