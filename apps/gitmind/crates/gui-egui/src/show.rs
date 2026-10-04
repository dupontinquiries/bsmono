//! show — how gitmind draws what went wrong, borrowed from stoat-egui.
//!
//! Two things stoat does that made its debugging readable and gitmind's not:
//!
//! 1. A diagnosis is data (`cockpit::finding::Finding`) and is drawn as the
//!    kit's CALLOUT: severity chip, code, a title in plain words, notes that
//!    explain, help lines in the success colour so the way out is findable.
//!    gitmind printed verdicts with `{:?}` and failures as one-line strings.
//! 2. Logs are TYPED rows (`uikit_egui::log_view`): a level chip, wrapped
//!    text, levels from the kit's `classify_line`, which classifies build
//!    output by the SHAPE of a diagnostic. gitmind's push page used
//!    `contains("error")`, which paints `Compiling thiserror` red.

use cockpit::finding::{Finding, Severity};
use uikit_egui::callout;
use uikit_egui::log_view::{classify_line, LogLevel};
use uikit_egui::toast::Severity as KitSeverity;
use uikit_egui::PaletteColors;

fn kit(sev: Severity) -> KitSeverity {
    match sev {
        Severity::Error => KitSeverity::Error,
        Severity::Warning => KitSeverity::Warning,
        Severity::Info => KitSeverity::Info,
        Severity::Ok => KitSeverity::Success,
    }
}

/// One finding as a collapsible callout (open state remembered per code),
/// with the fix, if any, as copyable monospace text below it.
pub fn finding(ui: &mut egui::Ui, c: &PaletteColors, f: &Finding) {
    let id = egui::Id::new(("finding-open", &f.code));
    let mut open = ui.data(|d| d.get_temp::<bool>(id)).unwrap_or(true);
    let lines: Vec<callout::Line> = f.notes.iter().map(callout::Line::note).chain(f.help.iter().map(callout::Line::help)).collect();
    // Scoped by code: the callout's own widgets are not keyed by content, so
    // two callouts on one page clashed (egui's red "second use of widget ID",
    // seen in a rendered screenshot with two findings).
    ui.push_id(("finding", &f.code), |ui| {
        callout::show_collapsible(ui, c, kit(f.severity), &f.code, &f.title, &lines, &mut open);
    });
    ui.data_mut(|d| d.insert_temp(id, open));
    if let (true, Some(fix)) = (open, &f.fix) {
        // The button on its own line, the fix full-width below: side by side
        // in a wrapped row, the long rule wrapped over the button (the phone
        // layout test caught it; the old Diagnostics page had the same bug).
        if ui.small_button("copy fix").clicked() {
            ui.ctx().copy_text(fix.clone());
        }
        ui.add(egui::Label::new(egui::RichText::new(fix).monospace().small()).wrap());
    }
}

pub fn findings(ui: &mut egui::Ui, c: &PaletteColors, list: &[Finding]) {
    for f in list {
        finding(ui, c, f);
        ui.add_space(4.0);
    }
}

// ── the task log ─────────────────────────────────────────────────────────────
//
// What every build, deploy and scan prints, and the thing a user reads when one
// fails. It was a 140-200 px box of kit `log_row`s: each line a DEBUG/INFO chip
// in a padded frame, 9 pt proportional text, so a 200 px box held about seven
// lines of "DEBUG Compiling ..." — and cargo prints hundreds. The error you
// needed was somewhere above, in a box too small to find it in.
//
// Now: dense monospace rows (the level is a coloured gutter mark and a tint,
// not a chip), a pane at least a third of the window tall and resizable by its
// bottom edge, "problems only" to see just the warnings and errors with the
// lines that explain them, copy, and a full-window view (also reachable from the
// task strip on every tab).

/// Monospace size for log text. egui's `.small()` monospace was 9 pt.
pub const LOG_TEXT: f32 = 13.0;
/// One unwrapped row: the text plus a little leading. The theme's item
/// spacing (8 px) between rows made a log read double-spaced.
const LINE_H: f32 = LOG_TEXT + 5.0;

/// The pane's own state, per log (`salt`), kept in egui memory.
#[derive(Clone, Copy, Debug, PartialEq)]
struct PaneState {
    /// User-chosen height (dragged); None = the default.
    height: Option<f32>,
    problems_only: bool,
}

impl Default for PaneState {
    fn default() -> Self { PaneState { height: None, problems_only: false } }
}

fn pane_id(salt: &str) -> egui::Id { egui::Id::new(("task-log-pane", salt)) }
fn overlay_id(salt: &str) -> egui::Id { egui::Id::new(("task-log-overlay", salt)) }

/// The rows "problems only" shows: every warning or error line and what
/// follows it up to the next blank line — rustc prints a diagnostic as a
/// block (`error[E0308]: ...`, ` --> src/x.rs:3:9`, the snippet, `= note`),
/// and the first line alone does not say where. Capped at 40 lines a block so
/// a runaway block cannot swallow the rest.
pub fn problem_rows(lines: &[String]) -> Vec<usize> {
    let mut out = Vec::new();
    let mut in_block = 0usize;
    for (i, l) in lines.iter().enumerate() {
        let lvl = classify_line(l);
        if matches!(lvl, LogLevel::Warn | LogLevel::Error) {
            in_block = 40;
        } else if l.trim().is_empty() {
            in_block = 0;
        }
        if in_block > 0 {
            out.push(i);
            in_block -= 1;
        }
    }
    out
}

/// The default pane height for a window `screen_h` tall: a third of it, never
/// less than the caller's minimum, never more than 60%.
pub fn default_height(screen_h: f32, min: f32) -> f32 {
    (screen_h * 0.36).max(min).min((screen_h * 0.6).max(min))
}

fn level_colour(l: LogLevel, ui: &egui::Ui) -> (egui::Color32, egui::Color32) {
    let text = ui.visuals().text_color();
    match l {
        LogLevel::Error => (LogLevel::Error.badge().1, LogLevel::Error.badge().1),
        LogLevel::Warn => (LogLevel::Warn.badge().1, LogLevel::Warn.badge().1),
        LogLevel::Info => (LogLevel::Info.badge().1, text),
        // Progress lines: most of a build. Readable, but a step back.
        LogLevel::Debug => (egui::Color32::TRANSPARENT, text.gamma_multiply(0.78)),
    }
}

/// One dense row: a 3 px gutter mark in the level's colour, then the line.
fn row(ui: &mut egui::Ui, line: &str, wrap: bool) {
    let lvl = classify_line(line);
    let (mark, col) = level_colour(lvl, ui);
    ui.horizontal_top(|ui| {
        let (r, _) = ui.allocate_exact_size(egui::vec2(3.0, LINE_H), egui::Sense::hover());
        if mark != egui::Color32::TRANSPARENT { ui.painter().rect_filled(r, 1.0, mark); }
        ui.add_space(4.0);
        let text = egui::RichText::new(if line.is_empty() { " " } else { line }).monospace().size(LOG_TEXT).color(col);
        let label = egui::Label::new(text);
        ui.add(if wrap { label.wrap() } else { label.extend() });
    });
}

/// The rows, virtualised when unwrapped (every row is one line tall, so only
/// the visible ones are laid out and the whole log can be kept), newest last
/// and pinned to the bottom while it grows.
fn rows(ui: &mut egui::Ui, id: egui::Id, lines: &[String], shown: &[usize], wrap: bool, height: f32) {
    let area = egui::ScrollArea::both().id_salt(id).max_height(height).min_scrolled_height(height)
        .stick_to_bottom(true).auto_shrink([false, false]);
    if wrap {
        let area = egui::ScrollArea::vertical().id_salt(id).max_height(height).min_scrolled_height(height)
            .stick_to_bottom(true).auto_shrink([false, false]);
        // Wrapped rows have different heights; lay out the newest 600.
        let start = shown.len().saturating_sub(600);
        area.show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            if start > 0 {
                ui.label(egui::RichText::new(format!("… {start} earlier lines (full window shows all)")).weak());
            }
            for &i in &shown[start..] { row(ui, &lines[i], true); }
        });
    } else {
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            area.show_rows(ui, LINE_H, shown.len(), |ui, range| {
                for &i in &shown[range] { row(ui, &lines[i], false); }
            });
        });
    }
}

/// The toolbar both views share. Returns true if "full window" was pressed.
fn toolbar(ui: &mut egui::Ui, st: &mut PaneState, lines: &[String], shown: usize, in_overlay: bool) -> bool {
    let mut open = false;
    ui.horizontal_wrapped(|ui| {
        let problems = problem_rows(lines).iter().filter(|&&i| matches!(classify_line(&lines[i]), LogLevel::Warn | LogLevel::Error)).count();
        ui.label(egui::RichText::new(format!("log · {} lines", lines.len())).strong());
        let label = if problems == 0 { "problems only".to_string() } else { format!("problems only ({problems})") };
        ui.toggle_value(&mut st.problems_only, label)
            .on_hover_text("warnings and errors, each with the lines that explain it");
        if ui.button("copy").on_hover_text("the whole log, to the clipboard").clicked() {
            ui.ctx().copy_text(lines.join("\n"));
        }
        if !in_overlay && ui.button("⤢ full window").clicked() { open = true; }
        if st.problems_only && shown == 0 {
            ui.label(egui::RichText::new("no warnings or errors").weak());
        }
    });
    open
}

/// The log pane for a page. `min_height` is the least it will be; by default
/// it is a third of the window, and dragging its bottom edge resizes it.
pub fn task_log(ui: &mut egui::Ui, salt: &str, lines: &[String], min_height: f32) {
    let id = pane_id(salt);
    let mut st: PaneState = ui.data(|d| d.get_temp(id)).unwrap_or_default();
    let screen_h = ui.ctx().content_rect().height();
    let compact = ui.ctx().content_rect().width() < 600.0;
    let all: Vec<usize>;
    let problems: Vec<usize>;
    let shown: &[usize] = if st.problems_only { problems = problem_rows(lines); &problems } else { all = (0..lines.len()).collect(); &all };
    egui::Frame::group(ui.style()).inner_margin(egui::Margin::same(6)).show(ui, |ui| {
        if toolbar(ui, &mut st, lines, shown.len(), false) {
            ui.data_mut(|d| d.insert_temp(overlay_id(salt), true));
        }
        let height = st.height.unwrap_or_else(|| default_height(screen_h, min_height)).clamp(min_height.min(120.0), screen_h * 0.9);
        // Wrapped on a phone (no one scrolls sideways through cargo output on
        // one); one line per row on a desktop, where it can all be kept.
        rows(ui, id.with("rows"), lines, shown, compact, height);
        // The resize grip: drag the bottom edge.
        let (grip, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 10.0), egui::Sense::drag());
        let c = if resp.hovered() || resp.dragged() { ui.visuals().strong_text_color() } else { ui.visuals().weak_text_color() };
        ui.painter().line_segment([grip.center() - egui::vec2(18.0, 0.0), grip.center() + egui::vec2(18.0, 0.0)], egui::Stroke::new(2.0_f32, c));
        if resp.hovered() || resp.dragged() { ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical); }
        if resp.dragged() {
            st.height = Some((height + resp.drag_delta().y).clamp(80.0, screen_h * 0.9));
        }
        if resp.double_clicked() { st.height = None; }
    });
    ui.data_mut(|d| d.insert_temp(id, st));
    log_overlay(ui.ctx(), salt, lines);
}

/// Open the full-window view of log `salt` (the task strip does this).
pub fn open_overlay(ctx: &egui::Context, salt: &str) {
    ctx.data_mut(|d| d.insert_temp(overlay_id(salt), true));
}

pub fn overlay_open(ctx: &egui::Context, salt: &str) -> bool {
    ctx.data(|d| d.get_temp::<bool>(overlay_id(salt))).unwrap_or(false)
}

/// The full-window log, when open: every line, over the page, closed by its
/// button or Esc.
pub fn log_overlay(ctx: &egui::Context, salt: &str, lines: &[String]) {
    if !overlay_open(ctx, salt) { return; }
    let id = pane_id(salt);
    let mut st: PaneState = ctx.data(|d| d.get_temp(id)).unwrap_or_default();
    let screen = ctx.content_rect();
    let compact = screen.width() < 600.0;
    let mut close = ctx.input(|i| i.key_pressed(egui::Key::Escape));
    egui::Area::new(id.with("overlay")).order(egui::Order::Foreground).fixed_pos(screen.min).constrain(false)
        .show(ctx, |ui| {
            // Opaque: this theme's panel fill is translucent glass, and the
            // page showed through the log (seen in a render).
            ui.painter().rect_filled(screen, 0.0, ui.visuals().extreme_bg_color.to_opaque());
            ui.allocate_rect(screen, egui::Sense::click_and_drag()); // nothing under it takes the press
            let mut inner = ui.new_child(egui::UiBuilder::new().max_rect(screen.shrink(10.0)));
            inner.horizontal(|ui| {
                if ui.button("✕ close").on_hover_text("back (Esc)").clicked() { close = true; }
            });
            let problems;
            let all;
            let shown: &[usize] = if st.problems_only { problems = problem_rows(lines); &problems } else { all = (0..lines.len()).collect::<Vec<_>>(); &all };
            toolbar(&mut inner, &mut st, lines, shown.len(), true);
            let h = inner.available_height();
            rows(&mut inner, id.with("overlay-rows"), lines, shown, compact, h);
        });
    ctx.data_mut(|d| d.insert_temp(id, st));
    if close { ctx.data_mut(|d| d.insert_temp(overlay_id(salt), false)); }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(v: &[&str]) -> Vec<String> { v.iter().map(|s| s.to_string()).collect() }

    #[test]
    fn problems_only_keeps_each_diagnostic_with_the_lines_that_locate_it() {
        let l = lines(&[
            "   Compiling a v1",
            "warning: unused variable: `x`",
            "  --> src/a.rs:3:9",
            "   |",
            "",
            "   Compiling thiserror v2",
            "error[E0308]: mismatched types",
            "   --> src/b.rs:4:1",
            "",
            "   Finished dev",
        ]);
        assert_eq!(problem_rows(&l), vec![1, 2, 3, 6, 7], "each block to its blank line; progress lines out");
        assert!(problem_rows(&lines(&["   Compiling x", "   Finished"])).is_empty());
    }

    #[test]
    fn a_runaway_block_is_capped() {
        let mut l = vec!["error: boom".to_string()];
        l.extend((0..100).map(|i| format!("   detail {i}")));
        assert_eq!(problem_rows(&l).len(), 40);
    }

    #[test]
    fn the_pane_is_a_third_of_the_window_within_bounds() {
        assert_eq!(default_height(900.0, 200.0), 900.0 * 0.36);
        assert_eq!(default_height(400.0, 200.0), 200.0, "never below the caller's minimum");
        assert!(default_height(3000.0, 200.0) <= 3000.0 * 0.6);
        // The old fixed boxes were 140-200 px; on a laptop window it is now well over.
        assert!(default_height(800.0, 140.0) > 280.0);
    }

    #[test]
    fn a_crate_named_error_is_not_an_error() {
        // the trap the push page fell into with contains("error")
        assert_eq!(classify_line("   Compiling thiserror v2.0.12"), LogLevel::Debug);
        assert_eq!(classify_line("error[E0308]: mismatched types"), LogLevel::Error);
    }

    #[test]
    fn every_severity_has_a_kit_colour() {
        for s in [Severity::Error, Severity::Warning, Severity::Info, Severity::Ok] {
            let _ = kit(s);
        }
    }
}
