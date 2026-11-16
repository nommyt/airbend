//! The saved configuration: curves (sensor → percent functions) and controls (fan outputs that
//! follow a curve, with their own tuning). Serialized as TOML on disk and JSON over IPC.

use std::collections::{BTreeMap, HashMap, HashSet};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::calibrate::Calibration;

pub const CONFIG_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub version: u32,
    /// How often the daemon reads sensors and updates outputs, in milliseconds.
    pub tick_ms: u64,
    /// User renames for sensors and controls, keyed by hardware id.
    pub labels: BTreeMap<String, String>,
    pub curves: Vec<Curve>,
    pub controls: Vec<ControlConfig>,
    /// Calibration results by control id. Written by the daemon after a calibration run.
    pub calibrations: BTreeMap<String, Calibration>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            version: CONFIG_VERSION,
            tick_ms: 1000,
            labels: BTreeMap::new(),
            curves: Vec::new(),
            controls: Vec::new(),
            calibrations: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Point {
    /// °C
    pub temp: f32,
    /// 0–100
    pub percent: f32,
}

impl Point {
    pub const fn new(temp: f32, percent: f32) -> Point {
        Point { temp, percent }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MixOp {
    Max,
    Min,
    Average,
    Sum,
}

impl MixOp {
    pub const ALL: [MixOp; 4] = [MixOp::Max, MixOp::Min, MixOp::Average, MixOp::Sum];
    pub fn label(self) -> &'static str {
        match self {
            MixOp::Max => "Max",
            MixOp::Min => "Min",
            MixOp::Average => "Average",
            MixOp::Sum => "Sum",
        }
    }
}

/// How a sensor-driven curve reacts to temperature changes.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Tuning {
    /// °C the temperature must move before the curve follows it.
    pub hysteresis: f32,
    /// When true, rising temperatures apply at once and only falls wait out the hysteresis.
    pub hysteresis_down_only: bool,
    /// Seconds of readings averaged together before the curve sees them.
    pub response_time: f32,
}

impl Default for Tuning {
    fn default() -> Self {
        Tuning { hysteresis: 2.0, hysteresis_down_only: true, response_time: 2.0 }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CurveKind {
    /// A constant.
    Flat { percent: f32 },
    /// A straight line between two temperatures.
    Linear {
        sensor: String,
        min_temp: f32,
        max_temp: f32,
        min_percent: f32,
        max_percent: f32,
        #[serde(default)]
        tuning: Tuning,
    },
    /// Piecewise linear through points sorted by temperature.
    Graph {
        sensor: String,
        points: Vec<Point>,
        #[serde(default)]
        tuning: Tuning,
    },
    /// Two levels: switch to `load_percent` at or above `load_temp`, back to `idle_percent` at or
    /// below `idle_temp`.
    Trigger {
        sensor: String,
        idle_temp: f32,
        load_temp: f32,
        idle_percent: f32,
        load_percent: f32,
        #[serde(default)]
        tuning: Tuning,
    },
    /// Combines other curves.
    Mix { curves: Vec<String>, op: MixOp },
}

impl CurveKind {
    pub fn type_label(&self) -> &'static str {
        match self {
            CurveKind::Flat { .. } => "Flat",
            CurveKind::Linear { .. } => "Linear",
            CurveKind::Graph { .. } => "Graph",
            CurveKind::Trigger { .. } => "Trigger",
            CurveKind::Mix { .. } => "Mix",
        }
    }

    pub fn sensor(&self) -> Option<&str> {
        match self {
            CurveKind::Linear { sensor, .. } | CurveKind::Graph { sensor, .. } | CurveKind::Trigger { sensor, .. } => {
                Some(sensor)
            }
            CurveKind::Flat { .. } | CurveKind::Mix { .. } => None,
        }
    }

    pub fn sensor_mut(&mut self) -> Option<&mut String> {
        match self {
            CurveKind::Linear { sensor, .. } | CurveKind::Graph { sensor, .. } | CurveKind::Trigger { sensor, .. } => {
                Some(sensor)
            }
            CurveKind::Flat { .. } | CurveKind::Mix { .. } => None,
        }
    }

    pub fn tuning(&self) -> Option<&Tuning> {
        match self {
            CurveKind::Linear { tuning, .. } | CurveKind::Graph { tuning, .. } | CurveKind::Trigger { tuning, .. } => {
                Some(tuning)
            }
            CurveKind::Flat { .. } | CurveKind::Mix { .. } => None,
        }
    }

    pub fn tuning_mut(&mut self) -> Option<&mut Tuning> {
        match self {
            CurveKind::Linear { tuning, .. } | CurveKind::Graph { tuning, .. } | CurveKind::Trigger { tuning, .. } => {
                Some(tuning)
            }
            CurveKind::Flat { .. } | CurveKind::Mix { .. } => None,
        }
    }

    /// A sensible default graph: quiet below 40 °C, full speed at 85 °C.
    pub fn default_graph(sensor: impl Into<String>) -> CurveKind {
        CurveKind::Graph {
            sensor: sensor.into(),
            points: vec![
                Point::new(30.0, 20.0),
                Point::new(50.0, 35.0),
                Point::new(65.0, 55.0),
                Point::new(75.0, 75.0),
                Point::new(85.0, 100.0),
            ],
            tuning: Tuning::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Curve {
    pub id: String,
    pub name: String,
    #[serde(flatten)]
    pub kind: CurveKind,
}

/// One fan output and how it follows its curve.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ControlConfig {
    /// Hardware control id.
    pub control: String,
    /// When false the output is left to the firmware (BIOS/auto).
    pub enabled: bool,
    /// The curve to follow. Ignored while `manual` is set.
    pub curve: Option<String>,
    /// A fixed percent overriding the curve.
    pub manual: Option<f32>,
    /// Lowest percent while the fan spins.
    pub min_percent: f32,
    pub max_percent: f32,
    /// When the fan starts from 0 %, it is kicked to at least this percent, then eases down.
    pub start_percent: f32,
    /// When true, targets below `min_percent` stop the fan (0 %). Otherwise they hold at the minimum.
    pub zero_rpm: bool,
    /// Largest change per second, rising and falling, in percent. 0 = instant.
    pub step_up: f32,
    pub step_down: f32,
    /// Added to the curve's output.
    pub offset: f32,
}

impl Default for ControlConfig {
    fn default() -> Self {
        ControlConfig {
            control: String::new(),
            enabled: false,
            curve: None,
            manual: None,
            min_percent: 0.0,
            max_percent: 100.0,
            start_percent: 0.0,
            zero_rpm: false,
            step_up: 10.0,
            step_down: 5.0,
            offset: 0.0,
        }
    }
}

impl ControlConfig {
    pub fn new(control: impl Into<String>) -> ControlConfig {
        ControlConfig { control: control.into(), ..ControlConfig::default() }
    }
}

impl Config {
    pub fn from_toml(s: &str) -> Result<Config> {
        let c: Config = toml::from_str(s).context("parsing config")?;
        c.validate()?;
        Ok(c)
    }

    pub fn to_toml(&self) -> Result<String> {
        Ok(toml::to_string_pretty(self)?)
    }

    pub fn curve(&self, id: &str) -> Option<&Curve> {
        self.curves.iter().find(|c| c.id == id)
    }

    pub fn curve_mut(&mut self, id: &str) -> Option<&mut Curve> {
        self.curves.iter_mut().find(|c| c.id == id)
    }

    pub fn control(&self, id: &str) -> Option<&ControlConfig> {
        self.controls.iter().find(|c| c.control == id)
    }

    /// The control's settings, created (disabled) when missing.
    pub fn control_mut(&mut self, id: &str) -> &mut ControlConfig {
        if let Some(i) = self.controls.iter().position(|c| c.control == id) {
            &mut self.controls[i]
        } else {
            self.controls.push(ControlConfig::new(id));
            self.controls.last_mut().unwrap()
        }
    }

    /// A new curve id not yet in use: `curve-1`, `curve-2`, …
    pub fn fresh_curve_id(&self) -> String {
        (1..).map(|n| format!("curve-{n}")).find(|id| self.curve(id).is_none()).unwrap()
    }

    /// Removes a curve and every reference to it.
    pub fn remove_curve(&mut self, id: &str) {
        self.curves.retain(|c| c.id != id);
        for c in &mut self.curves {
            if let CurveKind::Mix { curves, .. } = &mut c.kind {
                curves.retain(|x| x != id);
            }
        }
        for c in &mut self.controls {
            if c.curve.as_deref() == Some(id) {
                c.curve = None;
            }
        }
    }

    /// Rejects configs the engine can't run safely: duplicate ids, dangling references,
    /// mix cycles, unsorted graphs and out-of-range numbers.
    // `!(a < b)` on purpose: it also rejects NaN, which `a >= b` would let through.
    #[allow(clippy::neg_cmp_op_on_partial_ord)]
    pub fn validate(&self) -> Result<()> {
        let mut ids = HashSet::new();
        for c in &self.curves {
            if c.id.is_empty() {
                bail!("a curve has an empty id");
            }
            if !ids.insert(c.id.as_str()) {
                bail!("duplicate curve id {:?}", c.id);
            }
        }
        for c in &self.curves {
            let pct = |v: f32, what: &str| -> Result<()> {
                if !(0.0..=100.0).contains(&v) || v.is_nan() {
                    bail!("curve {:?}: {what} {v} is outside 0–100 %", c.id);
                }
                Ok(())
            };
            match &c.kind {
                CurveKind::Flat { percent } => pct(*percent, "percent")?,
                CurveKind::Linear { min_temp, max_temp, min_percent, max_percent, tuning, .. } => {
                    if !(min_temp < max_temp) {
                        bail!("curve {:?}: min_temp must be below max_temp", c.id);
                    }
                    pct(*min_percent, "min_percent")?;
                    pct(*max_percent, "max_percent")?;
                    check_tuning(&c.id, tuning)?;
                }
                CurveKind::Graph { points, tuning, .. } => {
                    if points.is_empty() {
                        bail!("curve {:?}: a graph needs at least one point", c.id);
                    }
                    for p in points {
                        pct(p.percent, "point")?;
                        if p.temp.is_nan() {
                            bail!("curve {:?}: a point's temperature is NaN", c.id);
                        }
                    }
                    if points.windows(2).any(|w| w[0].temp > w[1].temp) {
                        bail!("curve {:?}: graph points must be sorted by temperature", c.id);
                    }
                    check_tuning(&c.id, tuning)?;
                }
                CurveKind::Trigger { idle_temp, load_temp, idle_percent, load_percent, tuning, .. } => {
                    if !(idle_temp < load_temp) {
                        bail!("curve {:?}: idle_temp must be below load_temp", c.id);
                    }
                    pct(*idle_percent, "idle_percent")?;
                    pct(*load_percent, "load_percent")?;
                    check_tuning(&c.id, tuning)?;
                }
                CurveKind::Mix { curves, .. } => {
                    for r in curves {
                        if !ids.contains(r.as_str()) {
                            bail!("curve {:?}: mixes unknown curve {r:?}", c.id);
                        }
                    }
                }
            }
        }
        self.check_cycles()?;

        let mut controls = HashSet::new();
        for c in &self.controls {
            if !controls.insert(c.control.as_str()) {
                bail!("control {:?} is configured twice", c.control);
            }
            if let Some(curve) = &c.curve
                && !ids.contains(curve.as_str())
            {
                bail!("control {:?} follows unknown curve {curve:?}", c.control);
            }
            for (v, what) in [
                (c.min_percent, "min_percent"),
                (c.max_percent, "max_percent"),
                (c.start_percent, "start_percent"),
            ] {
                if !(0.0..=100.0).contains(&v) {
                    bail!("control {:?}: {what} {v} is outside 0–100 %", c.control);
                }
            }
            if let Some(m) = c.manual
                && !(0.0..=100.0).contains(&m)
            {
                bail!("control {:?}: manual {m} is outside 0–100 %", c.control);
            }
            if c.min_percent > c.max_percent {
                bail!("control {:?}: min_percent is above max_percent", c.control);
            }
            if c.step_up < 0.0 || c.step_down < 0.0 {
                bail!("control {:?}: steps can't be negative", c.control);
            }
        }
        if self.tick_ms < 100 || self.tick_ms > 10_000 {
            bail!("tick_ms must be between 100 and 10000");
        }
        Ok(())
    }

    fn check_cycles(&self) -> Result<()> {
        let edges: HashMap<&str, Vec<&str>> = self
            .curves
            .iter()
            .map(|c| {
                let to = match &c.kind {
                    CurveKind::Mix { curves, .. } => curves.iter().map(String::as_str).collect(),
                    _ => Vec::new(),
                };
                (c.id.as_str(), to)
            })
            .collect();
        // 0 unvisited, 1 on the stack, 2 done
        let mut mark: HashMap<&str, u8> = HashMap::new();
        fn visit<'a>(n: &'a str, e: &HashMap<&'a str, Vec<&'a str>>, mark: &mut HashMap<&'a str, u8>) -> Result<()> {
            match mark.get(n) {
                Some(1) => bail!("mix curves form a cycle through {n:?}"),
                Some(2) => return Ok(()),
                _ => {}
            }
            mark.insert(n, 1);
            for m in e.get(n).into_iter().flatten() {
                visit(m, e, mark)?;
            }
            mark.insert(n, 2);
            Ok(())
        }
        for id in edges.keys() {
            visit(id, &edges, &mut mark)?;
        }
        Ok(())
    }
}

fn check_tuning(id: &str, t: &Tuning) -> Result<()> {
    if t.hysteresis < 0.0 || t.response_time < 0.0 || t.response_time > 60.0 {
        bail!("curve {id:?}: hysteresis must be ≥ 0 and response_time within 0–60 s");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Config {
        Config {
            curves: vec![
                Curve { id: "cpu".into(), name: "CPU".into(), kind: CurveKind::default_graph("t1") },
                Curve { id: "quiet".into(), name: "Quiet".into(), kind: CurveKind::Flat { percent: 30.0 } },
                Curve {
                    id: "mix".into(),
                    name: "Mix".into(),
                    kind: CurveKind::Mix { curves: vec!["cpu".into(), "quiet".into()], op: MixOp::Max },
                },
            ],
            controls: vec![ControlConfig { enabled: true, curve: Some("mix".into()), ..ControlConfig::new("pwm1") }],
            ..Config::default()
        }
    }

    #[test]
    fn the_shipped_example_config_loads() {
        Config::from_toml(include_str!("../../../dist/config.example.toml")).unwrap();
    }

    #[test]
    fn toml_round_trips() {
        let c = sample();
        let s = c.to_toml().unwrap();
        assert_eq!(Config::from_toml(&s).unwrap(), c, "{s}");
    }

    #[test]
    fn rejects_cycles_and_dangling_refs() {
        let mut c = sample();
        c.curves.push(Curve {
            id: "a".into(),
            name: "A".into(),
            kind: CurveKind::Mix { curves: vec!["b".into()], op: MixOp::Max },
        });
        c.curves.push(Curve {
            id: "b".into(),
            name: "B".into(),
            kind: CurveKind::Mix { curves: vec!["a".into()], op: MixOp::Max },
        });
        assert!(c.validate().unwrap_err().to_string().contains("cycle"));

        let mut c = sample();
        c.controls[0].curve = Some("nope".into());
        assert!(c.validate().is_err());
    }

    #[test]
    fn removing_a_curve_drops_its_references() {
        let mut c = sample();
        c.remove_curve("cpu");
        c.validate().unwrap();
        let CurveKind::Mix { curves, .. } = &c.curve("mix").unwrap().kind else { panic!() };
        assert_eq!(curves, &["quiet".to_string()]);
    }

    #[test]
    fn rejects_unsorted_graphs() {
        let mut c = sample();
        if let CurveKind::Graph { points, .. } = &mut c.curve_mut("cpu").unwrap().kind {
            points.swap(0, 1);
        }
        assert!(c.validate().is_err());
    }
}
