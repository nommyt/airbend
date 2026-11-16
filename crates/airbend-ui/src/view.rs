//! Everything on screen: a sidebar with three pages (Controls, Curves, Sensors), a header with
//! Apply/Revert while the draft differs from the daemon's config, and the page itself.

use airbend_core::config::Tuning;
use airbend_core::{ControlConfig, Curve, CurveKind, MixOp, SensorKind};
use rune::{Color, Cx, El, ScrollSpec, TextStyle, Tx, col, el, row, text};
use rune_kit::{Badge, BadgeVariant, Button, ButtonVariant, ControlSize, Kit, SegmentedControl, Select, Switch, TextInput};

use crate::theme::ThemeChoice;
use crate::{Conn, Page, RenameTarget, Ui, graph};

const TITLE: TextStyle = TextStyle::new(20.0, 600.0);
const SECTION: TextStyle = TextStyle::new(13.0, 600.0);

fn big() -> TextStyle {
    TextStyle::new(28.0, 600.0).features("'tnum' 1")
}

fn accent(k: &Kit) -> Color {
    k.c.text_accent
}

const BAD: Color = Color { r: 0.95, g: 0.38, b: 0.33, a: 1.0 };

pub fn view(s: &Ui, _cx: &mut Cx<Ui>) -> El {
    let k = s.look().kit;
    let mut layer = Vec::new();
    let page = match s.page {
        Page::Controls => controls_page(s, &k, &mut layer),
        Page::Curves => curves_page(s, &k, &mut layer),
        Page::Sensors => sensors_page(s, &k),
        Page::Settings => settings_page(s, &k),
    };
    let mut content = col().w_full().padding(24.0).gap(16.0);
    if let Conn::Failed(e) = &s.conn {
        content = content.child(banner(&k, e));
    }
    for w in s.snapshot.warnings.iter().rev().take(3) {
        content = content.child(text(w).font(k.ty.small).color(Color { r: 0.95, g: 0.6, b: 0.25, a: 1.0 }).wrap());
    }
    content = content.child(page);

    let main = col()
        .grow(1.0)
        .h_full()
        .min_w(0.0)
        .child(header(s, &k))
        .child(col().key("content").w_full().grow(1.0).min_h(0.0).scroll(ScrollSpec { stick_bottom: false }).child(content));
    let body = row().w_full().h_full().fill(k.c.bg_base).child(sidebar(s, &k)).child(main);
    rune_kit::root(body, layer)
}

fn sidebar(s: &Ui, k: &Kit) -> El {
    let item = |page: Page, label: &str| {
        let on = s.page == page;
        row()
            .key(format!("nav-{label}"))
            .role("tab")
            .label(label)
            .w_full()
            .px(12.0)
            .py(8.0)
            .radius(6.0)
            .fill(if on { k.c.layer_02 } else { Color { a: 0.0, ..k.c.layer_02 } })
            .hover(k.c.hover)
            .cursor(rune::Cursor::Pointer)
            .on_press(move |s: &mut Ui| {
                s.page = page;
                s.open = None;
            })
            .child(text(label).font(if on { k.ty.ui_medium } else { k.ty.ui }).color(if on { k.c.text_base } else { k.c.text_muted }))
    };
    let (dot, status) = match &s.conn {
        Conn::Connecting => (k.c.text_faint, "Connecting…".to_string()),
        Conn::Connected => (Color { r: 0.3, g: 0.8, b: 0.45, a: 1.0 }, format!("Connected · {}", s.snapshot.backend)),
        Conn::Failed(_) => (Color { r: 0.95, g: 0.35, b: 0.3, a: 1.0 }, "Daemon not running".to_string()),
    };
    col()
        .w(200.0)
        .h_full()
        .padding(12.0)
        .gap(4.0)
        .fill(k.c.bg_deep)
        .border_sides([0.0, 1.0, 0.0, 0.0], k.c.border_base)
        .child(text("airbend").font(TextStyle::new(16.0, 700.0)).color(k.c.text_base).px(12.0).py(10.0))
        .child(item(Page::Controls, "Controls"))
        .child(item(Page::Curves, "Curves"))
        .child(item(Page::Sensors, "Sensors"))
        .child(item(Page::Settings, "Settings"))
        .child(el().grow(1.0))
        .child(
            row()
                .gap(8.0)
                .px(12.0)
                .py(8.0)
                .items_center()
                .child(el().size(8.0, 8.0).radius(4.0).fill(dot))
                .child(text(status).font(k.ty.small).color(k.c.text_muted)),
        )
}

fn header(s: &Ui, k: &Kit) -> El {
    let title = match s.page {
        Page::Controls => "Controls",
        Page::Curves => "Curves",
        Page::Sensors => "Sensors",
        Page::Settings => "Settings",
    };
    let mut right = row().gap(8.0).items_center();
    if let Some((ok, msg)) = &s.notice {
        let c = if *ok { k.c.text_muted } else { Color { r: 0.95, g: 0.4, b: 0.35, a: 1.0 } };
        right = right.child(text(msg).font(k.ty.small).color(c).max_w(420.0).truncate());
    }
    if s.dirty() {
        right = right
            .child(text("Unsaved changes").font(k.ty.small).color(k.c.text_faint))
            .child(Button::new("Revert").variant(ButtonVariant::Ghost).on_press(|s: &mut Ui| s.revert()).build(k))
            .child(
                Button::new(if s.saving { "Applying…" } else { "Apply" })
                    .variant(ButtonVariant::Contrast)
                    .disabled(s.saving)
                    .on_press(|s: &mut Ui| s.apply())
                    .build(k),
            );
    }
    row()
        .w_full()
        .h(56.0)
        .px(24.0)
        .items_center()
        .justify_between()
        .border_sides([0.0, 0.0, 1.0, 0.0], k.c.border_base)
        .child(text(title).font(TITLE).color(k.c.text_base))
        .child(right)
}

fn banner(k: &Kit, err: &str) -> El {
    col()
        .w_full()
        .padding(14.0)
        .gap(4.0)
        .radius(8.0)
        .fill(Color { r: 0.95, g: 0.35, b: 0.3, a: 0.12 })
        .child(text("Can't reach airbendd").font(k.ty.ui_medium).color(k.c.text_base))
        .child(text(err).font(k.ty.small).color(k.c.text_muted).wrap())
        .child(
            text("Start it with `sudo systemctl enable --now airbendd`, or for a dry run: `airbendd --mock --socket /tmp/airbend.sock` and AIRBEND_SOCKET=/tmp/airbend.sock airbend.")
                .font(k.ty.small)
                .color(k.c.text_faint)
                .wrap(),
        )
}

fn card(k: &Kit) -> El {
    col().padding(16.0).gap(12.0).radius(10.0).fill(k.c.layer_01).border(1.0, k.c.border_muted)
}

/// Element keys can't contain `/`: Rune reads it as a key path. Hardware ids do.
pub fn key_safe(id: &str) -> String {
    id.replace('/', "|")
}

fn fmt(v: f32) -> String {
    if (v - v.round()).abs() < 0.05 { format!("{v:.0}") } else { format!("{v:.1}") }
}

/// A labelled number with − and + buttons.
fn stepper(
    k: &Kit,
    key: String,
    label: &str,
    value: String,
    dec: impl Fn(&mut Ui) + 'static,
    inc: impl Fn(&mut Ui) + 'static,
) -> El {
    row()
        .key(key.clone())
        .w_full()
        .gap(6.0)
        .items_center()
        .child(text(label).font(k.ty.ui).color(k.c.text_muted).grow(1.0).min_w(0.0).truncate())
        .child(Button::new("−").control_size(ControlSize::Small).variant(ButtonVariant::Ghost).on_press(dec).build(k).key(format!("{key}.dec")))
        .child(text(value).font(k.ty.ui_medium.features("'tnum' 1")).color(k.c.text_base).w(64.0).align(rune::Align::Center))
        .child(Button::new("+").control_size(ControlSize::Small).variant(ButtonVariant::Ghost).on_press(inc).build(k).key(format!("{key}.inc")))
}

fn switch_row(k: &Kit, label: &str, on: bool, toggle: impl Fn(&mut Ui) + 'static) -> El {
    row()
        .w_full()
        .items_center()
        .justify_between()
        .child(text(label).font(k.ty.ui).color(k.c.text_muted))
        .child(Switch::new(on).label(label).hide_label().on_toggle(toggle).build(k))
}

fn edit_control(id: &str, f: impl Fn(&mut ControlConfig) + 'static) -> impl Fn(&mut Ui) + 'static {
    let id = id.to_string();
    move |s: &mut Ui| f(s.draft.control_mut(&id))
}

fn edit_curve(id: &str, f: impl Fn(&mut CurveKind) + 'static) -> impl Fn(&mut Ui) + 'static {
    let id = id.to_string();
    move |s: &mut Ui| {
        if let Some(c) = s.draft.curve_mut(&id) {
            f(&mut c.kind)
        }
    }
}

/// A stepper over one f32 field of a control, clamped to `lo..=hi`.
#[allow(clippy::too_many_arguments)]
fn control_stepper(
    k: &Kit,
    id: &str,
    label: &str,
    value: f32,
    unit: &str,
    step: f32,
    lo: f32,
    hi: f32,
    field: fn(&mut ControlConfig) -> &mut f32,
) -> El {
    stepper(
        k,
        format!("{}.{label}", key_safe(id)),
        label,
        format!("{}{unit}", fmt(value)),
        edit_control(id, move |c| *field(c) = (*field(c) - step).clamp(lo, hi)),
        edit_control(id, move |c| *field(c) = (*field(c) + step).clamp(lo, hi)),
    )
}

/// A stepper over one f32 field of a curve.
#[allow(clippy::too_many_arguments)]
fn curve_stepper(
    k: &Kit,
    id: &str,
    label: &str,
    value: f32,
    unit: &str,
    step: f32,
    lo: f32,
    hi: f32,
    field: fn(&mut CurveKind) -> Option<&mut f32>,
) -> El {
    stepper(
        k,
        format!("{}.{label}", key_safe(id)),
        label,
        format!("{}{unit}", fmt(value)),
        edit_curve(id, move |c| {
            if let Some(v) = field(c) {
                *v = (*v - step).clamp(lo, hi)
            }
        }),
        edit_curve(id, move |c| {
            if let Some(v) = field(c) {
                *v = (*v + step).clamp(lo, hi)
            }
        }),
    )
}

// ───────────────────────────── Controls ─────────────────────────────

fn controls_page(s: &Ui, k: &Kit, layer: &mut Vec<El>) -> El {
    if s.conn != Conn::Connected {
        return el();
    }
    if s.snapshot.controls.is_empty() {
        return empty(
            k,
            "No controllable fans found",
            "airbend sees no PWM outputs. On most desktop boards the fan headers sit behind a Super I/O chip whose driver isn't loaded: \
             run `sudo sensors-detect`, then load the driver it suggests (often `it87` for Gigabyte boards or `nct6775` for ASUS/MSI). \
             Laptops usually keep fans under firmware control.",
        );
    }
    let filter = row().w_full().justify_between().items_center().gap(12.0)
        .child(text("Hide inactive outputs with no detected fan").font(k.ty.small).color(k.c.text_muted).wrap())
        .child(Switch::new(s.prefs.hide_unused_outputs)
            .label("Hide unused outputs")
            .on_toggle(|s: &mut Ui| s.set_hide_unused_outputs(!s.prefs.hide_unused_outputs))
            .build(k).key("hide-unused-outputs"));
    let mut grid = row().w_full().flex_wrap().gap(16.0).items_start();
    let mut visible = 0;
    for c in s.snapshot.controls.iter().filter(|c| s.output_visible(c)) {
        visible += 1;
        grid = grid.child(control_card(s, k, c, layer));
    }
    let mut body = col().w_full().gap(16.0).child(filter);
    if visible == 0 {
        body = body.child(empty(k, "No fans to show", "Turn off Hide unused outputs to inspect every output or calibrate a fan."));
    } else {
        body = body.child(grid);
    }
    body
}

fn control_card(s: &Ui, k: &Kit, info: &airbend_core::ControlInfo, layer: &mut Vec<El>) -> El {
    let id = info.id.as_str();
    let cfg = s.draft.control(id).cloned().unwrap_or_else(|| ControlConfig::new(id));
    let rpm = s
        .fan_for(id)
        .and_then(|f| s.snapshot.sensors.iter().find(|x| x.id == f))
        .and_then(|f| f.value);

    let target = RenameTarget::Label(id.to_string());
    let renaming = s.renaming.as_ref() == Some(&target);
    let title = if renaming {
        rename_field(s, k)
    } else {
        row()
            .gap(4.0)
            .items_center()
            .min_w(0.0)
            .child(text(s.label(id)).font(k.ty.body_medium).color(k.c.text_base).truncate().shrink())
            .child(rename_button(k, format!("rename.{}", key_safe(id)), target))
    };
    let head = row()
        .w_full()
        .items_start()
        .justify_between()
        .gap(8.0)
        .child(
            col()
                .gap(2.0)
                .min_w(0.0)
                .shrink()
                .child(title)
                .child(text(id).font(k.ty.small).color(k.c.text_faint).truncate()),
        )
        .child(if renaming {
            el()
        } else {
            Badge::new(if info.managed { "airbend" } else { "firmware" })
                .variant(if info.managed { BadgeVariant::Accent } else { BadgeVariant::Subtle })
                .build(k)
        });

    let readout = row()
        .gap(16.0)
        .items_end()
        .child(text(info.percent.map_or("—".into(), |p| format!("{p:.0}%"))).font(big()).color(k.c.text_base))
        .child(
            text(rpm.map_or("no tachometer".into(), |r| format!("{r:.0} RPM")))
                .font(k.ty.ui)
                .color(k.c.text_muted)
                .pb(5.0),
        );

    // Curve picker: "None" plus every curve, by name.
    let mut names: Vec<(String, Option<String>)> = vec![("None".into(), None)];
    names.extend(s.draft.curves.iter().map(|c| (c.name.clone(), Some(c.id.clone()))));
    let labels: Vec<String> = names.iter().map(|(n, _)| n.clone()).collect();
    let current = names.iter().find(|(_, cid)| *cid == cfg.curve).map(|(n, _)| n.clone());
    let key = format!("curve-select.{}", key_safe(id));
    let picker = {
        let (key_t, id_c) = (key.clone(), id.to_string());
        Select::new(key.clone(), &labels)
            .current(current.as_deref())
            .open(s.open.as_deref() == Some(key.as_str()))
            .width(180.0)
            .on_toggle(move |s: &mut Ui| s.open = if s.open.as_deref() == Some(key_t.as_str()) { None } else { Some(key_t.clone()) })
            .on_close(|s: &mut Ui| s.open = None)
            .on_choose(move |s: &mut Ui, v: &str, _: &mut Tx| {
                let pick = names.iter().find(|(n, _)| n == v).and_then(|(_, c)| c.clone());
                let c = s.draft.control_mut(&id_c);
                c.curve = pick.clone();
                if pick.is_some() {
                    c.enabled = true;
                }
                s.open = None;
            })
            .build(k, layer)
    };

    let mut body = card(k)
        .key(format!("control.{}", key_safe(id)))
        .w(340.0)
        .child(head)
        .child(readout)
        .child(switch_row(k, "Control with airbend", cfg.enabled, edit_control(id, |c| c.enabled = !c.enabled)))
        .child(
            row()
                .w_full()
                .items_center()
                .justify_between()
                .child(text("Curve").font(k.ty.ui).color(k.c.text_muted))
                .child(picker),
        );

    if let Some(m) = cfg.manual {
        let (min, max) = (info.min_percent, info.max_percent);
        body = body.child(stepper(
            k,
            format!("{}.manual", key_safe(id)),
            "Manual",
            format!("{m:.0}%"),
            edit_control(id, move |c| c.manual = c.manual.map(|m| (m - 5.0).max(min))),
            edit_control(id, move |c| c.manual = c.manual.map(|m| (m + 5.0).min(max))),
        ));
    }
    body = body.child(calibration_section(s, k, id));
    body = body.child(switch_row(
        k,
        "Manual override",
        cfg.manual.is_some(),
        edit_control(id, |c| c.manual = if c.manual.is_some() { None } else { Some(50.0) }),
    ))
    .child(el().h(1.0).w_full().fill(k.c.border_muted))
    .child(control_stepper(k, id, "Minimum", cfg.min_percent.max(info.min_percent), "%", 5.0, info.min_percent, info.max_percent, |c| &mut c.min_percent));
    if info.calibration_supported {
        body = body.child(switch_row(k, "Stop below minimum", cfg.zero_rpm, edit_control(id, |c| c.zero_rpm = !c.zero_rpm)))
            .child(control_stepper(k, id, "Start kick", cfg.start_percent, "%", 5.0, 0.0, 100.0, |c| &mut c.start_percent));
    }
    body.child(control_stepper(k, id, "Step up", cfg.step_up, "%/s", 1.0, 0.0, 100.0, |c| &mut c.step_up))
    .child(control_stepper(k, id, "Step down", cfg.step_down, "%/s", 1.0, 0.0, 100.0, |c| &mut c.step_down))
    .child(control_stepper(k, id, "Offset", cfg.offset, "%", 5.0, -50.0, 50.0, |c| &mut c.offset))
}

fn empty(k: &Kit, title: &str, body: &str) -> El {
    col()
        .w_full()
        .max_w(640.0)
        .padding(24.0)
        .gap(8.0)
        .radius(10.0)
        .fill(k.c.layer_01)
        .child(text(title).font(k.ty.body_medium).color(k.c.text_base))
        .child(text(body).font(k.ty.ui).color(k.c.text_muted).wrap())
}

// ───────────────────────────── Curves ─────────────────────────────

fn first_temp_sensor(s: &Ui) -> String {
    s.snapshot
        .sensors
        .iter()
        .find(|x| x.kind == SensorKind::Temperature)
        .map(|x| x.id.clone())
        .unwrap_or_default()
}

fn add_curve(s: &mut Ui, kind: &str) {
    let sensor = first_temp_sensor(s);
    let id = s.draft.fresh_curve_id();
    let (name, kind) = match kind {
        "Graph" => ("Graph", CurveKind::default_graph(sensor)),
        "Linear" => (
            "Linear",
            CurveKind::Linear { sensor, min_temp: 40.0, max_temp: 80.0, min_percent: 25.0, max_percent: 100.0, tuning: Tuning::default() },
        ),
        "Trigger" => (
            "Trigger",
            CurveKind::Trigger {
                sensor,
                idle_temp: 50.0,
                load_temp: 70.0,
                idle_percent: 30.0,
                load_percent: 80.0,
                tuning: Tuning::default(),
            },
        ),
        "Mix" => ("Mix", CurveKind::Mix { curves: Vec::new(), op: MixOp::Max }),
        _ => ("Flat", CurveKind::Flat { percent: 50.0 }),
    };
    let n = s.draft.curves.iter().filter(|c| c.kind.type_label() == name).count() + 1;
    s.draft.curves.push(Curve { id: id.clone(), name: format!("{name} {n}"), kind });
    s.selected_curve = Some(id);
}

fn curves_page(s: &Ui, k: &Kit, layer: &mut Vec<El>) -> El {
    let mut list = col().w(260.0).gap(6.0);
    let mut adds = row().flex_wrap().gap(6.0);
    for kind in ["Graph", "Linear", "Flat", "Trigger", "Mix"] {
        adds = adds.child(
            Button::new(format!("+ {kind}"))
                .control_size(ControlSize::Small)
                .variant(ButtonVariant::Outline)
                .on_press(move |s: &mut Ui| add_curve(s, kind))
                .build(k),
        );
    }
    list = list.child(adds).child(el().h(6.0));
    if s.draft.curves.is_empty() {
        list = list.child(text("No curves yet. Add one, then pick it on a fan in Controls.").font(k.ty.small).color(k.c.text_faint).wrap());
    }
    for c in &s.draft.curves {
        let on = s.selected_curve.as_deref() == Some(c.id.as_str());
        let live = s.snapshot.curves.get(&c.id);
        let value = match live {
            Some(v) if v.error.is_some() => "failsafe".to_string(),
            Some(v) => match v.temp {
                Some(t) => format!("{t:.0} °C → {:.0}%", v.percent),
                None => format!("{:.0}%", v.percent),
            },
            None => "not applied".into(),
        };
        let cid = c.id.clone();
        list = list.child(
            col()
                .key(format!("curve.{}", key_safe(&c.id)))
                .role("tab")
                .label(&c.name)
                .w_full()
                .padding(12.0)
                .gap(2.0)
                .radius(8.0)
                .fill(if on { k.c.layer_02 } else { k.c.layer_01 })
                .border(1.0, if on { accent(k) } else { k.c.border_muted })
                .hover(k.c.hover)
                .cursor(rune::Cursor::Pointer)
                .on_press(move |s: &mut Ui| {
                    s.selected_curve = Some(cid.clone());
                    s.open = None;
                })
                .child(
                    row()
                        .w_full()
                        .justify_between()
                        .child(text(&c.name).font(k.ty.ui_medium).color(k.c.text_base).truncate().shrink())
                        .child(text(c.kind.type_label()).font(k.ty.small).color(k.c.text_faint)),
                )
                .child(text(value).font(k.ty.small.features("'tnum' 1")).color(k.c.text_muted)),
        );
    }

    let editor = match s.selected_curve.as_deref().and_then(|id| s.draft.curve(id)) {
        Some(c) => curve_editor(s, k, c, layer),
        None => empty(k, "Select a curve", "Curves turn a temperature into a fan speed. Pick one on the left, or add one."),
    };
    row().w_full().gap(20.0).items_start().child(list).child(col().grow(1.0).min_w(0.0).child(editor))
}

fn curve_editor(s: &Ui, k: &Kit, c: &Curve, layer: &mut Vec<El>) -> El {
    let id = c.id.as_str();
    let live = s.snapshot.curves.get(id);
    let used_by: Vec<String> = s
        .draft
        .controls
        .iter()
        .filter(|x| x.curve.as_deref() == Some(id))
        .map(|x| s.label(&x.control))
        .collect();

    let mut ed = card(k).w_full().max_w(620.0).child(
        row()
            .w_full()
            .items_center()
            .justify_between()
            .child(
                col()
                    .gap(2.0)
                    .child(if s.renaming.as_ref() == Some(&RenameTarget::Curve(c.id.clone())) {
                        rename_field(s, k)
                    } else {
                        row()
                            .gap(4.0)
                            .items_center()
                            .child(text(&c.name).font(TextStyle::new(16.0, 600.0)).color(k.c.text_base))
                            .child(rename_button(k, format!("rename-curve.{}", key_safe(id)), RenameTarget::Curve(c.id.clone())))
                    })
                    .child(
                        text(if used_by.is_empty() { "Not used by any fan".to_string() } else { format!("Drives {}", used_by.join(", ")) })
                            .font(k.ty.small)
                            .color(k.c.text_faint),
                    ),
            )
            .child(Badge::new(c.kind.type_label()).build(k)),
    );

    if let Some(v) = live {
        let line = match (&v.error, v.temp) {
            (Some(e), _) => format!("Failsafe 100 %: {e}"),
            (None, Some(t)) => format!("Now {t:.1} °C → {:.0} %", v.percent),
            (None, None) => format!("Now {:.0} %", v.percent),
        };
        ed = ed.child(text(line).font(k.ty.ui.features("'tnum' 1")).color(k.c.text_muted));
    }

    if let Some(sensor) = c.kind.sensor() {
        ed = ed.child(sensor_picker(s, k, id, sensor, layer));
    }

    ed = match &c.kind {
        CurveKind::Graph { points, .. } => {
            let current = live.and_then(|v| Some((v.temp?, v.percent)));
            ed.child(graph::editor(k, id, points, current, accent(k), s.look().warm, s.drag_point)).child(
                text("Drag a point to move it · press empty space to add one · double-click a point to remove it")
                    .font(k.ty.small)
                    .color(k.c.text_faint),
            )
        }
        CurveKind::Linear { min_temp, max_temp, min_percent, max_percent, .. } => ed
            .child(curve_stepper(k, id, "From temperature", *min_temp, " °C", 1.0, 0.0, 120.0, |c| match c {
                CurveKind::Linear { min_temp, .. } => Some(min_temp),
                _ => None,
            }))
            .child(curve_stepper(k, id, "To temperature", *max_temp, " °C", 1.0, 0.0, 120.0, |c| match c {
                CurveKind::Linear { max_temp, .. } => Some(max_temp),
                _ => None,
            }))
            .child(curve_stepper(k, id, "Speed at start", *min_percent, "%", 5.0, 0.0, 100.0, |c| match c {
                CurveKind::Linear { min_percent, .. } => Some(min_percent),
                _ => None,
            }))
            .child(curve_stepper(k, id, "Speed at end", *max_percent, "%", 5.0, 0.0, 100.0, |c| match c {
                CurveKind::Linear { max_percent, .. } => Some(max_percent),
                _ => None,
            })),
        CurveKind::Flat { percent } => ed.child(curve_stepper(k, id, "Speed", *percent, "%", 5.0, 0.0, 100.0, |c| match c {
            CurveKind::Flat { percent } => Some(percent),
            _ => None,
        })),
        CurveKind::Trigger { idle_temp, load_temp, idle_percent, load_percent, .. } => ed
            .child(curve_stepper(k, id, "Back to idle at", *idle_temp, " °C", 1.0, 0.0, 120.0, |c| match c {
                CurveKind::Trigger { idle_temp, .. } => Some(idle_temp),
                _ => None,
            }))
            .child(curve_stepper(k, id, "Load at", *load_temp, " °C", 1.0, 0.0, 120.0, |c| match c {
                CurveKind::Trigger { load_temp, .. } => Some(load_temp),
                _ => None,
            }))
            .child(curve_stepper(k, id, "Idle speed", *idle_percent, "%", 5.0, 0.0, 100.0, |c| match c {
                CurveKind::Trigger { idle_percent, .. } => Some(idle_percent),
                _ => None,
            }))
            .child(curve_stepper(k, id, "Load speed", *load_percent, "%", 5.0, 0.0, 100.0, |c| match c {
                CurveKind::Trigger { load_percent, .. } => Some(load_percent),
                _ => None,
            })),
        CurveKind::Mix { curves, op } => mix_editor(s, k, id, curves, *op, ed),
    };

    if let Some(t) = c.kind.tuning() {
        ed = ed
            .child(el().h(1.0).w_full().fill(k.c.border_muted))
            .child(text("Response").font(SECTION).color(k.c.text_base))
            .child(curve_stepper(k, id, "Hysteresis", t.hysteresis, " °C", 0.5, 0.0, 20.0, |c| c.tuning_mut().map(|t| &mut t.hysteresis)))
            .child(curve_stepper(k, id, "Response time", t.response_time, " s", 1.0, 0.0, 60.0, |c| {
                c.tuning_mut().map(|t| &mut t.response_time)
            }))
            .child(switch_row(
                k,
                "Hysteresis only when cooling",
                t.hysteresis_down_only,
                edit_curve(id, |c| {
                    if let Some(t) = c.tuning_mut() {
                        t.hysteresis_down_only = !t.hysteresis_down_only
                    }
                }),
            ));
    }

    let del = id.to_string();
    ed.child(
        row().w_full().justify_end().child(
            Button::new("Delete curve")
                .variant(ButtonVariant::Danger)
                .control_size(ControlSize::Small)
                .on_press(move |s: &mut Ui| {
                    s.draft.remove_curve(&del);
                    s.selected_curve = None;
                })
                .build(k),
        ),
    )
}

fn sensor_picker(s: &Ui, k: &Kit, id: &str, current: &str, layer: &mut Vec<El>) -> El {
    let temps: Vec<(String, String)> = s
        .snapshot
        .sensors
        .iter()
        .filter(|x| x.kind == SensorKind::Temperature)
        .map(|x| {
            let v = x.value.map_or(String::new(), |v| format!("  {v:.0}°"));
            (format!("{}{v}", s.label(&x.id)), x.id.clone())
        })
        .collect();
    let labels: Vec<String> = temps.iter().map(|(l, _)| l.clone()).collect();
    let cur = temps.iter().find(|(_, sid)| sid == current).map(|(l, _)| l.clone());
    let key = format!("sensor-select.{}", key_safe(id));
    let (key_t, cid) = (key.clone(), id.to_string());
    let select = Select::new(key.clone(), &labels)
        .current(cur.as_deref())
        .placeholder(if current.is_empty() { "Pick a sensor".to_string() } else { format!("{current} (missing)") })
        .open(s.open.as_deref() == Some(key.as_str()))
        .width(320.0)
        .on_toggle(move |s: &mut Ui| s.open = if s.open.as_deref() == Some(key_t.as_str()) { None } else { Some(key_t.clone()) })
        .on_close(|s: &mut Ui| s.open = None)
        .on_choose(move |s: &mut Ui, v: &str, _: &mut Tx| {
            if let Some((_, sid)) = temps.iter().find(|(l, _)| l == v)
                && let Some(c) = s.draft.curve_mut(&cid)
                && let Some(slot) = c.kind.sensor_mut()
            {
                *slot = sid.clone();
            }
            s.open = None;
        })
        .build(k, layer);
    row()
        .w_full()
        .items_center()
        .justify_between()
        .child(text("Sensor").font(k.ty.ui).color(k.c.text_muted))
        .child(select)
}

fn mix_editor(s: &Ui, k: &Kit, id: &str, parts: &[String], op: MixOp, mut ed: El) -> El {
    let labels = ["Max", "Min", "Avg", "Sum"];
    let selected = MixOp::ALL.iter().position(|o| *o == op);
    let cid = id.to_string();
    ed = ed.child(
        row().w_full().items_center().justify_between().child(text("Combine with").font(k.ty.ui).color(k.c.text_muted)).child(
            SegmentedControl::new(&labels, selected)
                .label("Mix function")
                .on_pick(move |s: &mut Ui, i: usize, _: &mut Tx| {
                    if let Some(CurveKind::Mix { op, .. }) = s.draft.curve_mut(&cid).map(|c| &mut c.kind) {
                        *op = MixOp::ALL[i];
                    }
                })
                .build(k),
        ),
    );
    ed = ed.child(text("Curves").font(SECTION).color(k.c.text_base));
    let others: Vec<&Curve> = s.draft.curves.iter().filter(|c| c.id != id).collect();
    if others.is_empty() {
        ed = ed.child(text("Add other curves to mix them here.").font(k.ty.small).color(k.c.text_faint));
    }
    for o in others {
        let on = parts.contains(&o.id);
        let (cid, oid) = (id.to_string(), o.id.clone());
        ed = ed.child(switch_row(k, &o.name, on, move |s: &mut Ui| {
            if let Some(CurveKind::Mix { curves, .. }) = s.draft.curve_mut(&cid).map(|c| &mut c.kind) {
                if let Some(i) = curves.iter().position(|x| *x == oid) {
                    curves.remove(i);
                } else {
                    curves.push(oid.clone());
                }
            }
        }));
    }
    ed
}

// ───────────────────────────── Sensors ─────────────────────────────

fn sensors_page(s: &Ui, k: &Kit) -> El {
    let section = |kind: SensorKind, title: &str| {
        let mut c = card(k).w_full().max_w(720.0).gap(0.0).child(text(title).font(SECTION).color(k.c.text_base).pb(8.0));
        let rows: Vec<_> = s.snapshot.sensors.iter().filter(|x| x.kind == kind).collect();
        if rows.is_empty() {
            c = c.child(text("None found").font(k.ty.small).color(k.c.text_faint));
        }
        for x in rows {
            c = c.child(
                row()
                    .key(format!("sensor.{}", key_safe(&x.id)))
                    .w_full()
                    .py(7.0)
                    .gap(12.0)
                    .items_center()
                    .border_sides([1.0, 0.0, 0.0, 0.0], k.c.border_muted)
                    .child(
                        col()
                            .grow(1.0)
                            .min_w(0.0)
                            .child(if s.renaming.as_ref() == Some(&RenameTarget::Label(x.id.clone())) {
                                rename_field(s, k)
                            } else {
                                row()
                                    .gap(4.0)
                                    .items_center()
                                    .min_w(0.0)
                                    .child(text(s.label(&x.id)).font(k.ty.ui).color(k.c.text_base).truncate().shrink())
                                    .child(rename_button(k, format!("rename.{}", key_safe(&x.id)), RenameTarget::Label(x.id.clone())))
                            })
                            .child(text(&x.id).font(k.ty.small).color(k.c.text_faint).truncate()),
                    )
                    .child(
                        text(x.value.map_or("—".into(), |v| format!("{} {}", fmt(v), kind.unit())))
                            .font(k.ty.ui_medium.features("'tnum' 1"))
                            .color(k.c.text_base),
                    ),
            );
        }
        c
    };
    col().w_full().gap(16.0).child(section(SensorKind::Temperature, "Temperatures")).child(section(SensorKind::Fan, "Fans"))
}

// ───────────────────────────── Renaming ─────────────────────────────

fn rename_button(k: &Kit, key: String, target: RenameTarget) -> El {
    Button::new("Rename")
        .variant(ButtonVariant::Ghost)
        .control_size(ControlSize::Small)
        .on_press(move |s: &mut Ui| s.start_rename(target.clone()))
        .build(k)
        .key(key)
}

/// The inline editor for whatever `s.renaming` names. Enter saves; an empty label resets it.
fn rename_field(s: &Ui, k: &Kit) -> El {
    row()
        .gap(6.0)
        .items_center()
        .child(
            TextInput::new(&s.rename_edit)
                .width(Some(180.0))
                .placeholder("Name (empty resets)")
                .on_edit(|s: &mut Ui| &mut s.rename_edit)
                .on_submit(|s: &mut Ui, _: &mut Tx| s.finish_rename())
                .build(k)
                .key("rename-input"),
        )
        .child(
            Button::new("Save")
                .variant(ButtonVariant::Contrast)
                .control_size(ControlSize::Small)
                .on_press(|s: &mut Ui| s.finish_rename())
                .build(k)
                .key("rename-save"),
        )
        .child(
            Button::new("Cancel")
                .variant(ButtonVariant::Ghost)
                .control_size(ControlSize::Small)
                .on_press(|s: &mut Ui| s.cancel_rename())
                .build(k),
        )
}

// ───────────────────────────── Calibration ─────────────────────────────

fn progress_bar(k: &Kit, fraction: f32) -> El {
    el().w_full().h(6.0).radius(3.0).fill(k.c.layer_03).child(
        el().h_full().w_frac(fraction.clamp(0.02, 1.0)).radius(3.0).fill(accent(k)),
    )
}

fn calibration_section(s: &Ui, k: &Kit, id: &str) -> El {
    if let Some(c) = s.snapshot.controls.iter().find(|c| c.id == id && !c.calibration_supported) {
        return col().w_full().gap(4.0).padding(12.0).radius(8.0).fill(k.c.layer_02)
            .child(text(format!("Driver range: {:.0}–{:.0} %", c.min_percent, c.max_percent)).font(k.ty.small).color(k.c.text_muted))
            .child(text("Uses NVIDIA's fan limits. At 85 °C or if GPU temperature is unavailable, fans run at full speed.").font(k.ty.small).color(k.c.text_faint).wrap());
    }
    let mut sec = col()
        .key(format!("calibration.{}", key_safe(id)))
        .w_full()
        .gap(8.0)
        .padding(12.0)
        .radius(8.0)
        .fill(k.c.layer_02)
        .child(text("Calibration").font(SECTION).color(k.c.text_base));

    if let Some(p) = s.snapshot.calibrating.get(id) {
        let cancel = id.to_string();
        return sec
            .child(
                row()
                    .w_full()
                    .justify_between()
                    .items_center()
                    .child(text(format!("{} · {:.0}%", p.phase, p.duty)).font(k.ty.ui).color(k.c.text_muted))
                    .child(
                        Button::new("Cancel")
                            .variant(ButtonVariant::Ghost)
                            .control_size(ControlSize::Small)
                            .on_press(move |s: &mut Ui| s.cancel_calibration(&cancel))
                            .build(k),
                    ),
            )
            .child(progress_bar(k, p.fraction));
    }

    let busy = !s.snapshot.calibrating.is_empty();
    let start = id.to_string();
    let run = Button::new(if s.config.calibrations.contains_key(id) { "Recalibrate" } else { "Calibrate" })
        .variant(ButtonVariant::Outline)
        .control_size(ControlSize::Small)
        .disabled(busy || s.conn != Conn::Connected)
        .on_press(move |s: &mut Ui| s.calibrate(&start))
        .build(k)
        .key(format!("calibrate.{}", key_safe(id)));

    if let Some(e) = s.snapshot.calibration_errors.get(id) {
        sec = sec.child(text(format!("Last run: {e}")).font(k.ty.small).color(BAD).wrap());
    }

    match s.config.calibrations.get(id) {
        Some(c) => {
            let fan = c.fan.as_deref().map_or("no tachometer answered".to_string(), |f| s.label(f));
            let facts = [
                format!("Fan: {fan}"),
                c.max_rpm.map_or(String::new(), |r| format!("{r:.0} RPM at full speed")),
                match c.stop_percent {
                    Some(p) if p <= 0.0 => "Never stops, even at 0 %".to_string(),
                    Some(p) => format!("Keeps spinning down to {p:.0} %"),
                    None => String::new(),
                },
                match c.start_percent {
                    Some(p) if p > 0.0 => format!("Starts from standstill at {p:.0} %"),
                    _ => String::new(),
                },
            ];
            for f in facts.iter().filter(|f| !f.is_empty()) {
                sec = sec.child(text(f).font(k.ty.small).color(k.c.text_muted));
            }
            let (min, kick) = c.suggested();
            let cfg = s.draft.control(id);
            let applied = cfg.is_some_and(|x| x.min_percent == min && x.start_percent == kick);
            let use_id = id.to_string();
            let mut buttons = row().gap(8.0).items_center().child(run);
            if c.fan.is_some() && !applied {
                buttons = buttons.child(
                    Button::new(format!("Use min {min:.0} % · kick {kick:.0} %"))
                        .variant(ButtonVariant::Contrast)
                        .control_size(ControlSize::Small)
                        .on_press(move |s: &mut Ui| {
                            let c = s.draft.control_mut(&use_id);
                            c.min_percent = min;
                            c.start_percent = kick;
                        })
                        .build(k)
                        .key(format!("use-calibration.{}", key_safe(id))),
                );
            }
            sec.child(buttons)
        }
        None => sec
            .child(
                text("Finds which fan this header drives, its top speed, and the slowest speeds it can hold and start from. About two minutes; the fan runs at full speed, then stops briefly.")
                    .font(k.ty.small)
                    .color(k.c.text_faint)
                    .wrap(),
            )
            .child(row().child(run)),
    }
}

// ───────────────────────────── Settings ─────────────────────────────

fn settings_page(s: &Ui, k: &Kit) -> El {
    let labels: Vec<&str> = ThemeChoice::ALL.iter().map(|t| t.label()).collect();
    let selected = ThemeChoice::ALL.iter().position(|t| *t == s.prefs.theme);
    let note = match (&s.omarchy, s.prefs.theme) {
        (Some(t), ThemeChoice::FollowOmarchy) => format!("Using {} · updates when you switch Omarchy themes", pretty(&t.name)),
        (Some(t), _) => format!("Omarchy theme available: {}", pretty(&t.name)),
        (None, ThemeChoice::FollowOmarchy) => "No Omarchy theme found; showing Dark".to_string(),
        (None, _) => "Omarchy not detected".to_string(),
    };
    let look = s.look();
    let swatch = |c: Color| el().size(22.0, 22.0).radius(6.0).fill(c).border(1.0, k.c.border_base);
    let theme_card = card(k)
        .key("settings-theme")
        .w_full()
        .max_w(620.0)
        .child(text("Theme").font(SECTION).color(k.c.text_base))
        .child(
            SegmentedControl::new(&labels, selected)
                .label("Theme")
                .width(Some(300.0))
                .on_pick(|s: &mut Ui, i: usize, _: &mut Tx| s.set_theme(ThemeChoice::ALL[i]))
                .build(k)
                .key("theme-picker"),
        )
        .child(text(note).font(k.ty.small).color(k.c.text_muted))
        .child(
            row()
                .gap(8.0)
                .items_center()
                .child(swatch(k.c.bg_base))
                .child(swatch(k.c.layer_02))
                .child(swatch(k.c.text_base))
                .child(swatch(look.accent))
                .child(swatch(look.warm)),
        );

    let about = card(k)
        .w_full()
        .max_w(620.0)
        .gap(6.0)
        .child(text("About").font(SECTION).color(k.c.text_base))
        .child(text(format!("airbend {}", env!("CARGO_PKG_VERSION"))).font(k.ty.ui).color(k.c.text_muted))
        .child(
            text(match &s.conn {
                Conn::Connected => format!("Daemon connected · {} backend · config revision {}", s.snapshot.backend, s.revision),
                Conn::Connecting => "Connecting to the daemon…".into(),
                Conn::Failed(e) => format!("Daemon unreachable: {e}"),
            })
            .font(k.ty.small)
            .color(k.c.text_faint)
            .wrap(),
        )
        .child(
            text(format!("Socket {}", airbend_ipc::default_socket().display()))
                .font(k.ty.small)
                .color(k.c.text_faint),
        );
    col().w_full().gap(16.0).child(theme_card).child(about)
}

/// "last-horizon" → "Last Horizon".
fn pretty(slug: &str) -> String {
    slug.split(['-', '_', ' '])
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            c.next().map_or(String::new(), |f| f.to_uppercase().chain(c).collect())
        })
        .collect::<Vec<_>>()
        .join(" ")
}
