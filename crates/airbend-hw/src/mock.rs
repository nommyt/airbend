//! A simulated machine for developing the UI and daemon without touching real fans: three
//! temperatures that drift with a fake load, and three fans whose RPM follows their duty.
//!
//! The fans behave like real ones for calibration: each stalls below its stop duty and needs
//! a higher duty to start again. Like many boards, `pwm2` doesn't drive `fan2` (which reads 0):
//! its fans report on `fan5`, so the hwmon-style pairing guess is wrong until calibrated.

use std::time::Instant;

use airbend_core::{ControlInfo, SensorInfo, SensorKind};
use anyhow::{Result, bail};

use crate::Backend;

struct Fan {
    id: &'static str,
    label: &'static str,
    /// Where its RPM shows up.
    tach: &'static str,
    /// What a hwmon-style backend would guess (`fanN` for `pwmN`).
    guess: &'static str,
    max_rpm: f32,
    stop: f32,
    start: f32,
    duty: f32,
    rpm: f32,
    spinning: bool,
    managed: bool,
}

pub struct MockBackend {
    start: Instant,
    last: f32,
    temps: [(&'static str, &'static str, &'static str, f32); 3],
    fans: Vec<Fan>,
}

impl Default for MockBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl MockBackend {
    pub fn new() -> MockBackend {
        let fan = |id, label, tach, guess, max_rpm, stop, start| Fan {
            id,
            label,
            tach,
            guess,
            max_rpm,
            stop,
            start,
            duty: 40.0,
            rpm: max_rpm * 0.4,
            spinning: true,
            managed: false,
        };
        MockBackend {
            start: Instant::now(),
            last: 0.0,
            temps: [
                ("mock:cpu/temp1", "CPU Package", "mock-cpu", 40.0),
                ("mock:gpu/temp1", "GPU Edge", "mock-gpu", 35.0),
                ("mock:nvme/temp1", "NVMe Composite", "mock-nvme", 33.0),
            ],
            fans: vec![
                fan("mock:board/pwm1", "PWM 1", "mock:board/fan1", "mock:board/fan1", 1500.0, 20.0, 30.0),
                fan("mock:board/pwm2", "PWM 2", "mock:board/fan5", "mock:board/fan2", 1450.0, 25.0, 35.0),
                fan("mock:board/pwm3", "PWM 3", "mock:board/fan3", "mock:board/fan3", 6000.0, 30.0, 45.0),
            ],
        }
    }
}

impl Backend for MockBackend {
    fn name(&self) -> &'static str {
        "mock"
    }

    fn refresh(&mut self) -> Result<()> {
        let t = self.start.elapsed().as_secs_f32();
        let dt = (t - self.last).clamp(0.0, 5.0);
        self.last = t;
        // A load that ramps up and down over a couple of minutes, with some jitter.
        let load = ((t / 40.0).sin() * 0.5 + 0.5) * 0.8 + ((t * 1.7).sin() * 0.5 + 0.5) * 0.2;
        let cooling = self.fans.iter().map(|f| f.rpm / f.max_rpm).sum::<f32>() / self.fans.len() as f32;
        let targets = [35.0 + load * 50.0 - cooling * 8.0, 32.0 + load * 38.0 - cooling * 4.0, 30.0 + load * 15.0];
        for (i, (_, _, _, v)) in self.temps.iter_mut().enumerate() {
            *v += (targets[i] - *v) * (dt * 0.3).min(1.0);
        }
        for f in &mut self.fans {
            let duty = if f.managed { f.duty } else { 40.0 };
            if f.spinning && duty < f.stop {
                f.spinning = false;
            } else if !f.spinning && duty >= f.start {
                f.spinning = true;
            }
            let target = if f.spinning { f.max_rpm * (0.15 + 0.85 * duty / 100.0) } else { 0.0 };
            f.rpm += (target - f.rpm) * (dt * 1.5).min(1.0);
        }
        Ok(())
    }

    fn sensors(&self) -> Vec<SensorInfo> {
        let temps = self.temps.iter().map(|&(id, label, device, v)| SensorInfo {
            id: id.into(),
            label: label.into(),
            device: device.into(),
            kind: SensorKind::Temperature,
            value: Some((v * 10.0).round() / 10.0),
        });
        let fans = self.fans.iter().map(|f| SensorInfo {
            id: f.tach.into(),
            label: f.tach.rsplit('/').next().unwrap_or(f.tach).replace("fan", "Fan "),
            device: "mock-board".into(),
            kind: SensorKind::Fan,
            value: Some(f.rpm.round()),
        });
        // A header with nothing on its tachometer.
        let dead = std::iter::once(SensorInfo {
            id: "mock:board/fan2".into(),
            label: "Fan 2".into(),
            device: "mock-board".into(),
            kind: SensorKind::Fan,
            value: Some(0.0),
        });
        temps.chain(fans).chain(dead).collect()
    }

    fn controls(&self) -> Vec<ControlInfo> {
        self.fans
            .iter()
            .map(|f| ControlInfo {
                id: f.id.into(),
                label: f.label.into(),
                device: "mock-board".into(),
                percent: Some(if f.managed { f.duty } else { 40.0 }),
                paired_fan: Some(f.guess.into()),
                managed: f.managed,
                min_percent: 0.0, max_percent: 100.0, calibration_supported: true,
            })
            .collect()
    }

    fn set(&mut self, control: &str, percent: f32) -> Result<()> {
        let Some(f) = self.fans.iter_mut().find(|f| f.id == control) else { bail!("no control {control}") };
        f.managed = true;
        f.duty = percent.clamp(0.0, 100.0);
        Ok(())
    }

    fn release(&mut self, control: &str) -> Result<()> {
        if let Some(f) = self.fans.iter_mut().find(|f| f.id == control) {
            f.managed = false;
        }
        Ok(())
    }

    fn release_all(&mut self) {
        for f in &mut self.fans {
            f.managed = false;
        }
    }
}
