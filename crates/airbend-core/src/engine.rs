//! The control loop's arithmetic. Each tick takes the latest temperatures and the time since
//! the last tick and returns, per control, either a duty to apply or "release to firmware".
//!
//! Failsafe rule: a curve whose sensor has no reading outputs 100 %. A fan running loud is a
//! nuisance; a fan stopped under load is damage.

use std::collections::{BTreeMap, HashMap, VecDeque};

use serde::{Deserialize, Serialize};

use crate::config::{Config, ControlConfig, CurveKind, MixOp, Point, Tuning};

pub const FAILSAFE_PERCENT: f32 = 100.0;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CurveValue {
    /// The temperature the curve used, after averaging and hysteresis.
    pub temp: Option<f32>,
    pub percent: f32,
    /// Set when the curve fell back to the failsafe.
    pub error: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum ControlTarget {
    /// Take manual control and set this duty (0–100).
    Set(f32),
    /// Hand the output back to the firmware.
    Release,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TickOutput {
    pub curves: BTreeMap<String, CurveValue>,
    pub controls: BTreeMap<String, ControlTarget>,
}

#[derive(Default)]
struct CurveState {
    samples: VecDeque<(f64, f32)>,
    effective: Option<f32>,
    triggered: bool,
}

pub struct Engine {
    config: Config,
    now: f64,
    curves: HashMap<String, CurveState>,
    /// Last duty sent per control, for stepping.
    outputs: HashMap<String, f32>,
}

impl Engine {
    pub fn new(config: Config) -> Engine {
        Engine { config, now: 0.0, curves: HashMap::new(), outputs: HashMap::new() }
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Swaps the config. Curve state (averages, hysteresis) is kept for curves whose sensor
    /// didn't change, so editing a curve doesn't make fans jump.
    pub fn set_config(&mut self, config: Config) {
        let old = std::mem::replace(&mut self.config, config);
        self.curves.retain(|id, _| {
            let before = old.curve(id).and_then(|c| c.kind.sensor());
            let after = self.config.curve(id).and_then(|c| c.kind.sensor());
            before.is_some() && before == after
        });
        self.outputs.retain(|id, _| self.config.control(id).is_some_and(|c| c.enabled));
    }

    /// One step: `temps` maps sensor id → °C, `dt` is seconds since the last tick.
    pub fn tick(&mut self, temps: &HashMap<String, f32>, dt: f32) -> TickOutput {
        self.now += dt as f64;
        let mut out = TickOutput::default();

        // Sensor-driven curves first (they carry state), then mixes resolve from those values.
        for curve in &self.config.curves {
            let Some(sensor) = curve.kind.sensor() else { continue };
            let state = self.curves.entry(curve.id.clone()).or_default();
            let value = match temps.get(sensor).copied().filter(|t| t.is_finite()) {
                None => CurveValue {
                    temp: None,
                    percent: FAILSAFE_PERCENT,
                    error: Some(format!("no reading from {sensor}")),
                },
                Some(raw) => {
                    let tuning = curve.kind.tuning().copied().unwrap_or_default();
                    let t = smooth(state, self.now, raw, &tuning);
                    CurveValue { temp: Some(t), percent: evaluate(&curve.kind, t, state), error: None }
                }
            };
            out.curves.insert(curve.id.clone(), value);
        }
        for curve in &self.config.curves {
            if !out.curves.contains_key(&curve.id) {
                let v = resolve(&self.config, &curve.id, &mut out.curves, 0);
                out.curves.insert(curve.id.clone(), v);
            }
        }

        for c in &self.config.controls {
            let target = self.control_target(c, &out.curves, dt);
            if let ControlTarget::Set(p) = target {
                self.outputs.insert(c.control.clone(), p);
            } else {
                self.outputs.remove(&c.control);
            }
            out.controls.insert(c.control.clone(), target);
        }
        out
    }

    fn control_target(&self, c: &ControlConfig, curves: &BTreeMap<String, CurveValue>, dt: f32) -> ControlTarget {
        if !c.enabled {
            return ControlTarget::Release;
        }
        let wanted = match (c.manual, &c.curve) {
            (Some(m), _) => m,
            (None, Some(id)) => curves.get(id).map_or(FAILSAFE_PERCENT, |v| v.percent) + c.offset,
            (None, None) => return ControlTarget::Release,
        };
        let failsafe = c.manual.is_none()
            && c.curve.as_ref().and_then(|id| curves.get(id)).is_some_and(|v| v.error.is_some());
        if failsafe {
            // Skip stepping and limits: go to full speed now.
            return ControlTarget::Set(FAILSAFE_PERCENT);
        }

        let mut target = wanted.clamp(0.0, 100.0);
        if target < c.min_percent {
            target = if c.zero_rpm { 0.0 } else { c.min_percent };
        }
        target = target.min(c.max_percent);

        let Some(&prev) = self.outputs.get(&c.control) else {
            // First tick under control: apply directly (with the start kick if spinning up).
            return ControlTarget::Set(if target > 0.0 { target.max(c.start_percent) } else { 0.0 });
        };
        if prev <= 0.0 && target > 0.0 && c.start_percent > 0.0 {
            return ControlTarget::Set(target.max(c.start_percent));
        }
        let next = step(prev, target, c.step_up * dt, c.step_down * dt);
        ControlTarget::Set(next)
    }
}

/// Moves `prev` toward `target` by at most `up` rising or `down` falling (0 = no limit).
fn step(prev: f32, target: f32, up: f32, down: f32) -> f32 {
    if target > prev {
        if up > 0.0 { (prev + up).min(target) } else { target }
    } else if down > 0.0 {
        (prev - down).max(target)
    } else {
        target
    }
}

/// Averages the last `response_time` seconds of readings, then applies hysteresis.
fn smooth(state: &mut CurveState, now: f64, raw: f32, t: &Tuning) -> f32 {
    state.samples.push_back((now, raw));
    let window = t.response_time.max(0.0) as f64;
    while state.samples.len() > 1 && state.samples.front().is_some_and(|&(at, _)| now - at > window) {
        state.samples.pop_front();
    }
    let avg = state.samples.iter().map(|&(_, v)| v).sum::<f32>() / state.samples.len() as f32;

    let eff = match state.effective {
        None => avg,
        Some(e) if avg > e && t.hysteresis_down_only => avg,
        Some(e) if (avg - e).abs() >= t.hysteresis => avg,
        Some(e) => e,
    };
    state.effective = Some(eff);
    eff
}

fn evaluate(kind: &CurveKind, t: f32, state: &mut CurveState) -> f32 {
    match kind {
        CurveKind::Flat { percent } => *percent,
        CurveKind::Linear { min_temp, max_temp, min_percent, max_percent, .. } => {
            let f = ((t - min_temp) / (max_temp - min_temp)).clamp(0.0, 1.0);
            min_percent + (max_percent - min_percent) * f
        }
        CurveKind::Graph { points, .. } => interpolate(points, t),
        CurveKind::Trigger { idle_temp, load_temp, idle_percent, load_percent, .. } => {
            if t >= *load_temp {
                state.triggered = true;
            } else if t <= *idle_temp {
                state.triggered = false;
            }
            if state.triggered { *load_percent } else { *idle_percent }
        }
        CurveKind::Mix { .. } => unreachable!("mixes have no sensor"),
    }
}

/// Piecewise-linear lookup, flat beyond the ends. `points` must be sorted by temperature.
pub fn interpolate(points: &[Point], t: f32) -> f32 {
    let (Some(first), Some(last)) = (points.first(), points.last()) else { return FAILSAFE_PERCENT };
    if t <= first.temp {
        return first.percent;
    }
    if t >= last.temp {
        return last.percent;
    }
    for w in points.windows(2) {
        let (a, b) = (w[0], w[1]);
        if t <= b.temp {
            if b.temp <= a.temp {
                return b.percent;
            }
            let f = (t - a.temp) / (b.temp - a.temp);
            return a.percent + (b.percent - a.percent) * f;
        }
    }
    last.percent
}

fn resolve(config: &Config, id: &str, done: &mut BTreeMap<String, CurveValue>, depth: u32) -> CurveValue {
    if let Some(v) = done.get(id) {
        return v.clone();
    }
    let failsafe = |e: String| CurveValue { temp: None, percent: FAILSAFE_PERCENT, error: Some(e) };
    if depth > 32 {
        return failsafe("mix nesting too deep".into());
    }
    let Some(curve) = config.curve(id) else { return failsafe(format!("unknown curve {id}")) };
    let v = match &curve.kind {
        CurveKind::Flat { percent } => CurveValue { temp: None, percent: *percent, error: None },
        CurveKind::Mix { curves, op } => {
            let parts: Vec<CurveValue> = curves.iter().map(|c| resolve(config, c, done, depth + 1)).collect();
            if parts.is_empty() {
                failsafe("mix has no curves".into())
            } else if let Some(err) = parts.iter().find_map(|p| p.error.clone()) {
                failsafe(err)
            } else {
                let vals = parts.iter().map(|p| p.percent);
                let percent = match op {
                    MixOp::Max => vals.fold(f32::MIN, f32::max),
                    MixOp::Min => vals.fold(f32::MAX, f32::min),
                    MixOp::Average => vals.sum::<f32>() / parts.len() as f32,
                    MixOp::Sum => vals.sum::<f32>().min(100.0),
                };
                CurveValue { temp: None, percent, error: None }
            }
        }
        // Sensor curves were all computed before mixes; reaching here means it had no entry.
        _ => failsafe(format!("curve {id} was not evaluated")),
    };
    done.insert(id.to_string(), v.clone());
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ControlConfig, Curve};

    fn temps(pairs: &[(&str, f32)]) -> HashMap<String, f32> {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    fn no_smoothing() -> Tuning {
        Tuning { hysteresis: 0.0, hysteresis_down_only: false, response_time: 0.0 }
    }

    fn graph(id: &str, sensor: &str, tuning: Tuning) -> Curve {
        Curve {
            id: id.into(),
            name: id.into(),
            kind: CurveKind::Graph {
                sensor: sensor.into(),
                points: vec![Point::new(40.0, 20.0), Point::new(80.0, 100.0)],
                tuning,
            },
        }
    }

    fn instant(control: &str, curve: &str) -> ControlConfig {
        ControlConfig { enabled: true, curve: Some(curve.into()), step_up: 0.0, step_down: 0.0, ..ControlConfig::new(control) }
    }

    #[test]
    fn interpolates_and_clamps_at_the_ends() {
        let p = [Point::new(40.0, 20.0), Point::new(60.0, 60.0), Point::new(80.0, 100.0)];
        assert_eq!(interpolate(&p, 20.0), 20.0);
        assert_eq!(interpolate(&p, 50.0), 40.0);
        assert_eq!(interpolate(&p, 70.0), 80.0);
        assert_eq!(interpolate(&p, 95.0), 100.0);
    }

    #[test]
    fn a_missing_sensor_runs_the_fan_at_full_speed() {
        let cfg = Config { curves: vec![graph("c", "t", no_smoothing())], controls: vec![instant("f", "c")], ..Config::default() };
        let mut e = Engine::new(cfg);
        let out = e.tick(&temps(&[]), 1.0);
        assert_eq!(out.controls["f"], ControlTarget::Set(100.0));
        assert!(out.curves["c"].error.is_some());
    }

    #[test]
    fn disabled_controls_are_released() {
        let mut ctl = instant("f", "c");
        ctl.enabled = false;
        let cfg = Config { curves: vec![graph("c", "t", no_smoothing())], controls: vec![ctl], ..Config::default() };
        let out = Engine::new(cfg).tick(&temps(&[("t", 50.0)]), 1.0);
        assert_eq!(out.controls["f"], ControlTarget::Release);
    }

    #[test]
    fn hysteresis_holds_small_drops_but_follows_rises() {
        let tuning = Tuning { hysteresis: 3.0, hysteresis_down_only: true, response_time: 0.0 };
        let cfg = Config { curves: vec![graph("c", "t", tuning)], controls: vec![instant("f", "c")], ..Config::default() };
        let mut e = Engine::new(cfg);
        assert_eq!(e.tick(&temps(&[("t", 60.0)]), 1.0).curves["c"].temp, Some(60.0));
        assert_eq!(e.tick(&temps(&[("t", 58.0)]), 1.0).curves["c"].temp, Some(60.0), "a 2° drop is held");
        assert_eq!(e.tick(&temps(&[("t", 61.0)]), 1.0).curves["c"].temp, Some(61.0), "rises apply at once");
        assert_eq!(e.tick(&temps(&[("t", 57.0)]), 1.0).curves["c"].temp, Some(57.0), "a 4° drop passes");
    }

    #[test]
    fn response_time_averages_recent_readings() {
        let tuning = Tuning { hysteresis: 0.0, hysteresis_down_only: false, response_time: 2.0 };
        let cfg = Config { curves: vec![graph("c", "t", tuning)], ..Config::default() };
        let mut e = Engine::new(cfg);
        e.tick(&temps(&[("t", 40.0)]), 1.0);
        e.tick(&temps(&[("t", 40.0)]), 1.0);
        let t = e.tick(&temps(&[("t", 70.0)]), 1.0).curves["c"].temp.unwrap();
        assert!((t - 50.0).abs() < 0.01, "spike averaged over three samples in 2 s: {t}");
    }

    #[test]
    fn steps_limit_how_fast_the_duty_moves() {
        let mut ctl = instant("f", "c");
        ctl.step_up = 10.0;
        ctl.step_down = 5.0;
        let cfg = Config { curves: vec![graph("c", "t", no_smoothing())], controls: vec![ctl], ..Config::default() };
        let mut e = Engine::new(cfg);
        assert_eq!(e.tick(&temps(&[("t", 40.0)]), 1.0).controls["f"], ControlTarget::Set(20.0));
        assert_eq!(e.tick(&temps(&[("t", 80.0)]), 1.0).controls["f"], ControlTarget::Set(30.0));
        assert_eq!(e.tick(&temps(&[("t", 80.0)]), 0.5).controls["f"], ControlTarget::Set(35.0));
        assert_eq!(e.tick(&temps(&[("t", 40.0)]), 1.0).controls["f"], ControlTarget::Set(30.0));
    }

    #[test]
    fn zero_rpm_stops_below_minimum_and_kicks_on_start() {
        let mut ctl = instant("f", "c");
        ctl.min_percent = 30.0;
        ctl.zero_rpm = true;
        ctl.start_percent = 50.0;
        let cfg = Config { curves: vec![graph("c", "t", no_smoothing())], controls: vec![ctl], ..Config::default() };
        let mut e = Engine::new(cfg);
        assert_eq!(e.tick(&temps(&[("t", 40.0)]), 1.0).controls["f"], ControlTarget::Set(0.0), "20 % < min → stop");
        assert_eq!(e.tick(&temps(&[("t", 50.0)]), 1.0).controls["f"], ControlTarget::Set(50.0), "40 % from stop → kick");
        assert_eq!(e.tick(&temps(&[("t", 50.0)]), 1.0).controls["f"], ControlTarget::Set(40.0));
    }

    #[test]
    fn min_percent_holds_without_zero_rpm() {
        let mut ctl = instant("f", "c");
        ctl.min_percent = 30.0;
        let cfg = Config { curves: vec![graph("c", "t", no_smoothing())], controls: vec![ctl], ..Config::default() };
        assert_eq!(Engine::new(cfg).tick(&temps(&[("t", 0.0)]), 1.0).controls["f"], ControlTarget::Set(30.0));
    }

    #[test]
    fn mix_takes_the_max_and_propagates_failures() {
        let cfg = Config {
            curves: vec![
                graph("a", "t1", no_smoothing()),
                graph("b", "t2", no_smoothing()),
                Curve { id: "m".into(), name: "m".into(), kind: CurveKind::Mix { curves: vec!["a".into(), "b".into()], op: MixOp::Max } },
            ],
            controls: vec![instant("f", "m")],
            ..Config::default()
        };
        let mut e = Engine::new(cfg.clone());
        let out = e.tick(&temps(&[("t1", 40.0), ("t2", 60.0)]), 1.0);
        assert_eq!(out.curves["m"].percent, 60.0);
        let out = Engine::new(cfg).tick(&temps(&[("t1", 40.0)]), 1.0);
        assert_eq!(out.controls["f"], ControlTarget::Set(100.0));
    }

    #[test]
    fn trigger_switches_with_a_dead_band() {
        let cfg = Config {
            curves: vec![Curve {
                id: "t".into(),
                name: "t".into(),
                kind: CurveKind::Trigger {
                    sensor: "s".into(),
                    idle_temp: 50.0,
                    load_temp: 70.0,
                    idle_percent: 25.0,
                    load_percent: 80.0,
                    tuning: no_smoothing(),
                },
            }],
            ..Config::default()
        };
        let mut e = Engine::new(cfg);
        let p = |e: &mut Engine, t| e.tick(&temps(&[("s", t)]), 1.0).curves["t"].percent;
        assert_eq!(p(&mut e, 60.0), 25.0);
        assert_eq!(p(&mut e, 72.0), 80.0);
        assert_eq!(p(&mut e, 60.0), 80.0, "stays loaded inside the band");
        assert_eq!(p(&mut e, 49.0), 25.0);
    }
}
