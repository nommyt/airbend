//! airbend-core: the parts of airbend that don't touch hardware.
//!
//! - [`model`]: what a backend reports (sensors, fans, controls).
//! - [`config`]: curves and control settings, as saved to disk and sent over IPC.
//! - [`engine`]: turns sensor readings into control outputs, every tick.

pub mod calibrate;
pub mod config;
pub mod engine;
pub mod model;

pub use calibrate::Calibration;
pub use config::{Config, ControlConfig, Curve, CurveKind, MixOp, Point};
pub use engine::{Engine, TickOutput};
pub use model::{ControlInfo, SensorInfo, SensorKind};
