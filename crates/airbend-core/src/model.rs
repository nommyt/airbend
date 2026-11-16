//! What a hardware backend reports. IDs are stable strings chosen by the backend, so they
//! survive reboots (on Linux, `hwmonN` numbers do not; the backend keys on the device instead).

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SensorKind {
    /// Degrees Celsius.
    Temperature,
    /// Revolutions per minute.
    Fan,
}

impl SensorKind {
    pub fn unit(self) -> &'static str {
        match self {
            SensorKind::Temperature => "°C",
            SensorKind::Fan => "RPM",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SensorInfo {
    pub id: String,
    /// Human label, e.g. "k10temp · Tctl".
    pub label: String,
    /// The chip or device it belongs to, e.g. "k10temp", "amdgpu".
    pub device: String,
    pub kind: SensorKind,
    /// Latest value, `None` when the last read failed.
    pub value: Option<f32>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ControlInfo {
    pub id: String,
    pub label: String,
    pub device: String,
    /// Current duty as last read back from hardware, 0–100.
    pub percent: Option<f32>,
    /// The fan (tachometer) sensor this output most likely drives, if the backend knows.
    pub paired_fan: Option<String>,
    /// True while airbend has taken manual control of this output.
    pub managed: bool,
    /// Bounds imposed by the driver, independent of the user's curve settings.
    #[serde(default)]
    pub min_percent: f32,
    #[serde(default = "full_percent")]
    pub max_percent: f32,
    /// Some drivers provide fixed limits and do not permit stop/start calibration.
    #[serde(default = "yes")]
    pub calibration_supported: bool,
}

fn full_percent() -> f32 { 100.0 }
fn yes() -> bool { true }
