use super::{card, row};
use crate::hold::{self, Target, Targets};
use crate::shell::ShellState;
use crate::source;
use egui::{Rect, RichText, Ui, UiBuilder};
use stoat_core::backend::StepKind;
use stoat_core::diag::Severity;
use stoat_core::model::RunState;
use uikit_egui::{GestureLayer, PaletteColors};

const BAR_H: f32 = 46.0;

pub fn ui(ui: &mut Ui, st: &mut ShellState, c: &PaletteColors, swipe: &mut GestureLayer, targets: &mut Targets, gestures: &[uikit_egui::touch::Gesture]) {
    // A long press, against what was drawn LAST frame (immediate mode: this
    // frame's rects do not exist yet). Tap stays tap; the recogniser reports
    // nothing while a widget owns the contact.
    for g in gestures {
        if let uikit_egui::touch::Gesture::LongPress(p) = g {
            match targets.hit(egui::pos2(p.x, p.y)).cloned() {
                Some(Target::SourceLine { file, line }) => {
                    let loc = format!("{}:{line}", file.display());
                    st.execute(&stoat_core::command::Command::Break(loc));
                    st.toast(format!("breakpoint at {}:{line}", file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()), uikit_egui::toast::Severity::Success);
                }
                Some(Target::Local { name, value }) => {
                    ui.ctx().copy_text(format!("{name} = {value}"));
                    st.toast(format!("copied {name}"), uikit_egui::toast::Severity::Info);
                }
                Some(Target::Location(loc)) => {
                    ui.ctx().copy_text(loc);
                    st.toast("copied location", uikit_egui::toast::Severity::Info);
                }
                None => {}
            }
        }
    }
    targets.clear();
    let full = ui.available_rect_before_wrap();
    let body = Rect::from_min_max(full.min, egui::pos2(full.max.x, (full.max.y - BAR_H - 6.0).max(full.min.y)));
    let bar = Rect::from_min_max(egui::pos2(full.min.x, body.max.y + 6.0), full.max);

    // Sideways drag walks the stack: the kit's gesture model, the same one every
    // other app pages with, retuned to this surface so its thresholds stay
    // fractions of the width. Next = toward the caller (gdb's `up`). The ends do
    // not wrap; past the outermost frame there is nothing, and saying so with the
    // barrier is more honest than landing back on frame 0.
    let n = st.session.frames.len();
    let stopped = st.session.state == RunState::Stopped && n > 0;
    let mut drag_px = 0.0;
    if stopped {
        *swipe.config_mut() = uikit_gesture::GestureConfig::for_page_size(f64::from(body.width()));
        let g = swipe.update(ui, body, true);
        // `raw` is the model's accumulator, not a drag offset: it does not return
        // to zero at release (seen on screen: after a committed swipe the frame
        // stayed 12pt left of where it belongs). Follow the finger only while
        // there is one.
        if ui.input(|i| i.pointer.primary_down()) {
            drag_px = (-g.raw).clamp(-body.width() * 0.5, body.width() * 0.5);
        }
        if let Some(dir) = g.page_jump {
            let at = st.session.selected;
            match dir {
                uikit_gesture::Direction::Next if at + 1 < n => st.toward_caller(),
                uikit_gesture::Direction::Prev if at > 0 => st.toward_callee(),
                _ => {}
            }
        }
        if let Some(edge) = g.barrier_edge {
            uikit_egui::paint_barrier(ui, body, g.barrier_fraction, edge, c.accent);
        }
    }

    let shifted = body.translate(egui::vec2(drag_px, 0.0));
    ui.scope_builder(UiBuilder::new().max_rect(shifted), |ui| {
        ui.set_clip_rect(body);
        frame_body(ui, st, c, targets);
    });
    ui.scope_builder(UiBuilder::new().max_rect(bar), |ui| transport(ui, st, c));
}

fn frame_body(ui: &mut Ui, st: &mut ShellState, c: &PaletteColors, targets: &mut Targets) {
    let level = st.session.current_frame().map(|f| f.level);
    // Keyed by frame: each frame keeps its own scroll offset instead of
    // inheriting the one you left the previous frame at.
    uikit_egui::scroll::area_keyed(ui, ("stack-frame", level), |ui| {
        ui.set_width(ui.available_width());
        if let Some((d, vitals_notes)) = &st.diagnostic {
            card(ui, c, |ui| {
                let (word, col) = match d.severity {
                    Severity::Error => ("error", c.danger),
                    Severity::Warning => ("warning", c.warning),
                    Severity::Info => ("info", c.accent),
                };
                ui.label(RichText::new(format!("{word}[{}]: {}", d.code, d.title)).strong().color(col));
                for n in d.notes.iter().chain(vitals_notes) {
                    ui.label(RichText::new(format!("note: {n}")).small().color(c.text));
                }
                for h in &d.help {
                    ui.label(RichText::new(format!("help: {h}")).small().color(c.success));
                }
            });
            ui.add_space(6.0);
        }

        let Some(f) = st.session.current_frame().cloned() else {
            ui.add_space(24.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new(match st.session.state {
                    RunState::Running => "Running",
                    RunState::Exited => "Exited",
                    _ => "Not stopped",
                }).heading().color(c.text_weak));
                ui.label(RichText::new("Frames appear here when the program stops. Drag sideways, or use the arrows, to walk the stack.").color(c.text_weak));
            });
            return;
        };

        let title = ui.label(RichText::new(f.func.as_deref().unwrap_or("??")).strong().size(17.0));
        let loc = ui.label(RichText::new(format!("#{} · {}", f.level, f.location())).small().color(c.text_weak));
        targets.add(title.rect.union(loc.rect), Target::Location(f.location()));
        ui.add_space(4.0);

        let lines = f.file.as_deref().and_then(|p| st.sources.lines(p));
        match (lines, f.line) {
            (Some(lines), Some(line)) => {
                let tints = source::Tints { text: c.text, weak: c.text_weak, current_bg: c.accent_soft };
                let job = source::layout(&lines, line, 6, &tints, 12.0, ui.available_width());
                // Laid out here rather than inside the Label so the rows are in
                // hand: a long press needs to know which SOURCE line a wrapped
                // row belongs to.
                let galley = ui.ctx().fonts_mut(|fonts| fonts.layout_job(job));
                let r = ui.add(egui::Label::new(galley.clone()).selectable(false));
                if let (Some(file), Some(first)) = (f.file.clone(), source::window(&lines, line, 6).first().map(|w| w.0)) {
                    let rows: Vec<(Rect, bool)> = galley.rows.iter().map(|row| (row.rect().translate(r.rect.min.to_vec2()), row.ends_with_newline)).collect();
                    for (i, (rect, _)) in rows.iter().enumerate() {
                        if let Some(n) = hold::line_at(&rows, first, rect.center()) {
                            // One target per row; rows of one wrapped line share its number.
                            let _ = i;
                            targets.add(*rect, Target::SourceLine { file: file.clone(), line: n });
                        }
                    }
                }
            }
            (None, Some(_)) => drop(ui.label(RichText::new("source file is not on this machine").small().color(c.text_weak))),
            _ => drop(ui.label(RichText::new("no line info for this frame").small().color(c.text_weak))),
        }

        ui.add_space(6.0);
        let vars = st.session.locals.get(&f.level).cloned();
        match vars {
            None => drop(ui.label(RichText::new("locals…").small().color(c.text_weak))),
            Some(vars) if vars.is_empty() => drop(ui.label(RichText::new("no locals in this frame").small().color(c.text_weak))),
            Some(vars) => {
                let mut expand = None;
                card(ui, c, |ui| {
                    for v in &vars {
                        let title = if v.is_arg { format!("{}  (arg)", v.name) } else { v.name.clone() };
                        let expanded = st.values.get(&(f.level, v.name.clone()));
                        match v.value.as_ref().or(expanded) {
                            Some(val) => {
                                let a = ui.label(RichText::new(title).small().color(c.text_weak));
                                let b = ui.add(egui::Label::new(RichText::new(val).monospace()).wrap());
                                targets.add(a.rect.union(b.rect), Target::Local { name: v.name.clone(), value: val.clone() });
                                ui.add_space(4.0);
                            }
                            // Aggregates are not fetched at the stop (--simple-values):
                            // a phone should not print every struct in scope on every
                            // stop. One tap evaluates the one you want.
                            None => {
                                let ty = v.ty.as_deref().unwrap_or("aggregate");
                                if row(ui, c, false, "", &title, &format!("{ty} — tap to evaluate")) {
                                    expand = Some(v.name.clone());
                                }
                            }
                        }
                    }
                });
                if let Some(name) = expand {
                    st.evaluate(&name);
                }
            }
        }
    });
}

fn transport(ui: &mut Ui, st: &mut ShellState, c: &PaletteColors) {
    let ctx = ui.ctx().clone();
    let g = |glyph: char, word: &str| uikit_egui::icons::glyph_or(&ctx, glyph, word);
    let stopped = st.session.state == RunState::Stopped;
    let running = st.session.state == RunState::Running;
    let n = st.session.frames.len();
    let at = st.session.selected;

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let w = ((ui.available_width() - 5.0 * 4.0) / 6.0).floor();
        let button = |ui: &mut Ui, enabled: bool, label: String, tip: &str| {
            ui.add_enabled(enabled, egui::Button::new(RichText::new(label).size(16.0)).min_size(egui::vec2(w, BAR_H - 6.0))).on_hover_text(tip).clicked()
        };
        if button(ui, stopped && at > 0, g('‹', "in"), "toward the callee (gdb: down)") {
            st.toward_callee();
        }
        if button(ui, stopped && at + 1 < n, g('›', "out"), "toward the caller (gdb: up)") {
            st.toward_caller();
        }
        let (go, tip) = if running { (g('⏸', "stop"), "interrupt") } else { (g('▶', "go"), "continue") };
        if button(ui, stopped || running, go, tip) {
            st.go_or_interrupt();
        }
        if button(ui, stopped, g('⤼', "over"), "step over") {
            st.step(StepKind::Over);
        }
        if button(ui, stopped, g('⤓', "into"), "step into") {
            st.step(StepKind::Into);
        }
        if button(ui, stopped, g('⤒', "ret"), "step out") {
            st.step(StepKind::Out);
        }
    });
    let _ = c;
}

/// "frame 11 of 12" and, with more than one thread, the thread picker. Drawn by
/// the shell above the page so it survives the drag offset.
pub fn header(ui: &mut Ui, st: &mut ShellState, c: &PaletteColors) {
    let n = st.session.frames.len();
    if st.session.state != RunState::Stopped || n == 0 {
        return;
    }
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("frame {} of {n}", st.session.selected + 1)).small().color(c.text_weak));
        if st.session.threads.len() > 1 {
            let current = st.session.thread;
            let label = |t: &stoat_core::model::Thread| format!("thread {} {}", t.id, t.name.as_deref().unwrap_or(""));
            let shown = st.session.threads.iter().find(|t| Some(t.id) == current).map(label).unwrap_or_else(|| "thread".into());
            let mut pick = None;
            egui::ComboBox::from_id_salt("stack-thread").selected_text(shown).show_ui(ui, |ui| {
                for t in &st.session.threads {
                    if ui.selectable_label(Some(t.id) == current, label(t)).clicked() {
                        pick = Some(t.id);
                    }
                }
            });
            if let Some(id) = pick.filter(|id| Some(*id) != current) {
                st.select_thread(id);
            }
        }
    });
}
