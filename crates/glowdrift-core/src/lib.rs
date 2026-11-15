//! Platform-independent core of Glowdrift: colour math, frame reduction,
//! temporal smoothing and the persisted configuration schema.

pub mod color;
pub mod config;
pub mod frame;
pub mod geometry;
pub mod reduce;
pub mod smooth;
pub mod zones;

pub use color::{LinearRgb, Rgb, linear_to_srgb, srgb_to_linear};
pub use config::{CONFIG_SCHEMA_VERSION, Config, HueConfig, IcueConfig, LightsConfig, ReduceMode};
pub use frame::FrameView;
pub use geometry::{NormRect, PixelRect};
pub use reduce::{ColorAdjust, reduce};
pub use smooth::Smoother;
pub use zones::{LightPosition, bounding_rect, default_zones, reduce_zones};
