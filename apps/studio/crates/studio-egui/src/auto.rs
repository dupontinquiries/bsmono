//! auto — the automation strip drawn under the timeline.
//!
//! `studio-core` grew lanes, ops and a per-sample render pass, and nothing on
//! screen could reach any of it. This is the view: one target's lane as a
//! polyline whose points you place, drag and remove. Same contract as `grid`
//! and `roll` — **nothing here owns the lane**, every interaction returns an
//! [`AutoEdit`] for the host to turn into `SetAutoPoint`/`RemoveAutoPoint`
//! ops, because an edit with no representation is invisible to a
//! collaborator and to undo alike.
//!
//! ## Alignment is by construction, not by care
//!
//! The strip maps time through the SAME [`View`](crate::View) the timeline
//! uses — `time_to_x`/`x_to_time`, gutter included — so a point over a
//! clip's edge stays over that edge at every zoom and scroll. A second
//! mapping that merely agreed today would drift the first time one of them
//! changed; sharing the functions makes misalignment unrepresentable.
//!
//! ## Commit on release, ghost until then
//!
//! A drag previews locally and lands as ops when the pointer lifts — the
//! fader rule, one panel over. A point dragged only vertically is ONE op
//! (`Set` restates its value at its own tick); dragged in time it is two
//! (`Remove` the old tick, `Set` the new), two undo steps, the precedent a
//! left trim set: two true facts, two ops.

use studio_core::Ticks;
use crate::View;

/// Strip height. Short on purpose: the strip is a ride under the
/// arrangement, not a second arrangement.
pub const AUTO_H: f32 = 72.0;
/// The top of the strip's value axis. Unity sits at half height with the
/// same headroom above it a boosting ride needs; the render already clamps
/// negatives, so the bottom is honest zero.
pub const MAX_VALUE: f32 = 2.0;
/// Grab radius around a point, in pixels — finger-sized targets are a
/// stated goal of this repo's phone builds.
pub const GRAB: f32 = 8.0;

/// What the strip asks the host to do. Target-free: the host chose which
/// lane it handed in, and keeps that decision.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AutoEdit {
    Set { at: Ticks, value: f32 },
    Remove { at: Ticks },
}

/// A point mid-drag: where it was grabbed, and where it currently hovers.
/// The ghost is drawn from this; the lane itself never moves until release.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PointDrag {
    pub orig_at: Ticks,
    pub at: Ticks,
    pub value: f32,
}

/// value → y within a strip spanning `top..top+h`.
pub fn value_to_y(v: f32, top: f32, h: f32) -> f32 {
    top + (1.0 - (v.clamp(0.0, MAX_VALUE) / MAX_VALUE)) * h
}

/// y → value, clamped into the strip's honest range — a pointer above the
/// strip means "as loud as the lane goes", below it means silence, never a
/// negative that would reach the mix as a phase flip.
pub fn y_to_value(y: f32, top: f32, h: f32) -> f32 {
    ((1.0 - (y - top) / h.max(1.0)) * MAX_VALUE).clamp(0.0, MAX_VALUE)
}

/// The index of the point under `pos`, if any — nearest within [`GRAB`],
/// so two close points resolve to the closer one instead of first-wins.
pub fn point_hit(pos: egui::Pos2, points: &[(Ticks, f32)], view: &View,
                 origin_x: f32, top: f32, h: f32) -> Option<usize> {
    let mut best: Option<(usize, f32)> = None;
    for (i, (t, v)) in points.iter().enumerate() {
        let p = egui::pos2(view.time_to_x(*t, origin_x), value_to_y(*v, top, h));
        let d = p.distance(pos);
        if d <= GRAB && best.map(|(_, bd)| d < bd).unwrap_or(true) {
            best = Some((i, d));
        }
    }
    best.map(|(i, _)| i)
}

/// The ops a finished drag means. Pure so the two-vs-one decision — the
/// part a UI test cannot reach — is pinned headlessly.
pub fn commit(drag: &PointDrag) -> Vec<AutoEdit> {
    if drag.at == drag.orig_at {
        vec![AutoEdit::Set { at: drag.at, value: drag.value }]
    } else {
        vec![AutoEdit::Remove { at: drag.orig_at },
             AutoEdit::Set { at: drag.at, value: drag.value }]
    }
}

/// Draw one lane's strip. Returns edits for the host to apply.
///
/// `points` must be sorted by time (the model keeps them that way) and
/// be in TIMELINE time — a host showing a gated curve shifts them to the
/// gate's origin first and shifts edits back. `gate` is that gate's span:
/// shaded, and the curve's flat holds stop at its edges because outside
/// the gate the binding reads unity, and the strip must draw what the mix
/// does. `None` is a plain lane. `playhead` draws the same transport line
/// the timeline shows so the two stay one instrument.
pub fn strip(ui: &mut egui::Ui, points: &[(Ticks, f32)], view: &View,
             drag: &mut Option<PointDrag>, playhead: Option<Ticks>,
             c: &uikit_egui::PaletteColors, gate: Option<(Ticks, Ticks)>) -> Vec<AutoEdit> {
    let mut edits = Vec::new();
    let width = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, AUTO_H),
                                              egui::Sense::click_and_drag());
    let painter = ui.painter();
    let (top, h) = (rect.top(), rect.height());
    let lane_left = rect.left() + view.gutter;

    painter.rect_filled(rect, 0.0, c.bg_base);
    // The gate: where the binding is live. Shaded so a curve that sits
    // under one clip reads as that clip's, and so the strip is not mistaken
    // for a lane that spans the song.
    let (hold_left, hold_right) = match gate {
        Some((s, e)) => {
            let (x0, x1) = (view.time_to_x(s, rect.left()).max(lane_left),
                            view.time_to_x(e, rect.left()).max(lane_left));
            if x1 > x0 {
                painter.rect_filled(egui::Rect::from_min_max(egui::pos2(x0, top), egui::pos2(x1, top + h)),
                                    0.0, c.accent.gamma_multiply(0.12));
            }
            (x0, x1.min(rect.right()))
        }
        None => (lane_left, rect.right()),
    };
    // The unity line: the value at which the ride is transparent. Without
    // it every lane reads as "somewhere", and the one thing a ride needs
    // to say at a glance is "above or below what the fader already does".
    let unity_y = value_to_y(1.0, top, h);
    painter.line_segment([egui::pos2(lane_left, unity_y), egui::pos2(rect.right(), unity_y)],
                         egui::Stroke::new(1.0_f32, c.text_weak.gamma_multiply(0.5)));
    painter.text(egui::pos2(rect.left() + 4.0, unity_y),
                 egui::Align2::LEFT_CENTER, "1.0",
                 egui::FontId::proportional(10.0), c.text_weak);

    // The lane, as the render evaluates it: held flat to the edges, linear
    // between points — drawn from the same shape `eval_lane` walks, so
    // what you see is what the mix does.
    let shown: Vec<(Ticks, f32)> = match *drag {
        // The ghost replaces its original while dragging; sorting keeps the
        // polyline honest when a point is dragged past its neighbour.
        Some(d) => {
            let mut v: Vec<(Ticks, f32)> = points.iter().copied()
                .filter(|(t, _)| *t != d.orig_at).collect();
            v.push((d.at, d.value));
            v.sort_by_key(|(t, _)| *t);
            v
        }
        None => points.to_vec(),
    };
    if let (Some(first), Some(last)) = (shown.first(), shown.last()) {
        let y0 = value_to_y(first.1, top, h);
        painter.line_segment([egui::pos2(hold_left, y0),
                              egui::pos2(view.time_to_x(first.0, rect.left()).max(hold_left), y0)],
                             egui::Stroke::new(1.5_f32, c.accent));
        for w in shown.windows(2) {
            painter.line_segment(
                [egui::pos2(view.time_to_x(w[0].0, rect.left()), value_to_y(w[0].1, top, h)),
                 egui::pos2(view.time_to_x(w[1].0, rect.left()), value_to_y(w[1].1, top, h))],
                egui::Stroke::new(1.5_f32, c.accent));
        }
        let yn = value_to_y(last.1, top, h);
        painter.line_segment([egui::pos2(view.time_to_x(last.0, rect.left()).min(hold_right), yn),
                              egui::pos2(hold_right, yn)],
                             egui::Stroke::new(1.5_f32, c.accent));
    } else {
        // No points: the lane is a transparent ride at unity. Saying so
        // beats an empty strip that reads as broken.
        painter.text(rect.center(), egui::Align2::CENTER_CENTER,
                     if gate.is_some() { "click to add a point — the curve rides its clip" }
                     else { "click to add a point — the lane rides the fader" },
                     egui::FontId::proportional(11.0), c.text_weak);
    }
    for (t, v) in &shown {
        let p = egui::pos2(view.time_to_x(*t, rect.left()), value_to_y(*v, top, h));
        if p.x < lane_left { continue; }
        painter.circle_filled(p, 3.5, c.accent);
    }
    if let Some(ph) = playhead {
        let x = view.time_to_x(ph, rect.left());
        if x >= lane_left {
            painter.line_segment([egui::pos2(x, top), egui::pos2(x, top + h)],
                                 egui::Stroke::new(1.5_f32, c.warning));
        }
    }

    // ── interaction ─────────────────────────────────────────────────────
    if let Some(pos) = resp.interact_pointer_pos() {
        if resp.drag_started() {
            if let Some(i) = point_hit(pos, points, view, rect.left(), top, h) {
                let (t, v) = points[i];
                *drag = Some(PointDrag { orig_at: t, at: t, value: v });
            }
        }
        if let Some(d) = drag {
            d.at = Ticks(view.x_to_time(pos.x, rect.left()).0.max(0));
            d.value = y_to_value(pos.y, top, h);
        } else if resp.clicked() && pos.x >= lane_left {
            let at = Ticks(view.x_to_time(pos.x, rect.left()).0.max(0));
            edits.push(AutoEdit::Set { at, value: y_to_value(pos.y, top, h) });
        }
        if resp.secondary_clicked() {
            if let Some(i) = point_hit(pos, points, view, rect.left(), top, h) {
                edits.push(AutoEdit::Remove { at: points[i].0 });
            }
        }
    }
    if resp.drag_stopped() {
        if let Some(d) = drag.take() {
            edits.extend(commit(&d));
        }
    } else if !resp.dragged() && !resp.is_pointer_button_down_on() {
        *drag = None;
    }
    edits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_and_y_are_inverses_and_clamp_at_the_strips_edges() {
        let (top, h) = (100.0, AUTO_H);
        for v in [0.0, 0.5, 1.0, 1.7, MAX_VALUE] {
            let y = value_to_y(v, top, h);
            assert!((y_to_value(y, top, h) - v).abs() < 1e-4, "v={v}");
        }
        assert_eq!(y_to_value(top - 50.0, top, h), MAX_VALUE, "above the strip is the ceiling");
        assert_eq!(y_to_value(top + h + 50.0, top, h), 0.0,
                   "below it is silence, never a negative phase flip");
        assert_eq!(value_to_y(1.0, top, h), top + h * 0.5, "unity sits at half height");
    }

    #[test]
    fn a_hit_finds_the_nearest_point_within_the_grab_radius() {
        let view = View::default();
        let points = vec![(Ticks::ZERO, 1.0), (Ticks::from_seconds(0.1), 1.0)];
        let (top, h) = (0.0, AUTO_H);
        // 0.1s at the default 100 px/sec is 10 px apart — both within GRAB
        // of a pointer between them, so nearest-wins matters here.
        let x0 = view.time_to_x(Ticks::ZERO, 0.0);
        let y = value_to_y(1.0, top, h);
        assert_eq!(point_hit(egui::pos2(x0 + 3.0, y), &points, &view, 0.0, top, h), Some(0));
        assert_eq!(point_hit(egui::pos2(x0 + 7.0, y), &points, &view, 0.0, top, h), Some(1));
        assert_eq!(point_hit(egui::pos2(x0 + 300.0, y), &points, &view, 0.0, top, h), None);
        // Vertical distance counts too: same x, far value, no grab.
        assert_eq!(point_hit(egui::pos2(x0, y - 40.0), &points, &view, 0.0, top, h), None);
    }

    #[test]
    fn a_vertical_drag_is_one_op_and_a_time_move_is_two() {
        // The left-trim precedent: a move in time is two true facts. A
        // value change at the point's own tick is one.
        let held = PointDrag { orig_at: Ticks(100), at: Ticks(100), value: 0.3 };
        assert_eq!(commit(&held), vec![AutoEdit::Set { at: Ticks(100), value: 0.3 }]);
        let moved = PointDrag { orig_at: Ticks(100), at: Ticks(400), value: 0.3 };
        assert_eq!(commit(&moved), vec![
            AutoEdit::Remove { at: Ticks(100) },
            AutoEdit::Set { at: Ticks(400), value: 0.3 },
        ]);
    }

    #[test]
    fn the_strip_shares_the_timelines_time_mapping() {
        // Alignment by construction: the strip has no mapping of its own to
        // drift. This pins the SHAPE of that promise — the round trip
        // through the shared View at a non-default zoom and scroll.
        let mut view = View::default();
        view.px_per_sec = 250.0;
        view.scroll = Ticks::from_seconds(3.0);
        let t = Ticks::from_seconds(4.5);
        let x = view.time_to_x(t, 17.0);
        let back = view.x_to_time(x, 17.0);
        assert!((back.0 - t.0).abs() < 1_000, "{back:?} vs {t:?}");
    }
}
