//! Independent curve/control snapshots; labels and calibration are shared.

use std::collections::BTreeMap;
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use crate::{Config, ControlConfig, Curve, CurveKind};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileId {
    #[default]
    Quiet,
    Performance,
}

impl ProfileId {
    pub const ALL: [Self; 2] = [Self::Quiet, Self::Performance];
    pub fn id(self) -> &'static str { match self { Self::Quiet => "quiet", Self::Performance => "performance" } }
    pub fn label(self) -> &'static str { match self { Self::Quiet => "Quiet", Self::Performance => "Performance" } }
    pub fn parse(s: &str) -> Result<Self> {
        match s { "quiet" => Ok(Self::Quiet), "performance" => Ok(Self::Performance), _ => bail!("unknown profile {s:?}; choose quiet or performance") }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    pub curves: Vec<Curve>,
    pub controls: Vec<ControlConfig>,
}

impl Config {
    /// Migrate the existing setup without changing the duties it requests. Quiet
    /// starts as the user's exact setup; Performance gets higher demand curves.
    pub fn initialize_profiles(&mut self) -> bool {
        if !self.profiles.is_empty() { return false; }
        let quiet = Profile { curves: self.curves.clone(), controls: self.controls.clone() };
        let mut performance = quiet.clone();
        let boost = |v: &mut f32| *v = (*v + 15.0).min(100.0);
        for curve in &mut performance.curves {
            match &mut curve.kind {
                CurveKind::Flat { percent } => boost(percent),
                CurveKind::Graph { points, .. } => for p in points { boost(&mut p.percent); },
                CurveKind::Linear { min_percent, max_percent, .. } => { boost(min_percent); boost(max_percent); }
                CurveKind::Trigger { idle_percent, load_percent, .. } => { boost(idle_percent); boost(load_percent); }
                CurveKind::Mix { .. } => {} // Its inputs are already boosted.
            }
            if let Some(tuning) = curve.kind.tuning_mut() {
                tuning.response_time = tuning.response_time.min(1.0);
                tuning.hysteresis = tuning.hysteresis.min(1.0);
            }
        }
        for control in &mut performance.controls {
            control.zero_rpm = false;
            control.step_up = 0.0;
        }
        self.profiles = BTreeMap::from([(ProfileId::Quiet, quiet), (ProfileId::Performance, performance)]);
        self.active_profile = ProfileId::Quiet;
        true
    }

    pub fn save_active_profile(&mut self) {
        self.profiles.insert(self.active_profile, Profile { curves: self.curves.clone(), controls: self.controls.clone() });
    }

    pub fn select_profile(&mut self, id: ProfileId) -> Result<()> {
        self.initialize_profiles();
        if !self.profiles.contains_key(&id) { bail!("profile {} is not configured", id.label()); }
        self.save_active_profile();
        let profile = &self.profiles[&id];
        self.curves = profile.curves.clone();
        self.controls = profile.controls.clone();
        self.active_profile = id;
        self.validate()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Engine, Point};
    use crate::engine::ControlTarget;
    use std::collections::HashMap;

    fn config() -> Config {
        Config { curves: vec![Curve { id: "cpu".into(), name: "CPU".into(), kind: CurveKind::Graph {
            sensor: "cpu/temp".into(), points: vec![Point::new(30.0, 30.0), Point::new(80.0, 100.0)], tuning: Default::default(),
        }}], controls: vec![ControlConfig { enabled: true, curve: Some("cpu".into()), min_percent: 20.0, ..ControlConfig::new("fan") }], ..Config::default() }
    }

    #[test]
    fn migration_preserves_current_behavior_and_performance_increases_cooling() {
        let mut c = config(); let before = c.clone();
        assert!(c.initialize_profiles()); assert!(!c.initialize_profiles());
        assert_eq!(c.curves, before.curves); assert_eq!(c.controls, before.controls);
        let temperatures = HashMap::from([("cpu/temp".into(), 30.0)]);
        assert_eq!(Engine::new(c.clone()).tick(&temperatures, 0.0).controls["fan"], ControlTarget::Set(30.0));
        c.select_profile(ProfileId::Performance).unwrap();
        assert_eq!(Engine::new(c.clone()).tick(&temperatures, 0.0).controls["fan"], ControlTarget::Set(45.0));
        let hot = HashMap::from([("cpu/temp".into(), 85.0)]);
        assert_eq!(Engine::new(c.clone()).tick(&hot, 0.0).controls["fan"], ControlTarget::Set(100.0));
        assert_eq!(Engine::new(c).tick(&HashMap::new(), 0.0).controls["fan"], ControlTarget::Set(100.0));
    }

    #[test]
    fn switching_preserves_independent_edits_and_shared_labels() {
        let mut c = config(); c.initialize_profiles();
        c.labels.insert("fan".into(), "CPU fan".into());
        c.controls[0].min_percent = 25.0;
        c.select_profile(ProfileId::Performance).unwrap();
        assert_eq!(c.controls[0].min_percent, 20.0);
        c.controls[0].min_percent = 40.0;
        c.select_profile(ProfileId::Quiet).unwrap();
        assert_eq!(c.controls[0].min_percent, 25.0);
        c.select_profile(ProfileId::Performance).unwrap();
        assert_eq!(c.controls[0].min_percent, 40.0);
        assert_eq!(c.labels["fan"], "CPU fan");
        c.save_active_profile();
        assert_eq!(Config::from_toml(&c.to_toml().unwrap()).unwrap(), c);
    }
}
