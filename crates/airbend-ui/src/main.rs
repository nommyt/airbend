//! airbend: the desktop app. It never touches hardware; it talks to `airbendd` over its socket,
//! shows what the daemon sees, and edits a draft config that "Apply" sends back.
//!
//! `cargo run -p airbend-ui -- --snap out.png --scene curves` renders headless with demo data.

mod demo;
mod graph;
mod theme;
mod view;
mod worker;

use std::sync::mpsc::{Receiver, Sender};
use std::time::Duration;

use airbend_core::Config;
use airbend_ipc::Snapshot;
use rune::{AppSpec, TextEdit, Tx, Waker};

use theme::{OmarchyTheme, Prefs, ThemeChoice};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Controls,
    Curves,
    Sensors,
    Settings,
}

/// What a rename edits: a sensor's or control's label, or a curve's name.
#[derive(Clone, Debug, PartialEq)]
pub enum RenameTarget {
    Label(String),
    Curve(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Conn {
    Connecting,
    Connected,
    Failed(String),
}

pub struct Ui {
    pub conn: Conn,
    pub snapshot: Snapshot,
    /// The config the daemon runs.
    pub config: Config,
    pub revision: u64,
    /// Local edits; "Apply" sends it, "Revert" drops it.
    pub draft: Config,
    pub page: Page,
    pub selected_curve: Option<String>,
    /// Which Select popup is open, by key.
    pub open: Option<String>,
    /// The graph point being dragged.
    pub drag_point: Option<usize>,
    pub notice: Option<(bool, String)>,
    pub saving: bool,
    /// Demo data from a scene: ignore the daemon.
    pub demo: bool,
    pub prefs: Prefs,
    pub omarchy: Option<OmarchyTheme>,
    omarchy_generation: Option<String>,
    last_theme_check: f64,
    pub renaming: Option<RenameTarget>,
    pub rename_edit: TextEdit,
    cmds: Option<Sender<worker::Cmd>>,
    msgs: Option<Receiver<worker::Msg>>,
}

impl Ui {
    fn new() -> Ui {
        Ui {
            conn: Conn::Connecting,
            snapshot: Snapshot::default(),
            config: Config::default(),
            revision: 0,
            draft: Config::default(),
            page: Page::Controls,
            selected_curve: None,
            open: None,
            drag_point: None,
            notice: None,
            saving: false,
            demo: false,
            prefs: Prefs::load(),
            omarchy: theme::load_omarchy(),
            omarchy_generation: theme::omarchy_generation(),
            last_theme_check: 0.0,
            renaming: None,
            rename_edit: TextEdit::default(),
            cmds: None,
            msgs: None,
        }
    }

    pub fn dirty(&self) -> bool {
        self.draft != self.config
    }

    pub fn apply(&mut self) {
        if let Err(e) = self.draft.validate() {
            self.notice = Some((false, format!("{e:#}")));
            return;
        }
        if self.demo {
            self.config = self.draft.clone();
            self.notice = Some((true, "Applied".into()));
            return;
        }
        if let Some(tx) = &self.cmds {
            self.saving = true;
            let _ = tx.send(worker::Cmd::Save(self.draft.clone()));
        }
    }

    pub fn revert(&mut self) {
        self.draft = self.config.clone();
        self.notice = None;
        if self.selected_curve.as_ref().is_some_and(|id| self.draft.curve(id).is_none()) {
            self.selected_curve = None;
        }
    }

    pub fn look(&self) -> theme::Look {
        theme::look(self.prefs.theme, self.omarchy.as_ref())
    }

    pub fn set_theme(&mut self, choice: ThemeChoice) {
        self.prefs.theme = choice;
        if choice == ThemeChoice::FollowOmarchy {
            self.omarchy = theme::load_omarchy();
            self.omarchy_generation = theme::omarchy_generation();
        }
        self.save_prefs();
    }

    pub fn set_hide_unused_outputs(&mut self, hide: bool) {
        self.prefs.hide_unused_outputs = hide;
        self.open = None;
        self.save_prefs();
    }

    fn save_prefs(&mut self) {
        if !self.demo
            && let Err(e) = self.prefs.save()
        {
            self.notice = Some((false, format!("saving preferences: {e:#}")));
        }
    }

    /// Keep known fans visible at zero RPM, including firmware-controlled GPUs.
    /// Only hide inactive headers without evidence of an attached fan.
    pub fn output_visible(&self, c: &airbend_core::ControlInfo) -> bool {
        !self.prefs.hide_unused_outputs
            || c.managed
            || self.draft.control(&c.id).is_some_and(|cfg| cfg.enabled)
            || self.config.control(&c.id).is_some_and(|cfg| cfg.enabled)
            || self.snapshot.calibrating.contains_key(&c.id)
            || self.renaming.as_ref() == Some(&RenameTarget::Label(c.id.clone()))
            || c.id.starts_with("nvidia:")
            || self.config.calibrations.get(&c.id).is_some_and(|cal| cal.fan.is_some())
            || c.paired_fan.as_ref().is_some_and(|fan| self.snapshot.sensors.iter().any(|s| {
                &s.id == fan && s.value.is_some_and(|rpm| rpm.is_finite() && rpm > 0.0)
            }))
    }

    /// Re-reads the Omarchy theme when `omarchy theme set` changed it. True when it did.
    fn check_omarchy(&mut self) -> bool {
        if self.prefs.theme != ThemeChoice::FollowOmarchy || self.demo {
            return false;
        }
        let generation = theme::omarchy_generation();
        if generation == self.omarchy_generation {
            return false;
        }
        self.omarchy_generation = generation;
        self.omarchy = theme::load_omarchy();
        true
    }

    pub fn start_rename(&mut self, target: RenameTarget) {
        let current = match &target {
            RenameTarget::Label(id) => self.label(id),
            RenameTarget::Curve(id) => self.draft.curve(id).map(|c| c.name.clone()).unwrap_or_default(),
        };
        let mut edit = TextEdit::new(current.clone());
        edit.anchor = 0;
        edit.focus = current.len();
        edit.focused = true;
        self.rename_edit = edit;
        self.renaming = Some(target);
        self.open = None;
    }

    /// Saves the rename into the draft. An empty label goes back to the hardware's name.
    pub fn finish_rename(&mut self) {
        let Some(target) = self.renaming.take() else { return };
        let text = self.rename_edit.text.trim().to_string();
        match target {
            RenameTarget::Label(id) => {
                if text.is_empty() || text == self.default_label(&id) {
                    self.draft.labels.remove(&id);
                } else {
                    self.draft.labels.insert(id, text);
                }
            }
            RenameTarget::Curve(id) => {
                if let Some(c) = self.draft.curve_mut(&id)
                    && !text.is_empty()
                {
                    c.name = text;
                }
            }
        }
    }

    pub fn cancel_rename(&mut self) {
        self.renaming = None;
    }

    pub fn calibrate(&mut self, control: &str) {
        if self.demo {
            self.notice = Some((false, "Calibration needs the daemon".into()));
            return;
        }
        if let Some(tx) = &self.cmds {
            let _ = tx.send(worker::Cmd::Calibrate(control.to_string()));
        }
    }

    pub fn cancel_calibration(&mut self, control: &str) {
        if let Some(tx) = &self.cmds {
            let _ = tx.send(worker::Cmd::CancelCalibration(control.to_string()));
        }
    }

    /// The tachometer that shows `control`'s speed: the calibrated one, else the backend's guess.
    pub fn fan_for(&self, control: &str) -> Option<String> {
        if let Some(c) = self.config.calibrations.get(control) {
            return c.fan.clone();
        }
        self.snapshot.controls.iter().find(|c| c.id == control).and_then(|c| c.paired_fan.clone())
    }

    /// The backend's name for a sensor or control.
    pub fn default_label(&self, id: &str) -> String {
        if let Some(s) = self.snapshot.sensors.iter().find(|s| s.id == id) {
            return format!("{} · {}", s.device, s.label);
        }
        if let Some(c) = self.snapshot.controls.iter().find(|c| c.id == id) {
            return format!("{} · {}", c.device, c.label);
        }
        id.to_string()
    }

    /// The label shown for a sensor or control id: the user's rename, else the backend's.
    pub fn label(&self, id: &str) -> String {
        if let Some(l) = self.draft.labels.get(id) {
            return l.clone();
        }
        self.default_label(id)
    }
}

fn setup(s: &mut Ui, waker: Waker) {
    let (cmds, msgs) = worker::spawn(airbend_ipc::default_socket(), waker);
    s.cmds = Some(cmds);
    s.msgs = Some(msgs);
}

fn poll(s: &mut Ui, t: f64, tx: &mut Tx) -> bool {
    let mut changed = false;
    if t - s.last_theme_check >= 2.0 {
        s.last_theme_check = t;
        changed |= s.check_omarchy();
    }
    if s.prefs.theme == ThemeChoice::FollowOmarchy && !s.demo {
        tx.wake_after(Duration::from_secs(2));
    }
    let Some(rx) = &s.msgs else { return changed };
    let msgs: Vec<_> = rx.try_iter().collect();
    if msgs.is_empty() || s.demo {
        return changed;
    }
    for m in msgs {
        match m {
            worker::Msg::Conn(c) => s.conn = c,
            worker::Msg::State(snap) => s.snapshot = snap,
            worker::Msg::Config(cfg, rev) => {
                let was_clean = !s.dirty();
                // Calibrations belong to the daemon: always take its copy, so applying a
                // draft never drops a result that arrived while editing.
                s.draft.calibrations = cfg.calibrations.clone();
                // Keep local edits unless they already match what the daemon now runs.
                let untouched = was_clean || s.draft == cfg;
                s.config = cfg;
                s.revision = rev;
                if untouched {
                    s.draft = s.config.clone();
                }
            }
            worker::Msg::Saved(cfg, rev) => {
                s.saving = false;
                s.revision = rev;
                s.draft.calibrations = cfg.calibrations.clone();
                s.config = cfg;
                s.notice = Some((true, "Applied".into()));
            }
            worker::Msg::SaveFailed(e) => {
                s.saving = false;
                s.notice = Some((false, e));
            }
            worker::Msg::Notice(ok, text) => s.notice = Some((ok, text)),
        }
    }
    true
}

fn main() {
    rune::set_app_id("airbend");
    rune::run(AppSpec::<Ui> {
        scenes: demo::SCENES,
        setup: Some(setup),
        poll: Some(poll),
        min_size: Some((820.0, 560.0)),
        ..AppSpec::new("airbend", (1100.0, 720.0), Ui::new, view::view)
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use airbend_core::CurveKind;
    use rune::{Env, Runtime};

    fn runtime(scene: &str) -> Runtime<Ui> {
        runtime_at(scene, (1100.0, 720.0))
    }

    fn runtime_at(scene: &str, size: (f32, f32)) -> Runtime<Ui> {
        let mut s = Ui::new();
        (demo::SCENES.iter().find(|(n, _)| *n == scene).unwrap().1)(&mut s);
        let mut rt = Runtime::new(s, view::view, size, Env { system_dark: true, scale: 1.0 });
        rt.frame(0.0);
        rt
    }

    #[test]
    fn hiding_unused_outputs_removes_empty_headers_and_can_be_toggled_back() {
        let mut rt = runtime_at("unused", (1800.0, 1000.0));
        let card = |n| format!("control.{}", view::key_safe(&format!("hwmon:it8689@it87.2624/pwm{n}")));
        assert!(rt.find(&card(2)).is_some());
        assert!(rt.find(&card(4)).is_some());
        let (x, y) = rt.find("hide-unused-outputs").unwrap();
        rt.pointer_down(x, y); rt.pointer_up(x, y); settle(&mut rt);
        assert!(rt.state.prefs.hide_unused_outputs);
        assert!(rt.find(&card(2)).is_none());
        assert!(rt.find(&card(4)).is_none());
        assert!(rt.find(&card(1)).is_some(), "detected fans stay visible under firmware control");
        assert!(rt.find(&card(3)).is_some());
        assert!(!rt.state.dirty(), "display preferences do not edit fan configuration");
        let (x, y) = rt.find("hide-unused-outputs").unwrap();
        rt.pointer_down(x, y); rt.pointer_up(x, y); settle(&mut rt);
        assert!(!rt.state.prefs.hide_unused_outputs);
        assert!(rt.find(&card(2)).is_some());
        assert!(rt.find(&card(4)).is_some());
    }

    #[test]
    fn stopped_enabled_calibrating_calibrated_and_gpu_fans_stay_visible() {
        let mut rt = runtime("unused");
        let s = &mut rt.state;
        s.prefs.hide_unused_outputs = true;
        for sensor in &mut s.snapshot.sensors {
            if sensor.kind == airbend_core::SensorKind::Fan { sensor.value = Some(0.0); }
        }
        s.draft.control_mut("hwmon:it8689@it87.2624/pwm1").enabled = true;
        s.snapshot.calibrating.insert("hwmon:it8689@it87.2624/pwm2".into(),
            airbend_core::calibrate::Progress { phase: "Spinning up".into(), duty: 100.0, fraction: 0.05 });
        s.config.calibrations.insert("hwmon:it8689@it87.2624/pwm3".into(), airbend_core::Calibration {
            fan: Some("hwmon:it8689@it87.2624/fan3".into()), max_rpm: Some(1500.0),
            stop_percent: Some(20.0), start_percent: Some(30.0), curve: vec![], at: 1,
        });
        for c in &s.snapshot.controls[..3] { assert!(s.output_visible(c), "{}", c.id); }
        assert!(!s.output_visible(&s.snapshot.controls[3]));
        let mut gpu = s.snapshot.controls[3].clone();
        gpu.id = "nvidia:GPU-demo/fan1".into();
        gpu.calibration_supported = false;
        assert!(s.output_visible(&gpu), "a GPU's zero RPM mode does not hide its fans");
    }

    #[test]
    fn calibration_updates_keep_clean_drafts_clean_and_preserve_local_edits() {
        for edited in [false, true] {
            let mut s = Ui::new();
            s.demo = false;
            if edited {
                s.draft.labels.insert("pwm1".into(), "CPU fan".into());
            }
            let mut cfg = s.config.clone();
            cfg.calibrations.insert("pwm1".into(), airbend_core::calibrate::Calibration {
                fan: Some("fan1".into()), max_rpm: Some(1500.0), stop_percent: Some(20.0),
                start_percent: Some(30.0), curve: Vec::new(), at: 1,
            });
            let (tx, rx) = std::sync::mpsc::channel();
            s.msgs = Some(rx);
            tx.send(worker::Msg::Config(cfg.clone(), 2)).unwrap();
            poll(&mut s, 0.0, &mut Tx::default());
            assert_eq!(s.draft.calibrations, cfg.calibrations);
            assert_eq!(s.dirty(), edited);
            if edited {
                assert_eq!(s.draft.labels["pwm1"], "CPU fan");
            }
        }
    }

    #[test]
    fn edits_made_during_save_remain_unapplied() {
        let mut s = Ui::new();
        s.draft.labels.insert("pwm1".into(), "CPU fan".into());
        let saved = s.draft.clone();
        s.draft.labels.insert("pwm1".into(), "Processor fan".into());
        let (tx, rx) = std::sync::mpsc::channel();
        s.msgs = Some(rx);
        tx.send(worker::Msg::Saved(saved.clone(), 2)).unwrap();
        poll(&mut s, 0.0, &mut Tx::default());
        assert_eq!(s.config, saved);
        assert_eq!(s.draft.labels["pwm1"], "Processor fan");
        assert!(s.dirty());
    }

    #[test]
    fn nvidia_controls_offer_renaming_and_manual_adjustment_without_calibration() {
        let mut rt = runtime("nvidia");
        let id = "nvidia:GPU-demo/fan1";
        assert!(rt.find(&format!("rename.{}", view::key_safe(id))).is_some());
        assert!(rt.find(&format!("calibrate.{}", view::key_safe(id))).is_none());
        rt.state.draft.control_mut(id).manual = Some(30.0);
        rt.invalidate();
        rt.frame(1.0 / 60.0);
        let key = format!("{}.manual.dec", view::key_safe(id));
        let (x, y) = rt.find(&key).expect("manual decrement button");
        rt.pointer_down(x, y); rt.pointer_up(x, y);
        settle(&mut rt);
        assert_eq!(rt.state.draft.control(id).unwrap().manual, Some(30.0));
    }

    fn settle(rt: &mut Runtime<Ui>) {
        for _ in 0..30 {
            rt.frame(1.0 / 60.0);
        }
    }

    fn graph_points(s: &Ui) -> Vec<airbend_core::Point> {
        match &s.draft.curve("cpu").unwrap().kind {
            CurveKind::Graph { points, .. } => points.clone(),
            _ => panic!("cpu is a graph"),
        }
    }

    #[test]
    fn dragging_a_graph_point_edits_the_draft_and_apply_commits_it() {
        let mut rt = runtime("curves");
        let before = graph_points(&rt.state);
        let (x, y) = rt.find("pt2").expect("point 2 is drawn");
        rt.pointer_down(x, y);
        for i in 1..=10 {
            rt.pointer_move(x, y - 4.0 * i as f32);
            rt.frame(1.0 / 60.0);
        }
        rt.pointer_up(x, y - 40.0);
        settle(&mut rt);

        let after = graph_points(&rt.state);
        assert_eq!(after.len(), before.len());
        assert!(after[2].percent > before[2].percent + 10.0, "{before:?} → {after:?}");
        assert_eq!(after[2].temp, before[2].temp, "a vertical drag keeps the temperature");
        assert!(rt.state.dirty());

        rt.state.apply();
        assert!(!rt.state.dirty(), "demo apply adopts the draft");
    }

    #[test]
    fn pressing_empty_graph_space_adds_a_point() {
        let mut rt = runtime("curves");
        let n = graph_points(&rt.state).len();
        let (gx, gy) = rt.find("graph").unwrap();
        // The graph's centre is around 60 °C, well away from existing points' y.
        rt.pointer_down(gx + 20.0, gy + 120.0);
        rt.pointer_up(gx + 20.0, gy + 120.0);
        settle(&mut rt);
        let pts = graph_points(&rt.state);
        assert_eq!(pts.len(), n + 1);
        assert!(pts.windows(2).all(|w| w[0].temp < w[1].temp), "still sorted: {pts:?}");
        rt.state.draft.validate().unwrap();
    }

    fn type_text(rt: &mut Runtime<Ui>, text: &str) {
        for c in text.chars() {
            rt.key(rune::KeyEvent { key: rune::Key::Text(c.to_string()), mods: Default::default() });
            rt.frame(1.0 / 60.0);
        }
    }

    #[test]
    fn renaming_a_fan_through_the_ui() {
        let mut rt = runtime("controls");
        let id = "hwmon:it8689@it87.2624/pwm1";
        let (x, y) = rt.find(&format!("rename.{}", view::key_safe(id))).expect("rename button");
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        settle(&mut rt);
        assert_eq!(rt.state.renaming, Some(RenameTarget::Label(id.into())));
        assert!(rt.find("rename-input").is_some());
        // The whole name starts selected, so typing replaces it.
        type_text(&mut rt, "CPU fan");
        rt.key(rune::KeyEvent { key: rune::Key::Enter, mods: Default::default() });
        settle(&mut rt);
        assert_eq!(rt.state.renaming, None);
        assert_eq!(rt.state.label(id), "CPU fan");
        assert!(rt.state.dirty(), "renames go through Apply");
    }

    #[test]
    fn clearing_a_label_resets_it_to_the_hardware_name() {
        let mut rt = runtime("controls");
        let id = "hwmon:it8689@it87.2624/pwm1";
        rt.state.start_rename(RenameTarget::Label(id.into()));
        rt.state.rename_edit.text.clear();
        rt.state.finish_rename();
        assert_eq!(rt.state.label(id), "it8689 · PWM 1");
    }

    #[test]
    fn renaming_a_curve() {
        let mut rt = runtime("curves");
        rt.state.start_rename(RenameTarget::Curve("cpu".into()));
        rt.state.rename_edit.text = "Processor".into();
        rt.state.finish_rename();
        assert_eq!(rt.state.draft.curve("cpu").unwrap().name, "Processor");
    }

    #[test]
    fn the_theme_picker_switches_between_omarchy_and_dark() {
        let mut rt = runtime("omarchy");
        let omarchy_bg = rt.state.look().kit.c.bg_base;
        let (x, y) = rt.find("theme-picker").expect("picker");
        // The right half is "Dark".
        rt.pointer_down(x + 70.0, y);
        rt.pointer_up(x + 70.0, y);
        settle(&mut rt);
        assert_eq!(rt.state.prefs.theme, ThemeChoice::Dark);
        assert_ne!(rt.state.look().kit.c.bg_base, omarchy_bg);
    }

    #[test]
    fn a_calibration_result_can_become_the_limits() {
        let mut rt = runtime("calibrated");
        let id = "hwmon:it8689@it87.2624/pwm2";
        let (x, y) = rt.find(&format!("use-calibration.{}", view::key_safe(id))).expect("button");
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        settle(&mut rt);
        let c = rt.state.draft.control(id).unwrap();
        assert_eq!((c.min_percent, c.start_percent), (35.0, 40.0));
        assert!(rt.find(&format!("use-calibration.{}", view::key_safe(id))).is_none(), "hidden once applied");
    }

    #[test]
    fn the_curve_picker_opens() {
        let mut rt = runtime("controls");
        let key = format!("curve-select.{}", view::key_safe("hwmon:it8689@it87.2624/pwm1"));
        let (x, y) = rt.find(&format!("{key}-trigger")).expect("picker drawn");
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        settle(&mut rt);
        assert_eq!(rt.state.open.as_deref(), Some(key.as_str()));
    }
}
