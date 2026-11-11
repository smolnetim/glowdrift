//! Persisted configuration schema. Serialised as camelCase JSON; unknown fields are
//! ignored and missing fields take their defaults.

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

/// Settings for the iCUE output.
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

/// Per-output settings.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OutputsConfig {
    pub icue: IcueConfig,
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
    /// Output gain, `0.0..=1.0`.
    pub brightness: f32,
    /// Saturation multiplier, `0.0..=2.0`.
    pub saturation: f32,
    pub outputs: OutputsConfig,
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
            outputs: OutputsConfig::default(),
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
        assert_eq!(v["outputs"]["icue"]["enabled"], true);
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
            outputs: OutputsConfig {
                icue: IcueConfig { enabled: false },
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
        let c: Config = serde_json::from_str(r#"{"region":{"x":0.5},"outputs":{}}"#).unwrap();
        assert_eq!(c.region, NormRect::new(0.5, 0.0, 1.0, 1.0));
        assert!(c.outputs.icue.enabled);
        let c: Config =
            serde_json::from_str(r#"{"outputs":{"icue":{"enabled":false,"x":1}}}"#).unwrap();
        assert!(!c.outputs.icue.enabled);
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
}
