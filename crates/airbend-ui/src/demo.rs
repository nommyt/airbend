//! Fixed data for headless scenes (`--scene curves`) and screenshots: no daemon involved.

use airbend_core::config::Tuning;
use airbend_core::engine::CurveValue;
use airbend_core::{Config, ControlConfig, ControlInfo, Curve, CurveKind, MixOp, SensorInfo, SensorKind};
use airbend_ipc::Snapshot;

use airbend_core::Calibration;
use airbend_core::calibrate::Progress;

use crate::theme::{self, ThemeChoice};
use crate::{Conn, Page, RenameTarget, Ui};

/// The Last Horizon theme's colours, so scenes don't depend on the machine's theme.
const LAST_HORIZON: &str = "mode = \"dark\"\naccent = \"#b59790\"\nbackground = \"#0c0b0c\"\nforeground = \"#FAFCFB\"\nyellow = \"#6B5E73\"\nred = \"#c38b7b\"\n";

fn omarchy(s: &mut Ui) {
    s.omarchy = theme::parse_colors(LAST_HORIZON, "last-horizon");
    s.prefs.theme = ThemeChoice::FollowOmarchy;
}

fn dark(s: &mut Ui) {
    s.prefs.theme = ThemeChoice::Dark;
}

fn calibration() -> Calibration {
    Calibration {
        fan: Some("hwmon:it8689@it87.2624/fan5".into()),
        max_rpm: Some(1450.0),
        stop_percent: Some(30.0),
        start_percent: Some(35.0),
        curve: vec![(100.0, 1450.0), (50.0, 834.0), (30.0, 587.0), (20.0, 0.0)],
        at: 0,
    }
}

pub const SCENES: &[(&str, fn(&mut Ui))] = &[
    ("unused", |s| {
        load(s, Page::Controls);
        s.prefs.hide_unused_outputs = false;
        for c in &mut s.snapshot.controls { c.managed = false; }
        for c in &mut s.config.controls { c.enabled = false; }
        s.snapshot.sensors.iter_mut().find(|s| s.id.ends_with("/fan2")).unwrap().value = Some(0.0);
        s.snapshot.controls.push(control(4, 100.0, false));
        s.config.labels.remove("hwmon:it8689@it87.2624/pwm2");
        s.draft = s.config.clone();
    }),
    ("nvidia", |s| {
        load(s, Page::Controls);
        s.snapshot.backend = "hwmon + NVIDIA".into();
        s.snapshot.controls = (1..=2).map(|i| ControlInfo {
            id: format!("nvidia:GPU-demo/fan{i}"), label: format!("GPU fan {i}"), device: "NVIDIA GeForce RTX 4070 SUPER".into(),
            percent: Some(40.0), paired_fan: Some(format!("nvidia:GPU-demo/rpm{i}")), managed: true,
            min_percent: 30.0, max_percent: 100.0, calibration_supported: false,
        }).collect();
        for i in 1..=2 {
            s.snapshot.sensors.push(SensorInfo { id: format!("nvidia:GPU-demo/rpm{i}"), label: format!("GPU fan {i}"),
                device: "NVIDIA GeForce RTX 4070 SUPER".into(), kind: SensorKind::Fan, value: Some(1300.0) });
            s.config.controls.push(ControlConfig { enabled: true, curve: Some("gpu".into()), min_percent: 30.0,
                ..ControlConfig::new(format!("nvidia:GPU-demo/fan{i}")) });
        }
        s.draft = s.config.clone();
    }),
    ("controls", |s| load(s, Page::Controls)),
    ("settings", |s| {
        load(s, Page::Settings);
        dark(s);
    }),
    ("omarchy", |s| {
        load(s, Page::Settings);
        omarchy(s);
    }),
    ("omarchy-curves", |s| {
        load(s, Page::Curves);
        omarchy(s);
        s.selected_curve = Some("cpu".into());
    }),
    ("calibrating", |s| {
        load(s, Page::Controls);
        s.snapshot.calibrating.insert(
            "hwmon:it8689@it87.2624/pwm1".into(),
            Progress { phase: "Finding stop speed".into(), duty: 40.0, fraction: 0.4 },
        );
    }),
    ("calibrated", |s| {
        load(s, Page::Controls);
        s.config.calibrations.insert("hwmon:it8689@it87.2624/pwm2".into(), calibration());
        s.draft = s.config.clone();
    }),
    ("renaming", |s| {
        load(s, Page::Controls);
        s.start_rename(RenameTarget::Label("hwmon:it8689@it87.2624/pwm1".into()));
    }),
    ("curves", |s| {
        load(s, Page::Curves);
        s.selected_curve = Some("cpu".into());
    }),
    ("mix", |s| {
        load(s, Page::Curves);
        s.selected_curve = Some("mix".into());
    }),
    ("sensors", |s| load(s, Page::Sensors)),
    ("dirty", |s| {
        load(s, Page::Curves);
        s.selected_curve = Some("cpu".into());
        if let Some(CurveKind::Graph { points, .. }) = s.draft.curve_mut("cpu").map(|c| &mut c.kind) {
            points[2].percent = 70.0;
        }
    }),
    ("disconnected", |s| {
        s.demo = true;
        s.conn = Conn::Failed("connecting to /run/airbend/airbend.sock: No such file or directory".into());
    }),
];

fn temp(id: &str, label: &str, device: &str, v: f32) -> SensorInfo {
    SensorInfo { id: id.into(), label: label.into(), device: device.into(), kind: SensorKind::Temperature, value: Some(v) }
}

fn fan(id: &str, label: &str, v: f32) -> SensorInfo {
    SensorInfo { id: id.into(), label: label.into(), device: "it8689".into(), kind: SensorKind::Fan, value: Some(v) }
}

fn control(n: u32, pct: f32, managed: bool) -> ControlInfo {
    ControlInfo {
        id: format!("hwmon:it8689@it87.2624/pwm{n}"),
        label: format!("PWM {n}"),
        device: "it8689".into(),
        percent: Some(pct),
        paired_fan: Some(format!("hwmon:it8689@it87.2624/fan{n}")),
        managed,
        min_percent: 0.0, max_percent: 100.0, calibration_supported: true,
    }
}

pub fn config() -> Config {
    let cpu_temp = "hwmon:k10temp@0000:00:18.3/temp1".to_string();
    let gpu_temp = "hwmon:amdgpu@0000:03:00.0/temp1".to_string();
    let mut cfg = Config {
        curves: vec![
            Curve { id: "cpu".into(), name: "CPU".into(), kind: CurveKind::default_graph(cpu_temp.clone()) },
            Curve {
                id: "gpu".into(),
                name: "GPU".into(),
                kind: CurveKind::Linear {
                    sensor: gpu_temp,
                    min_temp: 40.0,
                    max_temp: 80.0,
                    min_percent: 25.0,
                    max_percent: 90.0,
                    tuning: Tuning::default(),
                },
            },
            Curve { id: "quiet".into(), name: "Quiet floor".into(), kind: CurveKind::Flat { percent: 30.0 } },
            Curve {
                id: "mix".into(),
                name: "Case (max)".into(),
                kind: CurveKind::Mix { curves: vec!["cpu".into(), "gpu".into()], op: MixOp::Max },
            },
            Curve {
                id: "burst".into(),
                name: "Exhaust burst".into(),
                kind: CurveKind::Trigger {
                    sensor: cpu_temp,
                    idle_temp: 55.0,
                    load_temp: 75.0,
                    idle_percent: 30.0,
                    load_percent: 80.0,
                    tuning: Tuning::default(),
                },
            },
        ],
        controls: vec![
            ControlConfig { enabled: true, curve: Some("cpu".into()), min_percent: 20.0, ..ControlConfig::new(control(1, 0.0, true).id) },
            ControlConfig { enabled: true, curve: Some("mix".into()), min_percent: 25.0, zero_rpm: true, ..ControlConfig::new(control(2, 0.0, true).id) },
            ControlConfig { enabled: false, curve: Some("burst".into()), ..ControlConfig::new(control(3, 0.0, true).id) },
        ],
        ..Config::default()
    };
    cfg.labels.insert(control(1, 0.0, true).id, "CPU cooler".into());
    cfg.labels.insert(control(2, 0.0, true).id, "Front intake".into());
    cfg.labels.insert(control(3, 0.0, true).id, "Rear exhaust".into());
    cfg
}

pub fn snapshot() -> Snapshot {
    let cv = |temp: Option<f32>, percent: f32| CurveValue { temp, percent, error: None };
    Snapshot {
        backend: "hwmon".into(),
        tick: 1234,
        config_revision: 1,
        sensors: vec![
            temp("hwmon:k10temp@0000:00:18.3/temp1", "Tctl", "k10temp", 61.4),
            temp("hwmon:k10temp@0000:00:18.3/temp3", "Tccd1", "k10temp", 58.9),
            temp("hwmon:amdgpu@0000:03:00.0/temp1", "edge", "amdgpu", 52.0),
            temp("hwmon:nvme@nvme0/temp1", "Composite", "nvme", 41.9),
            temp("hwmon:it8689@it87.2624/temp1", "System", "it8689", 36.0),
            fan("hwmon:it8689@it87.2624/fan1", "Fan 1", 1104.0),
            fan("hwmon:it8689@it87.2624/fan2", "Fan 2", 860.0),
            fan("hwmon:it8689@it87.2624/fan3", "Fan 3", 742.0),
            fan("hwmon:it8689@it87.2624/fan5", "Fan 5", 1448.0),
        ],
        controls: vec![control(1, 46.0, true), control(2, 46.0, true), control(3, 38.0, false)],
        curves: [
            ("cpu".to_string(), cv(Some(61.4), 46.0)),
            ("gpu".to_string(), cv(Some(52.0), 44.5)),
            ("quiet".to_string(), cv(None, 30.0)),
            ("mix".to_string(), cv(None, 46.0)),
            ("burst".to_string(), cv(Some(61.4), 30.0)),
        ]
        .into_iter()
        .collect(),
        warnings: Vec::new(),
        ..Snapshot::default()
    }
}

fn load(s: &mut Ui, page: Page) {
    s.demo = true;
    dark(s);
    s.conn = Conn::Connected;
    s.snapshot = snapshot();
    s.config = config();
    s.draft = s.config.clone();
    s.revision = 1;
    s.page = page;
}
