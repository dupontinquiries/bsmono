//! fx_editor — the controls for one effect, on a phone or a desktop.
//!
//! ## Shape
//!
//! An effect lives in a chain (a clip's `effects` or a bus's `chain`), so
//! the editor is addressed by `(where, index)` — [`Target`] — and renders
//! the effect there from `studio_fx::schema::expand`. It knows nothing
//! about any particular effect: a compressor and a colour grade are the
//! same code path, and a new effect gets its controls by adding a schema
//! entry, not by touching this file.
//!
//! ## The two presentations
//!
//! Decided from WIDTH, not device class — the same rule `responsive_nav`
//! documents: a phone in landscape and a narrow desktop window want the
//! same thing.
//!
//! * **Narrow (< [`SHEET_BELOW`] pt):** a bottom sheet. Full width, anchored
//!   to the bottom edge, capped at half the screen so the clip stays
//!   visible above it, 44-pt touch rows, sliders that span the width, and a
//!   chip strip at the top to switch between the chain's effects without
//!   closing anything. One effect at a time, big.
//! * **Wide:** a floating panel anchored top-right, `dialog_width`-sized,
//!   the same controls at desktop density, with the chain strip as tabs.
//!
//! ## Commit discipline
//!
//! One op per release. During a drag the shown value comes from
//! `drag`; the chain is rewritten and sent as one `SetEffects` /
//! `SetBusChain` on `drag_stopped` (or on a click for discrete controls).
//! That is the rule every fader in the shell follows, for the reason
//! `Paint` gives: a packet per frame is not collaboration, it is noise.
//!
//! ## What "intuitive" meant here, concretely
//!
//! * Frequencies on a log slider with kHz formatting; nobody finds 2.4 kHz
//!   on a linear 20–20000 scale by touch.
//! * Enums as segmented buttons, not a dropdown: four saturation profiles
//!   are one tap, not two.
//! * Counts as `−  3 bands  +`, and the band sections appear/disappear
//!   as you press — the structure of the effect is visible.
//! * Every value readable as a plugin shows it (`-18.0 dB`, `4.0:1`), and
//!   double-clicking a slider resets it to the schema default.
//! * The bypass toggle and the "remove" are in the header, where a
//!   plugin window has them.

use egui::{Align2, RichText};
use studio_fx::schema::{self, Kind, ParamSpec, Scale};
use uikit_egui::PaletteColors;

use crate::Studio;

/// Narrower than this and the editor is a bottom sheet.
pub const SHEET_BELOW: f32 = 640.0;
/// Touch row height on the sheet.
const TOUCH_ROW: f32 = 44.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Clip(studio_core::ClipId),
    Bus(studio_core::BusId),
}

#[derive(Debug, Clone)]
pub struct FxEditor {
    pub target: Target,
    pub index: usize,
    /// A slider mid-drag: (param key, value shown). Committed on release.
    drag: Option<(String, f32)>,
}

impl FxEditor {
    pub fn open(target: Target, index: usize) -> FxEditor {
        FxEditor { target, index, drag: None }
    }
}

fn chain_of(s: &Studio, target: Target) -> Option<Vec<studio_core::Effect>> {
    let p = &s.session.doc.project;
    match target {
        Target::Clip(id) => p.clips.get(&id).map(|c| c.effects.clone()),
        Target::Bus(id) => p.busses.get(&id).map(|b| b.chain.clone()),
    }
}

fn title_of(s: &Studio, target: Target) -> String {
    let p = &s.session.doc.project;
    match target {
        Target::Clip(id) => {
            let src = p.clips.get(&id).and_then(|c| p.sources.get(&c.source))
                .map(|src| src.path.rsplit('/').next().unwrap_or(&src.path).to_string())
                .unwrap_or_default();
            format!("clip {} · {src}", id.0)
        }
        Target::Bus(id) => format!("bus {}", p.busses.get(&id).map(|b| b.name.as_str()).unwrap_or("?")),
    }
}

fn commit(s: &mut Studio, target: Target, chain: Vec<studio_core::Effect>) {
    match target {
        Target::Clip(id) => s.edit(studio_core::Op::SetEffects { id, effects: chain }, "clip fx"),
        Target::Bus(id) => s.edit(studio_core::Op::SetBusChain { id, chain }, "bus chain"),
    }
}

/// Draw the editor if one is open. Called once per frame from the shell.
pub fn window(ctx: &egui::Context, s: &mut Studio, c: &PaletteColors) {
    let Some(mut ed) = s.fx_editor.take() else { return };
    let Some(chain) = chain_of(s, ed.target) else { return }; // target vanished: editor closes
    if chain.is_empty() { return; }
    if ed.index >= chain.len() { ed.index = chain.len() - 1; }

    let screen = ctx.content_rect();
    let narrow = screen.width() < SHEET_BELOW;
    let mut open = true;
    let title = title_of(s, ed.target);

    let win = if narrow {
        egui::Window::new(&title).id(egui::Id::new("fx-editor"))
            .anchor(Align2::CENTER_BOTTOM, egui::vec2(0.0, 0.0))
            .fixed_size(egui::vec2(screen.width(), 0.0))
            .max_height(screen.height() * 0.5)
            .resizable(false).collapsible(false)
    } else {
        egui::Window::new(&title).id(egui::Id::new("fx-editor"))
            .anchor(Align2::RIGHT_TOP, egui::vec2(-12.0, 48.0))
            .default_width(uikit_egui::dialog::dialog_width(screen.width(), 380.0))
            .max_height(screen.height() * 0.8)
            .resizable(!narrow).collapsible(false)
    };

    let mut new_chain: Option<Vec<studio_core::Effect>> = None;
    let mut new_index = ed.index;
    let mut close = false;
    win.open(&mut open).show(ctx, |ui| {
        if narrow {
            ui.spacing_mut().interact_size.y = TOUCH_ROW;
            ui.spacing_mut().slider_width = (screen.width() - 140.0).max(120.0);
            ui.spacing_mut().button_padding = egui::vec2(14.0, 10.0);
        } else {
            ui.spacing_mut().slider_width = 200.0;
        }

        // The chain strip: which effect. Chips on the sheet, tabs on desktop.
        ui.horizontal_wrapped(|ui| {
            for (i, fx) in chain.iter().enumerate() {
                let label = if fx.enabled { fx.kind.clone() } else { format!("{} (off)", fx.kind) };
                if ui.add(egui::Button::selectable(i == ed.index, label)).clicked() {
                    new_index = i;
                }
            }
            ui.menu_button("+", |ui| {
                for (kind, what) in schema::KINDS {
                    if ui.button(format!("{kind}  —  {what}")).clicked() {
                        let mut ch = chain.clone();
                        ch.push(studio_core::Effect { kind: (*kind).into(), params: schema::defaults(kind), enabled: true });
                        new_index = ch.len() - 1;
                        new_chain = Some(ch);
                        ui.close();
                    }
                }
            });
        });
        ui.separator();

        let fx = &chain[ed.index];
        // Header: name, bypass, reset, remove.
        ui.horizontal(|ui| {
            ui.label(RichText::new(&fx.kind).strong().size(if narrow { 20.0 } else { 16.0 }));
            let what = schema::KINDS.iter().find(|(k, _)| *k == fx.kind).map(|(_, w)| *w).unwrap_or("");
            if !what.is_empty() { ui.colored_label(c.text_weak, what); }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("remove").on_hover_text("take this effect out of the chain").clicked() {
                    let mut ch = chain.clone();
                    ch.remove(ed.index);
                    new_index = ed.index.saturating_sub(1);
                    if ch.is_empty() { close = true; }
                    new_chain = Some(ch);
                }
                if ui.button("reset").on_hover_text("every parameter back to its default").clicked() {
                    let mut ch = chain.clone();
                    ch[ed.index].params = schema::defaults(&fx.kind);
                    new_chain = Some(ch);
                }
                let mut on = fx.enabled;
                let label = if on { "on" } else { "bypassed" };
                if ui.toggle_value(&mut on, label).changed() {
                    let mut ch = chain.clone();
                    ch[ed.index].enabled = on;
                    new_chain = Some(ch);
                }
            });
        });

        let Some(specs) = schema::expand(fx) else {
            // A kind this build's schema does not know (a plugin from a
            // newer peer): show the raw map so it is at least visible.
            ui.colored_label(c.text_weak, "no schema for this effect; raw parameters:");
            for (k, v) in &fx.params { ui.label(format!("{k} = {v}")); }
            return;
        };

        egui::ScrollArea::vertical().max_height(screen.height() * if narrow { 0.38 } else { 0.7 }).show(ui, |ui| {
            let mut current_group: Option<String> = None;
            for spec in &specs {
                if spec.group != current_group {
                    current_group = spec.group.clone();
                    if let Some(g) = &current_group {
                        ui.add_space(6.0);
                        ui.label(RichText::new(g).strong().color(c.text_weak));
                    }
                }
                let mut resp = None;
                let committed = control(ui, fx, spec, &mut ed.drag, narrow, &mut resp);
                // The knob is where automation starts (the fader's own
                // rule): a right-click / long-press on a control the
                // effect can move live offers the lane and the curve.
                // The target is the effect's INDEX in this chain — see
                // `AutoTarget`'s docs for why there is no effect id.
                if let (Some(resp), Some(key)) = (resp.as_ref(), studio_core::ParamKey::new(&spec.key)) {
                    if studio_fx::Instance::is_live_param(&fx.kind, &spec.key) {
                        let effect = ed.index.min(u8::MAX as usize) as u8;
                        let target = match ed.target {
                            Target::Bus(id) => studio_core::AutoTarget::BusParam { bus: id, effect, key },
                            Target::Clip(id) => studio_core::AutoTarget::ClipParam { clip: id, effect, key },
                        };
                        crate::mixer_panel::automate_menu(resp, s, target);
                    }
                }
                if let Some(v) = committed {
                    let mut ch = chain.clone();
                    ch[ed.index].params.insert(spec.key.clone(), spec.clamp(v));
                    // Dropping a count drops the params of the bands past it,
                    // so a later increase starts them from defaults rather
                    // than from whatever the old band 6 was.
                    if let Kind::Count { .. } = spec.kind {
                        let n = spec.clamp(v) as usize;
                        let keep: Vec<String> = schema::expand(&ch[ed.index]).unwrap_or_default().into_iter().map(|s| s.key).collect();
                        let _ = n;
                        ch[ed.index].params.retain(|k, _| keep.contains(k) || !k.chars().last().is_some_and(|c| c.is_ascii_digit()));
                    }
                    new_chain = Some(ch);
                }
            }
        });
    });

    if let Some(ch) = new_chain {
        commit(s, ed.target, ch);
    }
    ed.index = new_index;
    if open && !close {
        s.fx_editor = Some(ed);
    }
}

/// One control. Returns a value to COMMIT (release or click), else `None`;
/// the live drag value is kept in `drag`.
/// `resp_out` receives the slider's response for Float and Int kinds, so
/// the caller can hang a context menu on the control it does not draw.
fn control(ui: &mut egui::Ui, fx: &studio_core::Effect, spec: &ParamSpec, drag: &mut Option<(String, f32)>, narrow: bool,
           resp_out: &mut Option<egui::Response>) -> Option<f32> {
    let stored = schema::value(fx, spec);
    let mut commit = None;
    match &spec.kind {
        Kind::Float { min, max, scale, step, .. } => {
            let mut shown = match drag { Some((k, v)) if *k == spec.key => *v, _ => stored };
            let text = spec.format(shown);
            let mut slider = egui::Slider::new(&mut shown, *min..=*max).text(&spec.label)
                .custom_formatter(move |_, _| text.clone())
                .clamping(egui::SliderClamping::Always);
            if *scale == Scale::Log { slider = slider.logarithmic(true); }
            if *step > 0.0 { slider = slider.step_by(*step as f64); }
            let resp = ui.add(slider);
            if resp.double_clicked() { *drag = None; *resp_out = Some(resp); return Some(spec.default); }
            if resp.dragged() { *drag = Some((spec.key.clone(), shown)); }
            if resp.drag_stopped() { *drag = None; commit = Some(shown); }
            else if resp.changed() && !resp.dragged() { commit = Some(shown); }
            *resp_out = Some(resp);
        }
        Kind::Int { min, max, .. } => {
            let mut v = stored.round() as i32;
            let resp = ui.add(egui::Slider::new(&mut v, *min..=*max).text(&spec.label).clamping(egui::SliderClamping::Always));
            if resp.drag_stopped() || (resp.changed() && !resp.dragged()) { commit = Some(v as f32); }
            *resp_out = Some(resp);
        }
        Kind::Bool => {
            let mut on = stored > 0.5;
            if ui.checkbox(&mut on, &spec.label).changed() { commit = Some(if on { 1.0 } else { 0.0 }); }
        }
        Kind::Enum(names) => {
            ui.horizontal_wrapped(|ui| {
                ui.label(&spec.label);
                let cur = stored.round() as usize;
                if names.len() <= 6 || !narrow {
                    for (i, n) in names.iter().enumerate() {
                        if ui.add(egui::Button::selectable(i == cur, *n)).clicked() { commit = Some(i as f32); }
                    }
                } else {
                    egui::ComboBox::from_id_salt(("fx-enum", &spec.key)).selected_text(names.get(cur).copied().unwrap_or("?"))
                        .show_ui(ui, |ui| {
                            for (i, n) in names.iter().enumerate() {
                                if ui.selectable_label(i == cur, *n).clicked() { commit = Some(i as f32); }
                            }
                        });
                }
            });
        }
        Kind::Count { min, max } => {
            ui.horizontal(|ui| {
                let n = stored as usize;
                if ui.add_enabled(n > *min, egui::Button::new("−")).clicked() { commit = Some((n - 1) as f32); }
                ui.label(format!("{n} {}", spec.label));
                if ui.add_enabled(n < *max, egui::Button::new("+")).clicked() { commit = Some((n + 1) as f32); }
            });
        }
    }
    commit
}

/// Draw a chain as clickable chips; a click opens the editor on that
/// effect. Shared by the clip inspector and the mixer so both doors lead
/// to the same room.
pub fn chain_chips(ui: &mut egui::Ui, s: &mut Studio, target: Target, chain: &[studio_core::Effect], c: &PaletteColors) {
    for (i, fx) in chain.iter().enumerate() {
        let label = if fx.enabled { fx.kind.clone() } else { format!("{} (off)", fx.kind) };
        let selected = matches!(&s.fx_editor, Some(e) if e.target == target && e.index == i);
        let resp = ui.add(egui::Button::selectable(selected, label));
        if resp.clicked() {
            s.fx_editor = Some(FxEditor::open(target, i));
        }
        resp.on_hover_text(schema::expand(fx).map(|specs| specs.iter().filter(|sp| !sp.is_count())
            .map(|sp| format!("{}: {}", sp.label, sp.format(schema::value(fx, sp)))).collect::<Vec<_>>().join("\n"))
            .unwrap_or_else(|| "(no schema)".into()));
    }
    if chain.is_empty() { ui.colored_label(c.text_weak, "no effects"); }
}
