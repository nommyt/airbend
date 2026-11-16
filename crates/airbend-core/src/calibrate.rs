//! Fan calibration: drive one output through its range and watch every tachometer to learn
//!
//! - which tachometer the output drives (the one whose RPM follows the duty),
//! - its RPM at full duty,
//! - the lowest duty that keeps it spinning (`stop_percent`), stepping down from 100 %,
//! - the duty it needs to start from standstill (`start_percent`), stepping up from 0 %.
//!
//! Pure and clock-driven: the daemon calls [`Calibrator::step`] every tick with the time and
//! the latest readings, and applies the duty it returns. It aborts (and the daemon hands the
//! fan back to its curve) if any temperature passes [`ABORT_TEMP`] or it runs past
//! [`TIMEOUT`].

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Seconds held at each duty before reading the tachometers.
pub const SETTLE: f64 = 4.0;
/// Seconds at 100 % first, so the fan reaches full speed.
pub const SPIN_UP: f64 = 6.0;
/// Seconds held at 0 % before probing the start duty, so the fan has really stopped.
pub const REST: f64 = 6.0;
pub const STEP_DOWN: f32 = 10.0;
pub const STEP_UP: f32 = 5.0;
/// Below this a fan counts as stopped; tachometers read a few RPM of noise on some boards.
pub const STOPPED_RPM: f32 = 60.0;
/// A tachometer must move at least this much to be called the output's fan.
pub const PAIRING_RPM: f32 = 150.0;
pub const ABORT_TEMP: f32 = 85.0;
pub const TIMEOUT: f64 = 240.0;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Calibration {
    /// The tachometer that followed this output, if any did.
    pub fan: Option<String>,
    pub max_rpm: Option<f32>,
    /// Lowest duty at which the fan still spun on the way down. `Some(0.0)` when it never stops.
    pub stop_percent: Option<f32>,
    /// Duty that got the fan turning from standstill.
    pub start_percent: Option<f32>,
    /// (duty %, RPM) measured on the way down.
    pub curve: Vec<(f32, f32)>,
    /// Unix seconds.
    pub at: u64,
}

impl Calibration {
    /// Limits to suggest from this calibration: a minimum a little above where it stopped, and a
    /// start kick a little above where it started.
    pub fn suggested(&self) -> (f32, f32) {
        let min = self.stop_percent.map_or(0.0, |s| if s <= 0.0 { 0.0 } else { (s + 5.0).min(100.0) });
        let start = self.start_percent.map_or(0.0, |s| if s <= 0.0 { 0.0 } else { (s + 5.0).min(100.0) });
        (min, start.max(min))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Progress {
    pub phase: String,
    pub duty: f32,
    /// 0–1, an estimate.
    pub fraction: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    SpinUp,
    Down,
    Rest,
    Up,
}

#[derive(Debug, PartialEq)]
pub enum Step {
    /// Keep going: apply this duty.
    Run(f32),
    Done(Calibration),
    Failed(String),
}

pub struct Calibrator {
    /// A fan the backend already paired with this output; detection still runs.
    hint: Option<String>,
    phase: Phase,
    started: f64,
    phase_started: f64,
    duty: f32,
    /// Every tachometer at 100 %.
    full: BTreeMap<String, f32>,
    /// (duty, every tachometer) on the way down.
    down: Vec<(f32, BTreeMap<String, f32>)>,
    fan: Option<String>,
    stop_percent: Option<f32>,
    start_percent: Option<f32>,
}

impl Calibrator {
    pub fn new(now: f64, hint: Option<String>) -> Calibrator {
        Calibrator {
            hint,
            phase: Phase::SpinUp,
            started: now,
            phase_started: now,
            duty: 100.0,
            full: BTreeMap::new(),
            down: Vec::new(),
            fan: None,
            stop_percent: None,
            start_percent: None,
        }
    }

    pub fn progress(&self) -> Progress {
        let (phase, fraction) = match self.phase {
            Phase::SpinUp => ("Spinning up", 0.05),
            Phase::Down => ("Finding stop speed", 0.1 + (100.0 - self.duty) / 100.0 * 0.5),
            Phase::Rest => ("Letting the fan stop", 0.62),
            Phase::Up => ("Finding start speed", 0.65 + self.duty / 100.0 * 0.35),
        };
        Progress { phase: phase.into(), duty: self.duty, fraction }
    }

    /// The fan that dropped the most between 100 % and the latest step down.
    fn detect_fan(&self) -> Option<String> {
        let (_, last) = self.down.last()?;
        let mut best: Option<(String, f32)> = None;
        for (id, full) in &self.full {
            let drop = full - last.get(id).copied().unwrap_or(*full);
            if drop >= PAIRING_RPM && best.as_ref().is_none_or(|(_, d)| drop > *d) {
                best = Some((id.clone(), drop));
            }
        }
        // Prefer the backend's pairing when it also moved: two fans on one header both drop.
        if let (Some(h), Some((_, d))) = (&self.hint, &best)
            && let (Some(f), Some(l)) = (self.full.get(h), last.get(h))
            && f - l >= d * 0.5
        {
            return Some(h.clone());
        }
        best.map(|(id, _)| id)
    }

    fn finish(&self) -> Calibration {
        let fan = self.fan.clone();
        let curve = match &fan {
            Some(f) => self.down.iter().filter_map(|(d, r)| Some((*d, *r.get(f)?))).collect(),
            None => Vec::new(),
        };
        Calibration {
            max_rpm: fan.as_ref().and_then(|f| self.full.get(f).copied()),
            fan,
            stop_percent: self.stop_percent,
            start_percent: self.start_percent,
            curve,
            at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs()),
        }
    }

    /// Advances with the time (seconds), every tachometer's RPM and the hottest temperature.
    pub fn step(&mut self, now: f64, fans: &BTreeMap<String, f32>, max_temp: Option<f32>) -> Step {
        if max_temp.is_none_or(|t| !t.is_finite()) {
            return Step::Failed("stopped: temperature monitoring is unavailable".into());
        }
        if let Some(t) = max_temp
            && t >= ABORT_TEMP
        {
            return Step::Failed(format!("stopped: a sensor reached {t:.0} °C"));
        }
        if now - self.started > TIMEOUT {
            return Step::Failed("timed out".into());
        }
        if let Some(fan) = &self.fan
            && fans.get(fan).is_none_or(|rpm| !rpm.is_finite() || *rpm < 0.0)
        {
            return Step::Failed("stopped: the fan tachometer is unavailable".into());
        }
        let held = now - self.phase_started;
        match self.phase {
            Phase::SpinUp => {
                if held >= SPIN_UP {
                    self.full = fans.clone();
                    self.down.push((100.0, fans.clone()));
                    self.phase = Phase::Down;
                    self.phase_started = now;
                    self.duty = 100.0 - STEP_DOWN;
                }
            }
            Phase::Down => {
                if held >= SETTLE {
                    self.down.push((self.duty, fans.clone()));
                    if self.fan.is_none() {
                        self.fan = self.detect_fan();
                    }
                    let stopped = self.fan.as_ref().is_some_and(|f| fans.get(f).copied().unwrap_or(0.0) < STOPPED_RPM);
                    if stopped {
                        // The previous step was the last that spun.
                        self.stop_percent = Some(self.duty + STEP_DOWN);
                        self.phase = Phase::Rest;
                        self.duty = 0.0;
                    } else if self.duty <= 0.0 {
                        // Ran all the way to 0 %: either it never stops, or no fan answered.
                        if self.fan.is_some() {
                            self.stop_percent = Some(0.0);
                            self.start_percent = Some(0.0);
                        }
                        return Step::Done(self.finish());
                    } else {
                        self.duty = (self.duty - STEP_DOWN).max(0.0);
                    }
                    self.phase_started = now;
                }
            }
            Phase::Rest => {
                if held >= REST {
                    self.phase = Phase::Up;
                    self.phase_started = now;
                    self.duty = STEP_UP;
                }
            }
            Phase::Up => {
                if held >= SETTLE {
                    let rpm = self.fan.as_ref().and_then(|f| fans.get(f)).copied().unwrap_or(0.0);
                    if rpm >= STOPPED_RPM {
                        self.start_percent = Some(self.duty);
                        return Step::Done(self.finish());
                    }
                    if self.duty >= 100.0 {
                        return Step::Failed("the fan never restarted; it is back on its curve".into());
                    }
                    self.duty = (self.duty + STEP_UP).min(100.0);
                    self.phase_started = now;
                }
            }
        }
        Step::Run(self.duty)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fan on `tach` that stops below `stop` %, needs `start` % to spin up, and a second
    /// fan that ignores the output.
    struct Sim {
        stop: f32,
        start: f32,
        spinning: bool,
    }

    impl Sim {
        fn read(&mut self, duty: f32) -> BTreeMap<String, f32> {
            if self.spinning && duty < self.stop {
                self.spinning = false;
            } else if !self.spinning && duty >= self.start {
                self.spinning = true;
            }
            let rpm = if self.spinning { 300.0 + duty * 15.0 } else { 0.0 };
            [("tach".to_string(), rpm), ("other".to_string(), 900.0)].into_iter().collect()
        }
    }

    fn run(sim: &mut Sim, hint: Option<String>, temp: f32) -> Step {
        let mut c = Calibrator::new(0.0, hint);
        let mut duty = 100.0;
        let mut t = 0.0;
        loop {
            t += 1.0;
            let fans = sim.read(duty);
            match c.step(t, &fans, Some(temp)) {
                Step::Run(d) => duty = d,
                other => return other,
            }
        }
    }

    #[test]
    fn finds_the_fan_and_its_stop_and_start_duties() {
        let mut sim = Sim { stop: 25.0, start: 40.0, spinning: true };
        let Step::Done(c) = run(&mut sim, None, 50.0) else { panic!() };
        assert_eq!(c.fan.as_deref(), Some("tach"));
        assert_eq!(c.max_rpm, Some(1800.0));
        assert_eq!(c.stop_percent, Some(30.0), "spun at 30 %, stopped at 20 %");
        assert_eq!(c.start_percent, Some(40.0));
        assert_eq!(c.suggested(), (35.0, 45.0));
        assert_eq!(c.curve.first(), Some(&(100.0, 1800.0)));
    }

    #[test]
    fn a_fan_that_never_stops_reports_zero() {
        let mut sim = Sim { stop: -1.0, start: 0.0, spinning: true };
        let Step::Done(c) = run(&mut sim, None, 50.0) else { panic!() };
        assert_eq!(c.stop_percent, Some(0.0));
        assert_eq!(c.suggested(), (0.0, 0.0));
    }

    #[test]
    fn aborts_when_hot() {
        let mut sim = Sim { stop: 25.0, start: 40.0, spinning: true };
        assert!(matches!(run(&mut sim, None, 90.0), Step::Failed(_)));
    }

    #[test]
    fn aborts_when_temperature_or_paired_tachometer_is_lost() {
        let mut c = Calibrator::new(0.0, None);
        assert!(matches!(c.step(1.0, &BTreeMap::new(), None), Step::Failed(_)));
        assert!(matches!(c.step(1.0, &BTreeMap::new(), Some(f32::NAN)), Step::Failed(_)));
        let full = [("tach".into(), 1800.0)].into_iter().collect();
        let lower = [("tach".into(), 1500.0)].into_iter().collect();
        c.step(SPIN_UP, &full, Some(50.0));
        c.step(SPIN_UP + SETTLE, &lower, Some(50.0));
        assert_eq!(c.fan.as_deref(), Some("tach"));
        assert!(matches!(c.step(SPIN_UP + SETTLE + 1.0, &BTreeMap::new(), Some(50.0)), Step::Failed(_)));
    }

    #[test]
    fn an_output_with_no_fan_finishes_without_one() {
        let mut c = Calibrator::new(0.0, None);
        let fans: BTreeMap<String, f32> = [("other".to_string(), 900.0)].into_iter().collect();
        let mut t = 0.0;
        let out = loop {
            t += 1.0;
            match c.step(t, &fans, Some(50.0)) {
                Step::Run(_) => {}
                other => break other,
            }
        };
        let Step::Done(c) = out else { panic!("{out:?}") };
        assert_eq!(c.fan, None);
        assert_eq!(c.stop_percent, None);
    }
}
