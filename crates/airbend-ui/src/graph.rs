//! The graph curve editor: drag points to move them, press empty space to add one, double-click
//! a point to remove it. Points stay sorted: a point can't pass its neighbours.

use airbend_core::{CurveKind, Point};
use rune::{Color, Drag, El, LineCap, Phase, Shape, TextStyle, Tx, canvas, el, text};
use rune_kit::Kit;

use crate::Ui;

pub const W: f32 = 520.0;
pub const H: f32 = 300.0;
const L: f32 = 40.0;
const R: f32 = 14.0;
const T: f32 = 12.0;
const B: f32 = 26.0;
pub const T_MIN: f32 = 20.0;
pub const T_MAX: f32 = 100.0;
const GRAB: f32 = 14.0;

fn x_of(temp: f32) -> f32 {
    L + (temp.clamp(T_MIN, T_MAX) - T_MIN) / (T_MAX - T_MIN) * (W - L - R)
}

fn y_of(pct: f32) -> f32 {
    T + (1.0 - pct.clamp(0.0, 100.0) / 100.0) * (H - T - B)
}

fn temp_of(x: f32) -> f32 {
    T_MIN + (x - L) / (W - L - R) * (T_MAX - T_MIN)
}

fn pct_of(y: f32) -> f32 {
    (1.0 - (y - T) / (H - T - B)) * 100.0
}

fn with_alpha(c: Color, a: f32) -> Color {
    Color { a, ..c }
}

/// The graph's points, when `id` is a graph curve in the draft.
fn points_mut<'a>(s: &'a mut Ui, id: &str) -> Option<&'a mut Vec<Point>> {
    match s.draft.curve_mut(id).map(|c| &mut c.kind) {
        Some(CurveKind::Graph { points, .. }) => Some(points),
        _ => None,
    }
}

fn nearest(points: &[Point], x: f32, y: f32) -> Option<usize> {
    points
        .iter()
        .enumerate()
        .map(|(i, p)| (i, (x_of(p.temp) - x).hypot(y_of(p.percent) - y)))
        .filter(|&(_, d)| d <= GRAB)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

/// Moves point `i` to (x, y), kept between its neighbours and inside the chart.
fn move_point(points: &mut [Point], i: usize, x: f32, y: f32) {
    let lo = if i > 0 { points[i - 1].temp + 1.0 } else { T_MIN };
    let hi = if i + 1 < points.len() { points[i + 1].temp - 1.0 } else { T_MAX };
    let temp = (temp_of(x).round()).clamp(lo, hi.max(lo));
    let pct = pct_of(y).round().clamp(0.0, 100.0);
    points[i] = Point::new(temp, pct);
}

fn on_drag(id: String) -> impl Fn(&mut Ui, &mut Drag, &mut Tx) + 'static {
    move |s: &mut Ui, d: &mut Drag, _tx: &mut Tx| {
        let dragging = s.drag_point;
        let Some(points) = points_mut(s, &id) else { return };
        match d.phase {
            Phase::Down => {
                let hit = nearest(points, d.x, d.y);
                let next = match hit {
                    Some(i) if d.clicks >= 2 && points.len() > 1 => {
                        points.remove(i);
                        None
                    }
                    Some(i) => Some(i),
                    None => {
                        let temp = temp_of(d.x).round().clamp(T_MIN, T_MAX);
                        if points.iter().any(|p| (p.temp - temp).abs() < 1.0) {
                            None
                        } else {
                            let at = points.iter().position(|p| p.temp > temp).unwrap_or(points.len());
                            points.insert(at, Point::new(temp, pct_of(d.y).round().clamp(0.0, 100.0)));
                            Some(at)
                        }
                    }
                };
                s.drag_point = next;
            }
            Phase::Move => {
                if let Some(i) = dragging.filter(|&i| i < points.len()) {
                    move_point(points, i, d.x, d.y);
                }
            }
            Phase::Up => s.drag_point = None,
        }
    }
}

pub fn editor(
    k: &Kit,
    id: &str,
    points: &[Point],
    current: Option<(f32, f32)>,
    accent: Color,
    warm: Color,
    dragging: Option<usize>,
) -> El {
    let grid = with_alpha(k.c.text_base, 0.08);
    let axis = with_alpha(k.c.text_base, 0.25);

    let mut shape = Shape::new();
    let mut t = T_MIN;
    while t <= T_MAX + 0.1 {
        shape = shape.line([x_of(t), T], [x_of(t), H - B], 1.0, LineCap::Butt, grid);
        t += 10.0;
    }
    for p in (0..=100).step_by(20) {
        shape = shape.line([L, y_of(p as f32)], [W - R, y_of(p as f32)], 1.0, LineCap::Butt, grid);
    }
    shape = shape
        .line([L, T], [L, H - B], 1.0, LineCap::Butt, axis)
        .line([L, H - B], [W - R, H - B], 1.0, LineCap::Butt, axis);

    if let (Some(first), Some(last)) = (points.first(), points.last()) {
        let mut line = vec![[L, y_of(first.percent)]];
        line.extend(points.iter().map(|p| [x_of(p.temp), y_of(p.percent)]));
        line.push([W - R, y_of(last.percent)]);
        let mut area = line.clone();
        area.push([W - R, H - B]);
        area.push([L, H - B]);
        shape = shape.fill(area, with_alpha(accent, 0.14)).stroke(line, 2.0, accent);
    }
    if let Some((temp, pct)) = current {
        let (x, y) = (x_of(temp), y_of(pct));
        shape = shape
            .line([x, T], [x, H - B], 1.0, LineCap::Butt, with_alpha(warm, 0.7))
            .line([L, y], [W - R, y], 1.0, LineCap::Butt, with_alpha(warm, 0.35));
    }

    let small = TextStyle::new(10.0, 400.0);
    let mut root = el().key("graph").size(W, H).child(canvas(shape).absolute(0.0, 0.0).size(W, H));
    let mut t = T_MIN;
    while t <= T_MAX + 0.1 {
        root = root.child(
            text(format!("{t:.0}°")).font(small).color(k.c.text_faint).absolute(x_of(t) - 10.0, H - B + 6.0),
        );
        t += 10.0;
    }
    for p in (0..=100).step_by(20) {
        root = root.child(text(format!("{p}%")).font(small).color(k.c.text_faint).absolute(4.0, y_of(p as f32) - 7.0));
    }
    if let Some((temp, pct)) = current {
        root = root.child(
            el().absolute(x_of(temp) - 5.0, y_of(pct) - 5.0).size(10.0, 10.0).radius(5.0).fill(warm),
        );
    }
    for (i, p) in points.iter().enumerate() {
        let r = if dragging == Some(i) { 8.0 } else { 6.0 };
        root = root.child(
            el().key(format!("pt{i}"))
                .absolute(x_of(p.temp) - r, y_of(p.percent) - r)
                .size(r * 2.0, r * 2.0)
                .radius(r)
                .fill(accent)
                .border(2.0, k.c.bg_base),
        );
    }
    if let Some(i) = dragging.filter(|&i| i < points.len()) {
        let p = points[i];
        root = root.child(
            text(format!("{:.0} °C → {:.0} %", p.temp, p.percent))
                .font(small)
                .color(k.c.text_base)
                .absolute((x_of(p.temp) + 10.0).min(W - 100.0), (y_of(p.percent) - 24.0).max(0.0)),
        );
    }
    root.cursor(rune::Cursor::Pointer).on_drag(on_drag(id.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coordinates_round_trip() {
        for (t, p) in [(20.0, 0.0), (55.0, 40.0), (100.0, 100.0)] {
            assert!((temp_of(x_of(t)) - t).abs() < 0.01);
            assert!((pct_of(y_of(p)) - p).abs() < 0.01);
        }
    }

    #[test]
    fn a_point_cannot_pass_its_neighbours() {
        let mut pts = vec![Point::new(30.0, 20.0), Point::new(50.0, 40.0), Point::new(70.0, 80.0)];
        move_point(&mut pts, 1, x_of(90.0), y_of(50.0));
        assert_eq!(pts[1].temp, 69.0);
        move_point(&mut pts, 1, x_of(0.0), y_of(150.0));
        assert_eq!(pts[1], Point::new(31.0, 100.0));
    }
}
