//! Web-dashboard-style widgets for the egui port: the exact palette, the
//! signature metric "row" (fill = mean, white whisker = p99, red tick = max,
//! dashed budget line, fill turns red when p99 > budget), panel cards with a
//! coloured lead bar + per-section copy, achievement cells, badges, event
//! pills, and run-list items. Built on egui 0.34 primitives.

use crate::model::{Metric, Workload};
use egui::text::{LayoutJob, TextFormat};
use egui::{
    Align, Color32, CornerRadius, FontFamily, FontId, Frame, Layout, Margin, Pos2, Rect, RichText,
    Sense, Stroke, StrokeKind, Ui,
};

// ── palette (exact hex from unendlich-telemetry.html) ───────────────────────
pub const BG: Color32 = Color32::from_rgb(0x0a, 0x0c, 0x10);
pub const PANEL: Color32 = Color32::from_rgb(0x10, 0x14, 0x1b);
pub const PANEL2: Color32 = Color32::from_rgb(0x16, 0x1b, 0x24);
pub const LINE: Color32 = Color32::from_rgb(0x22, 0x2a, 0x36);
pub const INK: Color32 = Color32::from_rgb(0xcd, 0xd6, 0xe3);
pub const DIM: Color32 = Color32::from_rgb(0x7b, 0x87, 0x98);
pub const FAINT: Color32 = Color32::from_rgb(0x4b, 0x55, 0x63);
pub const AMBER: Color32 = Color32::from_rgb(0xf7, 0xa2, 0x3b);
pub const AMBER_SOFT: Color32 = Color32::from_rgb(0xff, 0xce, 0x8a);
pub const CYAN: Color32 = Color32::from_rgb(0x57, 0xd6, 0xc9);
pub const GREEN: Color32 = Color32::from_rgb(0x8f, 0xd6, 0x94);
pub const RED: Color32 = Color32::from_rgb(0xff, 0x63, 0x63);
pub const VIOLET: Color32 = Color32::from_rgb(0x9a, 0x8c, 0xff);
pub const TRACK_BG: Color32 = Color32::from_rgb(0x0b, 0x0e, 0x13);
const MAXMARK: Color32 = Color32::from_rgb(0xb4, 0x46, 0x46);
const WHISKER: Color32 = Color32::from_rgb(0xff, 0xff, 0xff);

// back-compat aliases used by the compare view / older call sites
pub const ACCENT: Color32 = AMBER;
pub const GOOD: Color32 = GREEN;
pub const WARN: Color32 = AMBER;
pub const BAD: Color32 = RED;

// Corner radii as plain calls (not `const`, so we don't depend on
// `CornerRadius::same` being a const fn across egui point releases).
fn r(n: u8) -> CornerRadius {
    CornerRadius::same(n)
}

/// p99 → colour, against the 8.3 ms frame budget.
pub fn budget_color(p99: f64) -> Color32 {
    if p99 > 8.3 {
        RED
    } else if p99 > 5.0 {
        AMBER
    } else {
        GREEN
    }
}

// ── panel card ──────────────────────────────────────────────────────────────
/// A `.panel` card: panel fill, line border, rounded, with an `h2` header made
/// of a coloured lead bar (▌) + uppercase title and an optional right-aligned
/// `⧉ copy` that drops `copy` onto the clipboard.
pub fn panel(ui: &mut Ui, accent: Color32, title: &str, copy: Option<String>, body: impl FnOnce(&mut Ui)) {
    Frame {
        inner_margin: Margin::same(16),
        outer_margin: Margin { bottom: 14, ..Default::default() },
        fill: PANEL,
        stroke: Stroke::new(1.0, LINE),
        corner_radius: r(12),
        shadow: unendlich_ui::glow(accent, 0.45),
        ..Default::default()
    }
    .show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            // coloured lead bar (painted, like the web's h2 ::before)
            let (bar, _) = ui.allocate_exact_size(egui::vec2(5.0, 15.0), Sense::hover());
            ui.painter().rect_filled(bar, r(1), accent);
            ui.label(RichText::new(title.to_uppercase()).size(10.5).color(DIM));
            if let Some(text) = copy {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button(RichText::new("copy").size(9.5).color(DIM)).clicked() {
                        ui.ctx().copy_text(text.clone());
                    }
                });
            }
        });
        ui.add_space(9.0);
        body(ui);
    });
}

// ── metric row (the signature widget) ────────────────────────────────────────
fn tf(color: Color32) -> TextFormat {
    TextFormat { font_id: FontId::new(11.0, FontFamily::Monospace), color, ..Default::default() }
}

fn metric_read(m: &Metric) -> LayoutJob {
    let mut j = LayoutJob::default();
    j.append(&format!("{:.2}", m.mean), 0.0, tf(INK));
    j.append("  p99 ", 0.0, tf(DIM));
    j.append(&format!("{:.2}", m.p99), 0.0, tf(AMBER_SOFT));
    j.append(&format!("  \u{2191}{:.2}", m.max), 0.0, tf(RED));
    if let (Some(n), Some(ms)) = (m.over_n, m.over_ms) {
        if n > 0 {
            j.append(&format!("  {n}\u{00d7}>{ms}"), 0.0, tf(RED));
        }
    }
    j
}

fn workload_read(wl: &Workload) -> LayoutJob {
    let mut j = LayoutJob::default();
    j.append(&format!("p50 {}", wl.p50), 0.0, tf(INK));
    j.append("  max ", 0.0, tf(DIM));
    j.append(&format!("{}", wl.max), 0.0, tf(AMBER_SOFT));
    j.append("  chunks", 0.0, tf(DIM));
    j
}

const NAME_W: f32 = 92.0;
const READ_W: f32 = 196.0;
const TRACK_H: f32 = 22.0;

fn paint_track(
    ui: &mut Ui,
    fill_frac: f32,
    whisker: Option<f32>,
    maxf: Option<f32>,
    budget: Option<f32>,
    over: bool,
    accent: Color32,
) {
    let w = (ui.available_width() - READ_W - 8.0).max(48.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, TRACK_H), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, r(4), TRACK_BG);
    let f = fill_frac.clamp(0.0, 1.0);
    if f > 0.0 {
        let mut fr = rect;
        fr.set_width(rect.width() * f);
        p.rect_filled(fr, r(4), if over { RED } else { accent });
    }
    p.rect_stroke(rect, r(4), Stroke::new(1.0, LINE), StrokeKind::Inside);
    let x_at = |v: f32| rect.left() + v.clamp(0.0, 1.0) * rect.width();
    if let Some(b) = budget {
        let x = x_at(b);
        p.line_segment([Pos2::new(x, rect.top() - 3.0), Pos2::new(x, rect.bottom() + 3.0)], Stroke::new(1.0, FAINT));
    }
    if let Some(mx) = maxf {
        let x = x_at(mx);
        p.line_segment([Pos2::new(x, rect.top() + 3.0), Pos2::new(x, rect.bottom() - 3.0)], Stroke::new(2.0, MAXMARK));
    }
    if let Some(wk) = whisker {
        let x = x_at(wk);
        p.line_segment([Pos2::new(x, rect.top() - 2.0), Pos2::new(x, rect.bottom() + 2.0)], Stroke::new(2.0, WHISKER));
    }
}

fn empty_track(ui: &mut Ui) {
    let w = (ui.available_width() - READ_W - 8.0).max(48.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, TRACK_H), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, r(4), TRACK_BG);
    p.rect_stroke(rect, r(4), Stroke::new(1.0, LINE), StrokeKind::Inside);
}

fn name_cell(ui: &mut Ui, name: &str, sub: &str) {
    ui.allocate_ui_with_layout(egui::vec2(NAME_W, TRACK_H + 4.0), Layout::top_down(Align::LEFT), |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        ui.label(RichText::new(name).size(12.0).color(INK));
        if !sub.is_empty() {
            ui.label(RichText::new(sub).size(9.0).color(FAINT));
        }
    });
}

fn read_cell(ui: &mut Ui, job: LayoutJob) {
    ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), TRACK_H + 4.0), Layout::right_to_left(Align::Center), |ui| {
        ui.label(job);
    });
}

/// One metric row: `name | track | read`. `scale_max` is the full-scale value
/// the bar/markers are measured against; `budget` (if any) draws the dashed
/// line and reddens the fill when p99 exceeds it.
pub fn metric_row(ui: &mut Ui, name: &str, sub: &str, m: Option<&Metric>, accent: Color32, scale_max: f64, budget: Option<f64>) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 12.0;
        match m {
            Some(m) => {
                name_cell(ui, name, sub);
                let sc = scale_max.max(1e-6);
                let over = budget.map(|b| m.p99 > b).unwrap_or(false);
                paint_track(
                    ui,
                    (m.mean / sc) as f32,
                    Some((m.p99 / sc) as f32),
                    Some((m.max / sc) as f32),
                    budget.map(|b| (b / sc) as f32),
                    over,
                    accent,
                );
                read_cell(ui, metric_read(m));
            }
            None => {
                name_cell(ui, name, if sub.is_empty() { "idle" } else { sub });
                empty_track(ui);
                ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), TRACK_H + 4.0), Layout::right_to_left(Align::Center), |ui| {
                    ui.label(RichText::new("\u{2014}").size(11.0).color(FAINT));
                });
            }
        }
    });
}

/// Workload row: counts rather than ms (fill = p50, whisker = p90, tick = max).
pub fn workload_row(ui: &mut Ui, wl: &Workload, scale_max: f64) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 12.0;
        name_cell(ui, &wl.kind, "chunks");
        let sc = scale_max.max(1.0);
        paint_track(
            ui,
            (wl.p50 as f64 / sc) as f32,
            Some((wl.p90 as f64 / sc) as f32),
            Some((wl.max as f64 / sc) as f32),
            None,
            false,
            GREEN,
        );
        read_cell(ui, workload_read(wl));
    });
}

// ── achievement cell ──────────────────────────────────────────────────────────
pub fn achievement_cell(ui: &mut Ui, icon: &str, title: &str, desc: &str, got: bool, bad: bool) {
    let (border, title_col, fill) = if got && bad {
        (RED, RED, Color32::from_rgb(0x19, 0x0d, 0x0d))
    } else if got {
        (GREEN, GREEN, Color32::from_rgb(0x0c, 0x16, 0x10))
    } else {
        (LINE, DIM, TRACK_BG)
    };
    Frame {
        inner_margin: Margin { left: 11, right: 11, top: 9, bottom: 9 },
        fill,
        stroke: Stroke::new(1.0, border),
        corner_radius: CornerRadius::same(8),
        ..Default::default()
    }
    .show(ui, |ui| {
        // Fill the column the layout gives us (see grid_fit) instead of a fixed
        // width, so a wide page stretches the cells rather than leaving dead space.
        ui.set_width(ui.available_width());
        ui.set_min_height(30.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 9.0;
            let ic = RichText::new(icon).size(16.0);
            ui.label(if got { ic } else { ic.color(FAINT) });
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 1.0;
                ui.label(RichText::new(title).size(10.5).strong().color(title_col));
                ui.label(RichText::new(desc).size(9.0).color(FAINT));
            });
        });
    });
}

// ── badges / pills ───────────────────────────────────────────────────────────
pub fn badge(ui: &mut Ui, text: &str, override_flag: bool) {
    // Delegate to the shared kit pill: a measured widget that wraps as a whole
    // unit (never a glyph-column) and text-wraps over-wide values instead of
    // overflowing the row. override = filled cyan, default = faint outline.
    let color = if override_flag { CYAN } else { DIM };
    unendlich_ui::pill(ui, text, color, override_flag);
}

pub fn event_pill(ui: &mut Ui, kind: &str, count: usize, spike: bool) {
    let cc = if spike { RED } else { AMBER };
    Frame {
        inner_margin: Margin { left: 9, right: 9, top: 4, bottom: 4 },
        fill: TRACK_BG,
        stroke: Stroke::new(1.0, LINE),
        corner_radius: r(6),
        ..Default::default()
    }
    .show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 5.0;
            ui.label(RichText::new(kind).size(11.0).strong().color(cc));
            ui.label(RichText::new(format!("\u{00d7}{count}")).size(11.0).color(DIM));
        });
    });
}

// ── run-list item (sidebar) ───────────────────────────────────────────────────
/// One `.runitem` card. Returns true when clicked.
pub fn runitem(ui: &mut Ui, idx: usize, ts: &str, overrides: &str, totals: &str, selected: bool) -> bool {
    let (fill, stroke) = if selected {
        (Color32::from_rgb(0x15, 0x11, 0x0a), AMBER)
    } else {
        (PANEL2, LINE)
    };
    let inner = Frame {
        inner_margin: Margin { left: 10, right: 10, top: 8, bottom: 8 },
        outer_margin: Margin { bottom: 5, ..Default::default() },
        fill,
        stroke: Stroke::new(1.0, stroke),
        corner_radius: CornerRadius::same(8),
        ..Default::default()
    }
    .show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = 3.0;
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("#{idx}")).size(12.0).strong().color(AMBER));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(RichText::new(ts).size(9.5).color(DIM));
            });
        });
        if overrides.is_empty() {
            ui.label(RichText::new("all defaults").size(10.0).color(FAINT));
        } else {
            ui.label(RichText::new(overrides).size(10.0).color(CYAN));
        }
        ui.label(RichText::new(totals).size(10.0).color(DIM));
    });
    // Hover: fade an amber border in on the item the pointer is over (selected
    // items already wear amber, so skip them). animate_bool gives the smooth ramp.
    let resp = inner.response.interact(Sense::click());
    let t = ui.ctx().animate_bool(resp.id, resp.hovered());
    if t > 0.0 && !selected {
        ui.painter().rect_stroke(
            resp.rect,
            CornerRadius::same(8),
            Stroke::new(1.0, AMBER.gamma_multiply(0.45 * t)),
            StrokeKind::Inside,
        );
    }
    // Ask-first: a click only counts if nothing above (e.g. an open modal) vetoes it.
    resp.clicked() && unendlich_ui::intent::may_fire(ui, &resp)
}

// ── legacy helpers retained for the compare view ──────────────────────────────
pub fn stat_card(ui: &mut Ui, label: &str, value: &str, accent: Color32) {
    Frame {
        inner_margin: Margin::same(12),
        fill: PANEL2,
        stroke: Stroke::new(1.0, LINE),
        corner_radius: CornerRadius::same(10),
        ..Default::default()
    }
    .show(ui, |ui| {
        ui.vertical(|ui| {
            ui.set_min_width(120.0);
            ui.label(RichText::new(label.to_uppercase()).size(9.5).color(DIM));
            ui.label(RichText::new(value).size(20.0).strong().color(accent));
        });
    });
}

pub fn section(ui: &mut Ui, title: &str) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new("\u{258c}").size(14.0).color(AMBER));
        ui.label(RichText::new(title.to_uppercase()).size(10.5).color(DIM));
    });
    ui.add_space(4.0);
}

pub fn chip(ui: &mut Ui, text: &str, color: Color32) {
    Frame {
        inner_margin: Margin { left: 9, right: 9, top: 3, bottom: 3 },
        fill: Color32::TRANSPARENT,
        stroke: Stroke::new(1.0, color),
        corner_radius: r(10),
        ..Default::default()
    }
    .show(ui, |ui| {
        ui.label(RichText::new(text).size(10.5).strong().color(color));
    });
}

/// `Rect` is re-exported for callers that need to size custom areas.
pub type _RectAlias = Rect;
