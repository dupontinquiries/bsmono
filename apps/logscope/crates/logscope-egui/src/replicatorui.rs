//! replicatorui.rs — the two controls replicator mode needs and egui does not have.
//!
//! Both are drawn rather than composed from existing widgets, for the same reason: what the
//! operator needs to see is a LIMIT, and egui's slider expresses a limit by not existing past it.
//!
//! ## The detented track
//!
//! ```text
//!   -X-X-X-X-X-X-
//!          ^ 24 GiB fits 3
//! ```
//!
//! The track always shows every position the machines could host (4 per machine). The tick marks
//! where memory stops keeping the promise; positions past it are drawn greyed and refuse the
//! click. A shrinking maximum would hide the same information: the operator would see a shorter
//! slider and learn nothing about why, whereas greyed positions are visibly something a smaller
//! model would unlock.
//!
//! ## The dilation matrix
//!
//! One row per worker, one thumbtack per column: how often that row's worker sees that column
//! worker's latest message. Row and column are different directions and can differ — an
//! implementer may want its reviewer every turn while the reviewer wants a digest every third.
//! The diagonal is inert (a worker rereading its own last message is noise), and the values are
//! ticks rather than a text field because a dilation of 97 is not a setting anyone means.

use egui::{Color32, Rect, Response, Sense, Stroke, Vec2, pos2, vec2};
use logscope::replicator::{DILATIONS, ShareGrid, Slots};

/// Height of the slider strip, in points. Tall enough that the 44pt touch rule is met by the
/// clickable band without the strip itself dominating a phone's toolbar.
const TRACK_H: f32 = 44.0;

/// Draw the slot slider. Returns the (possibly changed) value.
///
/// `value` is clamped to the detent, never to the track: clicking a greyed position leaves the
/// value alone rather than snapping to the nearest legal one, because a control that moves
/// somewhere you did not click reads as a bug even when the destination is sensible.
pub fn slot_slider(ui: &mut egui::Ui, slots: &Slots, value: usize) -> usize {
    let n = slots.track_max.max(1);
    let usable = slots.selectable_max();
    let (rect, resp) = ui.allocate_exact_size(
        vec2(ui.available_width().min(20.0 + n as f32 * 34.0), TRACK_H), Sense::click_and_drag());
    let p = ui.painter();
    let inset = 14.0;
    let y = rect.center().y - 4.0;
    let x_of = |i: usize| {
        let span = (rect.width() - 2.0 * inset).max(1.0);
        rect.left() + inset + span * (i as f32) / ((n - 1).max(1) as f32)
    };

    // the rail
    p.line_segment([pos2(x_of(0), y), pos2(x_of(n - 1), y)],
                   Stroke::new(1.0, crate::theme::DIM().gamma_multiply(0.5)));

    let mut out = value.clamp(slots.min, usable);
    for i in 0..n {
        let slot = i + 1;
        let reachable = slot <= usable;
        let x = x_of(i);
        let col = if !reachable { crate::theme::DIM().gamma_multiply(0.35) }
                  else if slot <= out { crate::theme::accent() }
                  else { crate::theme::DIM() };
        // X marks, per the operator's sketch: the position is a thing you land ON, not a
        // continuum you slide through — the count is discrete and the control should say so.
        let r = 4.0;
        p.line_segment([pos2(x - r, y - r), pos2(x + r, y + r)], Stroke::new(1.6, col));
        p.line_segment([pos2(x - r, y + r), pos2(x + r, y - r)], Stroke::new(1.6, col));
    }

    // the detent: a caret under the last reachable position, with the reason beside it
    if usable < n {
        let x = x_of(usable.saturating_sub(1));
        let top = y + 9.0;
        p.add(egui::Shape::convex_polygon(
            vec![pos2(x, top), pos2(x - 4.0, top + 7.0), pos2(x + 4.0, top + 7.0)],
            crate::theme::accent().gamma_multiply(0.8), Stroke::NONE));
        p.text(pos2(x + 8.0, top + 1.0), egui::Align2::LEFT_TOP,
               format!("{usable} fit"), egui::FontId::proportional(10.0), crate::theme::DIM());
    }

    if resp.clicked() || resp.dragged() {
        if let Some(pos) = resp.interact_pointer_pos() {
            // nearest position by x, then honour the detent by REFUSING rather than snapping
            let mut best = (f32::MAX, out);
            for i in 0..n { let d = (x_of(i) - pos.x).abs(); if d < best.0 { best = (d, i + 1); } }
            if best.1 <= usable { out = best.1; }
        }
    }
    out
}

/// One cell of the matrix: the current dilation, drawn as a thumbtack on a tiny tick row.
fn dilation_cell(ui: &mut egui::Ui, cur: u8, inert: bool) -> (Response, Option<u8>) {
    let (rect, resp) = ui.allocate_exact_size(vec2(46.0, 22.0), Sense::click());
    let p = ui.painter();
    if inert {
        p.text(rect.center(), egui::Align2::CENTER_CENTER, "·",
               egui::FontId::proportional(12.0), crate::theme::DIM().gamma_multiply(0.4));
        return (resp, None);
    }
    let k = DILATIONS.len();
    let x_of = |i: usize| rect.left() + 5.0 + (rect.width() - 10.0) * i as f32 / (k - 1) as f32;
    p.line_segment([pos2(x_of(0), rect.center().y), pos2(x_of(k - 1), rect.center().y)],
                   Stroke::new(1.0, crate::theme::DIM().gamma_multiply(0.4)));
    for (i, d) in DILATIONS.iter().enumerate() {
        let on = *d == cur;
        let x = x_of(i);
        let col = if on { crate::theme::accent() }
                  else { crate::theme::DIM().gamma_multiply(0.55) };
        let h = if on { 6.0 } else { 3.0 };
        p.line_segment([pos2(x, rect.center().y - h), pos2(x, rect.center().y + h)], Stroke::new(if on { 2.0 } else { 1.0 }, col));
    }
    let mut picked = None;
    if resp.clicked() {
        if let Some(pos) = resp.interact_pointer_pos() {
            let mut best = (f32::MAX, cur);
            for (i, d) in DILATIONS.iter().enumerate() {
                let dx = (x_of(i) - pos.x).abs();
                if dx < best.0 { best = (dx, *d); }
            }
            picked = Some(best.1);
        }
    }
    (resp, picked)
}

/// Draw the whole matrix. `labels` is one short name per worker in id order, and `now_turn` is the
/// current turn — a dilation set here is ANCHORED to it, so the first share happens on the turn the
/// operator moved the thumbtack rather than whenever the next multiple of n came around.
///
/// Mutates the grid in place and returns true if anything changed, so the caller can persist or
/// re-render without diffing.
pub fn dilation_matrix(ui: &mut egui::Ui, grid: &mut ShareGrid, labels: &[String], now_turn: u32) -> bool {
    let mut changed = false;
    let n = labels.len();
    ui.label(egui::RichText::new("how often each worker sees the others' latest message")
        .color(crate::theme::DIM()).small());
    egui::Grid::new("replicator_dilation_matrix").spacing(vec2(6.0, 3.0)).show(ui, |ui| {
        ui.label("");
        for l in labels.iter() {
            ui.label(egui::RichText::new(label_head(l)).color(crate::theme::DIM()).small());
        }
        ui.end_row();
        for to in 0..n {
            ui.label(egui::RichText::new(label_head(&labels[to])).small());
            for from in 0..n {
                let (resp, picked) = dilation_cell(ui, grid.dilation(to, from), to == from);
                if let Some(v) = picked { grid.set_dilation(to, from, v, now_turn); changed = true; }
                let cur = grid.dilation(to, from);
                if to != from {
                    resp.on_hover_text(match cur {
                        0 => format!("{} never sees {}", label_head(&labels[to]), label_head(&labels[from])),
                        1 => format!("{} sees {} every turn", label_head(&labels[to]), label_head(&labels[from])),
                        k => format!("{} sees {} every {k} turns (from turn {})",
                                     label_head(&labels[to]), label_head(&labels[from]),
                                     grid.anchored_at(to, from)),
                    });
                }
            }
            ui.end_row();
        }
    });
    changed
}

/// One column's live state, as the panel needs it. Borrowed from the caller's own structures so
/// the panel owns nothing — it draws, reports clicks, and keeps no copy that could go stale.
pub struct ColumnView<'a> {
    pub id: usize,
    pub host: &'a str,
    pub assignment: &'a mut String,
    pub lines: &'a [String],
    pub turn: usize,
}

/// What the operator did in the panel this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct PanelAction {
    /// A column header was clicked: aim the composer here (None = broadcast).
    pub aim: Option<Option<usize>>,
    /// The slider moved.
    pub want: Option<usize>,
    pub toggle_matrix: bool,
}

/// Draw the replicator panel: the track, the columns, and the wires between them.
///
/// Crossing edges are drawn from the LOG, not from the grid: a configured link that has never
/// carried anything is a dashed hint, while an edge that actually moved words is drawn hot and
/// fades over the following turns. The difference matters because "why did this worker change
/// direction" is answered by what crossed, not by what was configured — and those come apart
/// exactly when a partner had nothing to say.
pub fn panel(ui: &mut egui::Ui, slots: &Slots, want: usize, aim: Option<usize>,
             cols: &mut [ColumnView<'_>], grid: &ShareGrid,
             log: &logscope::replicator::CrossingLog, model: &str) -> PanelAction {
    let mut act = PanelAction::default();

    ui.horizontal(|ui| {
        let v = slot_slider(ui, slots, want);
        if v != want { act.want = Some(v); }
        ui.vertical(|ui| {
            ui.label(egui::RichText::new(format!("{want} worker(s)")).strong());
            ui.label(egui::RichText::new(slots.explain(model)).color(crate::theme::DIM()).small());
        });
        if crate::theme::carved_button(ui, if aim.is_some() { "aim: one" } else { "aim: all" }).clicked() {
            act.aim = Some(None);   // clicking the badge always returns to broadcast
        }
        if crate::theme::carved_button(ui, "links").clicked() { act.toggle_matrix = true; }
    });

    // Column headers first, so their centres are known before the wires are drawn under them.
    let n = cols.len().max(1);
    let full = ui.available_width();
    let col_w = (full / n as f32).max(120.0);
    let wire_h = 18.0;
    let (wire_rect, _) = ui.allocate_exact_size(egui::vec2(full, wire_h), egui::Sense::hover());
    let centre = |i: usize| wire_rect.left() + col_w * (i as f32 + 0.5);

    // the wires
    let p = ui.painter();
    let newest_turn = cols.iter().map(|c| c.turn).max().unwrap_or(0);
    for to in 0..cols.len() {
        for from in 0..cols.len() {
            if to == from || grid.dilation(to, from) == 0 { continue; }
            let (x0, x1) = (centre(from), centre(to));
            let heat = log.last_on(from, to)
                .map(|c| {
                    let age = newest_turn.saturating_sub(c.turn);
                    (1.0 - age as f32 / 6.0).clamp(0.0, 1.0)
                }).unwrap_or(0.0);
            let y = wire_rect.center().y;
            if heat > 0.0 {
                // A live wire: bright, thicker with what it carried, drawn with a dot travelling
                // toward the RECEIVER so the direction is visible without reading a label.
                let col = crate::theme::accent().gamma_multiply(0.35 + 0.65 * heat);
                p.line_segment([egui::pos2(x0, y), egui::pos2(x1, y)], egui::Stroke::new(1.0 + 1.5 * heat, col));
                p.circle_filled(egui::pos2(x0 + (x1 - x0) * 0.72, y), 2.0 + heat, col);
            } else {
                // Configured but idle: a dashed hint, deliberately quiet.
                let step = 6.0;
                let (a, b) = (x0.min(x1), x0.max(x1));
                let mut x = a;
                while x < b {
                    p.line_segment([egui::pos2(x, y), egui::pos2((x + 3.0).min(b), y)],
                                   egui::Stroke::new(1.0, crate::theme::DIM().gamma_multiply(0.35)));
                    x += step;
                }
            }
        }
    }

    // the columns
    ui.horizontal_top(|ui| {
        for (i, c) in cols.iter_mut().enumerate() {
            ui.allocate_ui(egui::vec2(col_w - 6.0, ui.available_height()), |ui| {
                let aimed = aim == Some(c.id);
                let head = format!("{} · {}", c.id, c.host);
                if crate::theme::carved_button_tinted(ui, &head,
                        if aimed { crate::theme::accent() } else { crate::theme::DIM() }).clicked() {
                    act.aim = Some(if aimed { None } else { Some(c.id) });
                }
                ui.add(egui::TextEdit::singleline(c.assignment)
                    .hint_text("what this one works on").desired_width(col_w - 16.0));
                ui.label(egui::RichText::new(format!("turn {}", c.turn))
                    .color(crate::theme::DIM()).small());
                egui::ScrollArea::vertical().id_salt(("replicant_lane", i)).max_height(220.0)
                    .show(ui, |ui| {
                        for line in c.lines.iter().rev().take(12) {
                            ui.label(egui::RichText::new(line).small());
                            ui.separator();
                        }
                    });
            });
        }
    });
    act
}

fn label_head(s: &str) -> String {
    let t = s.trim();
    if t.is_empty() { return "—".into(); }
    t.chars().take(10).collect()
}

/// A rectangle helper kept local so the widgets do not depend on layout details elsewhere.
#[allow(dead_code)]
fn pad(r: Rect, by: f32) -> Rect { Rect::from_min_size(r.min + Vec2::splat(by), r.size() - Vec2::splat(by * 2.0)) }

#[allow(dead_code)]
fn transparent() -> Color32 { Color32::TRANSPARENT }
