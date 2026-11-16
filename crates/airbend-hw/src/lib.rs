//! Hardware backends. A backend lists sensors and controllable outputs, sets an output's duty
//! (taking it from the firmware first) and hands outputs back.
//!
//! Every backend must make [`Backend::release_all`] safe to call at any time, including from a
//! shutdown path after an error: it is what keeps fans from being stranded at a low duty.

use std::path::PathBuf;

use airbend_core::{ControlInfo, SensorInfo};
use anyhow::Result;

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "linux")]
pub mod nvidia;
pub mod mock;

pub trait Backend: Send {
    fn name(&self) -> &'static str;
    /// Re-reads every sensor and output.
    fn refresh(&mut self) -> Result<()>;
    fn sensors(&self) -> Vec<SensorInfo>;
    fn controls(&self) -> Vec<ControlInfo>;
    /// Takes manual control of `control` (if not already) and sets its duty, 0–100.
    fn set(&mut self, control: &str, percent: f32) -> Result<()>;
    /// Hands `control` back to the firmware, if airbend had taken it.
    fn release(&mut self, control: &str) -> Result<()>;
    /// Hands every output airbend took back to the firmware. Never fails; logs instead.
    fn release_all(&mut self);
}

#[derive(Clone, Debug, Default)]
pub struct Options {
    /// Use the simulated backend instead of real hardware.
    pub mock: bool,
    /// Where to record each output's original mode before taking it, so a crashed daemon's
    /// outputs can be restored (`airbendd --restore`).
    pub restore_file: Option<PathBuf>,
}

pub fn open(opts: &Options) -> Result<Box<dyn Backend>> {
    if opts.mock {
        return Ok(Box::new(mock::MockBackend::new()));
    }
    #[cfg(target_os = "linux")]
    {
        let mut backends: Vec<Box<dyn Backend>> = vec![Box::new(linux::HwmonBackend::new(opts.restore_file.clone())?)];
        match nvidia::NvidiaBackend::new(opts.restore_file.as_deref().map(nvidia::restore_path)) {
            Ok(gpu) => backends.push(Box::new(gpu)),
            Err(e) => log::debug!("NVIDIA support unavailable: {e:#}"),
        }
        Ok(Box::new(CombinedBackend { backends }))
    }
    #[cfg(not(target_os = "linux"))]
    {
        anyhow::bail!("no hardware backend for this platform yet; run with --mock")
    }
}

struct CombinedBackend { backends: Vec<Box<dyn Backend>> }

impl Backend for CombinedBackend {
    fn name(&self) -> &'static str { if self.backends.len() > 1 { "hwmon + NVIDIA" } else { "hwmon" } }
    fn refresh(&mut self) -> Result<()> {
        let mut result = Ok(());
        for backend in &mut self.backends {
            if let Err(e) = backend.refresh() { result = Err(e); }
        }
        result
    }
    fn sensors(&self) -> Vec<SensorInfo> { self.backends.iter().flat_map(|b| b.sensors()).collect() }
    fn controls(&self) -> Vec<ControlInfo> { self.backends.iter().flat_map(|b| b.controls()).collect() }
    fn set(&mut self, control: &str, percent: f32) -> Result<()> {
        for b in &mut self.backends {
            if b.controls().iter().any(|c| c.id == control) { return b.set(control, percent); }
        }
        anyhow::bail!("no control {control}")
    }
    fn release(&mut self, control: &str) -> Result<()> {
        for b in &mut self.backends {
            if b.controls().iter().any(|c| c.id == control) { return b.release(control); }
        }
        anyhow::bail!("no control {control}")
    }
    fn release_all(&mut self) { for b in &mut self.backends { b.release_all(); } }
}

/// Converts 0–100 % to an 8-bit PWM value.
pub fn percent_to_pwm(p: f32) -> u8 {
    (p.clamp(0.0, 100.0) * 2.55).round() as u8
}

pub fn pwm_to_percent(v: u8) -> f32 {
    v as f32 / 2.55
}
