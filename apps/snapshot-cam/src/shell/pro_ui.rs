//! The pro panel (lens, crop, EV, WB, aids, the camera's own controls) and
//! the aids drawn over the viewfinder (zebras, peaking, grid, histogram,
//! the readout). Widgets only: every number comes from `pro`, `develop`
//! and `hwctl`.

use super::{Cam, ViewMode};
use crate::bracket::Mode;
use crate::cover;
use crate::develop::{self, WhiteBalance, WB_PRESETS};
use crate::hwctl::{self, Role};
use crate::lens::{self, AutoCrop};
use egui::{Color32, Pos2, Rect, Stroke};

impl Cam {
    /// Paint the aids over the picture. `shown` is the whole image rect,
    /// `visible` the part on screen, `uv` the texture rect the picture used
    /// (so the zebra/peaking layer mirrors with it).
    pub(super) fn draw_aids(&self, ui: &egui::Ui, shown: Rect, visible: Rect, uv: Rect) {
        let p = ui.painter().with_clip_rect(visible);
        let o = self.pro.overlays;
        if self.developed {
            if let Some(t) = &self.overlay { p.image(t.id(), shown, uv, Color32::WHITE); }
        }
        if o.grid {
            let s = Stroke::new(1.0_f32, Color32::from_white_alpha(90));
            for k in [1.0 / 3.0, 2.0 / 3.0] {
                let x = shown.left() + shown.width() * k;
                let y = shown.top() + shown.height() * k;
                p.line_segment([Pos2::new(x, visible.top()), Pos2::new(x, visible.bottom())], s);
                p.line_segment([Pos2::new(visible.left(), y), Pos2::new(visible.right(), y)], s);
            }
        }
        let analysis = self.measured.as_ref().map(|m| &m.analysis);
        if o.histogram {
            if let Some(a) = analysis { histogram(&p, a, Rect::from_min_size(visible.min + egui::vec2(8.0, 8.0), egui::vec2(132.0, 54.0))); }
        }
        if self.pro.develops() || analysis.is_some() {
            let mut text = self.pro.readout(analysis.map(|a| a.meter_ev));
            if let Some(a) = analysis { if a.clipped > 0.01 { text += &format!(" · clipped {:.0}%", a.clipped * 100.0); } }
            let font = egui::FontId::proportional(11.0);
            let g = p.layout(text, font, Color32::WHITE, (visible.width() - 24.0).max(40.0));
            let at = Pos2::new(visible.left() + 8.0, visible.bottom() - g.size().y - 10.0);
            p.rect_filled(Rect::from_min_size(at - egui::vec2(5.0, 3.0), g.size() + egui::vec2(10.0, 6.0)), 4.0, Color32::from_black_alpha(150));
            p.galley(at, g, Color32::WHITE);
        }
    }

    /// The panel under the controls.
    pub(super) fn pro_panel(&mut self, ui: &mut egui::Ui) {
        let c = self.colors.clone();
        egui::Frame::group(ui.style()).show(ui, |ui| {
            // LENS: profile, autocrop, lock, save.
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(c.text_weak, "lens");
                let current = self.pro.profile().name.clone();
                let mut pick = None;
                egui::ComboBox::from_id_salt("lens").selected_text(&current).width(150.0).show_ui(ui, |ui| {
                    for (i, l) in self.pro.lenses.iter().enumerate() {
                        if ui.selectable_label(i == self.pro.lens, &l.name).on_hover_text(&l.source).clicked() { pick = Some(i); }
                    }
                });
                if let Some(i) = pick {
                    self.pro.select_lens(i);
                    self.settings_dirty = true;
                    // The lens's focus, through the camera's own control.
                    if let Some(ctl) = self.hw.controls.clone() {
                        let pairs = self.pro.lens_focus(&ctl);
                        if !pairs.is_empty() { self.apply_hw(&ctl.device, &pairs); }
                    }
                }
                let mut auto = self.pro.profile().autocrop == AutoCrop::Auto;
                if ui.checkbox(&mut auto, "autocrop").on_hover_text("find the lens's black border and crop it away").changed() {
                    self.pro.profile_mut().autocrop = if auto { AutoCrop::Auto } else { AutoCrop::Off };
                    self.pro.measured_crop = None;
                }
                if self.pro.profile().crop.is_some() {
                    if ui.small_button("unlock").on_hover_text("measure the border again").clicked() { self.pro.unlock_crop(); }
                } else if self.pro.measuring() && self.pro.measured_crop.is_some()
                    && ui.small_button("lock").on_hover_text("keep this crop for this lens").clicked() {
                    self.pro.lock_crop();
                }
                if ui.small_button("save").on_hover_text(format!("write this profile to {}", lens::lens_dir().display())).clicked() {
                    if let Some(ctl) = self.hw.controls.clone() { self.pro.capture_focus(&ctl); }
                    let prof = self.pro.profile().clone();
                    self.note = match lens::save(&lens::lens_dir(), &prof) {
                        Ok(path) => { self.pro.profile_mut().source = path.display().to_string(); format!("lens profile saved: {}", path.display()) }
                        Err(e) => format!("could not save the lens profile: {e}"),
                    };
                }
            });
            if !self.pro.profile().note.is_empty() {
                ui.add(egui::Label::new(egui::RichText::new(&self.pro.profile().note).small().color(c.text_weak)).wrap());
            }
            // EXPOSURE COMPENSATION and WHITE BALANCE. `horizontal`, not
            // wrapped: a label after the first in a wrapped row is laid out
            // as a galley starting at the row's left edge, which is where
            // the layout probe (and a screen reader's bounds) find it.
            ui.horizontal(|ui| {
                ui.colored_label(c.text_weak, "EV");
                if ui.small_button("−").on_hover_text("⅓ stop darker").clicked() { self.pro.nudge_ev(-1); self.settings_dirty = true; }
                ui.monospace(self.pro.ev_label());
                if ui.small_button("+").on_hover_text("⅓ stop brighter").clicked() { self.pro.nudge_ev(1); self.settings_dirty = true; }
                ui.add_space(8.0);
                ui.colored_label(c.text_weak, "WB");
                let mut wb = self.pro.wb;
                egui::ComboBox::from_id_salt("wb").selected_text(wb.label()).width(120.0).show_ui(ui, |ui| {
                    ui.selectable_value(&mut wb, WhiteBalance::AsShot, "as shot");
                    ui.selectable_value(&mut wb, WhiteBalance::Auto, "auto");
                    for (name, k) in WB_PRESETS { ui.selectable_value(&mut wb, WhiteBalance::Kelvin(k), format!("{name} {k}K")); }
                });
                if let WhiteBalance::Kelvin(mut k) = wb {
                    if ui.add(egui::DragValue::new(&mut k).range(2000..=10000).speed(20).suffix("K")).changed() { wb = WhiteBalance::Kelvin(k); }
                }
                if wb != self.pro.wb { self.pro.wb = wb; self.settings_dirty = true; }
            });
            // AIDS.
            ui.horizontal_wrapped(|ui| {
                let o = &mut self.pro.overlays;
                let before = *o;
                ui.checkbox(&mut o.histogram, "histogram");
                ui.checkbox(&mut o.zebra, "zebras").on_hover_text(format!("stripes where a channel reaches {}", develop::ZEBRA_AT));
                ui.checkbox(&mut o.peaking, "peaking").on_hover_text("marks what is in focus");
                ui.checkbox(&mut o.grid, "grid");
                if *o != before { self.settings_dirty = true; }
            });
            // SHOTS: single, or a multi-frame merge.
            ui.horizontal(|ui| {
                ui.colored_label(c.text_weak, "shots");
                let mut mode = self.flow.mode;
                egui::ComboBox::from_id_salt("mode").selected_text(mode.label()).width(110.0).show_ui(ui, |ui| {
                    for m in Mode::ALL { ui.selectable_value(&mut mode, m, m.label()); }
                });
                if mode != self.flow.mode && !self.flow.busy() { self.flow.mode = mode; self.settings_dirty = true; }
                if ui.checkbox(&mut self.pro.fusion, "fusion (shadows)").on_hover_text("blend virtual exposures of each saved photo: lifts shadows and subjects; cannot recover burnt highlights").changed() { self.settings_dirty = true; }
                if ui.checkbox(&mut self.pro.auto_tone, "auto tone (saved photos)").on_hover_text("black point, white balance and exposure bias measured from each photo").changed() { self.settings_dirty = true; }
                if let Err(why) = self.flow.mode.available(self.hw.controls.as_ref()) {
                    ui.add(egui::Label::new(egui::RichText::new(why).small().color(c.warning)).truncate());
                }
            });
            // TRIGGERS: a press or a shake primes the shutter for a second.
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(c.text_weak, "trigger");
                let t = &mut self.flow.triggers;
                let before = *t;
                ui.checkbox(&mut t.volume, "volume keys").on_hover_text("either volume key primes the shutter; the sharpest frame in the next second is kept");
                ui.checkbox(&mut t.power, "power key").on_hover_text("held from the system while on (systemd-inhibit)");
                ui.checkbox(&mut t.shake, "shake");
                if *t != before { self.settings_dirty = true; self.apply_triggers(); }
            });
            if !self.flow.report.is_empty() {
                ui.add(egui::Label::new(egui::RichText::new(self.flow.report.join(" · ")).small().color(c.text_weak)).truncate());
            }
            // COVER: the plastic or glass over the camera.
            ui.horizontal(|ui| {
                ui.colored_label(c.text_weak, "cover");
                let measured = self.pro.cover != cover::CoverProfile::none();
                if ui.add_enabled(measured, egui::Checkbox::new(&mut self.pro.cover_on, "correct")).changed() { self.settings_dirty = true; }
                if ui.small_button("calibrate").on_hover_text("point at a grey or white card filling the middle, with a shadow or something black in the frame").clicked() {
                    self.calibrate_cover();
                }
                if measured {
                    let g = self.pro.cover.gains;
                    ui.add(egui::Label::new(egui::RichText::new(format!("R×{:.2} B×{:.2} haze {:.1}%", g[0], g[2], self.pro.cover.flare[1] * 100.0)).small().color(c.text_weak)).truncate());
                }
            });
            // VIEW: full screen or compact.
            ui.horizontal(|ui| {
                ui.colored_label(c.text_weak, "view");
                for v in ViewMode::ALL {
                    if ui.selectable_label(self.layout == v, v.label()).clicked() && self.layout != v {
                        self.layout = v;
                        self.settings_dirty = true;
                    }
                }
            });
            self.hw_rows(ui);
        });
    }

    /// The camera's own exposure, gain, WB and focus, when it is a V4L2
    /// device that offers them; one line saying why not otherwise.
    fn hw_rows(&mut self, ui: &mut egui::Ui) {
        let c = self.colors.clone();
        let Some(ctl) = self.hw.controls.clone() else {
            let why = match (&self.hw.error, &self.hw.checked) {
                (Some(e), _) => e.clone(),
                (None, Some(None)) | (None, None) => "the source is not a V4L2 device (PipeWire, a gst: override or the test pattern)".into(),
                (None, Some(Some(d))) => format!("{d}: none"),
            };
            // A phone's voice-coil lens is its own device, not a control on the camera's node: a fine-tune slider for it.
            if self.lens_fine_row(ui) { return; }
            ui.add(egui::Label::new(egui::RichText::new(format!("camera controls: {why}")).small().color(c.text_weak)).truncate());
            return;
        };
        let mut apply: Option<Vec<(String, i64)>> = None;
        ui.horizontal_wrapped(|ui| {
            ui.colored_label(c.text_weak, format!("camera {}", ctl.device));
            if let Some(ae) = ctl.get(Role::AutoExposure) {
                let manual = ae.value == hwctl::AE_MANUAL;
                if ui.selectable_label(!manual, "auto exposure").clicked() && manual {
                    apply = Some(ctl.plan_set(Role::AutoExposure, hwctl::AE_AUTO));
                }
            }
            for (role, auto) in [(Role::AutoWb, "auto WB"), (Role::AutoFocus, "autofocus")] {
                if let Some(a) = ctl.get(role) {
                    if ui.selectable_label(a.value != 0, auto).clicked() { apply = Some(ctl.plan_set(role, if a.value != 0 { 0 } else { 1 })); }
                }
            }
        });
        for role in [Role::Exposure, Role::Gain, Role::WbTemp, Role::Focus] {
            let Some(k) = ctl.get(role) else { continue };
            ui.horizontal(|ui| {
                let mut v = k.value;
                let label = match role { Role::Exposure => hwctl::shutter_label(v), Role::WbTemp => format!("{v}K"), _ => v.to_string() };
                ui.colored_label(c.text_weak, format!("{} {label}", role.label()));
                let mut slider = egui::Slider::new(&mut v, k.min..=k.max).show_value(false).step_by(k.step as f64);
                if role == Role::Exposure { slider = slider.logarithmic(true); }
                let r = ui.add(slider);
                if r.drag_stopped() || (r.changed() && !r.dragged()) { apply = Some(ctl.plan_set(role, v)); }
            });
        }
        if let Some(pairs) = apply { self.apply_hw(&ctl.device, &pairs); }
    }

    /// The lens motor's position as a slider, to nudge focus after a tap or autofocus. Returns whether there is a
    /// lens motor to show (a fixed-focus camera has none). Dragging stops the automatic focus from fighting it.
    fn lens_fine_row(&mut self, ui: &mut egui::Ui) -> bool {
        if self.ensure_lens().is_err() { return false; }
        let c = self.colors.clone();
        let Some((_, lens, act)) = &mut self.focus_lens else { return false };
        let (min, max, step) = act.range;
        let mut v = lens.current().unwrap_or((min + max) / 2).clamp(min, max);
        let mut moved = None;
        ui.horizontal(|ui| {
            ui.colored_label(c.text_weak, format!("focus {v}"));
            let r = ui.add(egui::Slider::new(&mut v, min..=max).show_value(false).step_by(step.max(1) as f64));
            if r.changed() { moved = Some(v); }
            for (label, d) in [("−", -(step.max(1) * 4)), ("+", step.max(1) * 4)] {
                if ui.small_button(label).on_hover_text("a small nudge").clicked() { moved = Some((v + d).clamp(min, max)); }
            }
        });
        if let Some(p) = moved {
            if let Some((_, lens, _)) = &mut self.focus_lens { let _ = lens.move_to(p); }
            self.autofocus.cancel();
            self.focus.cancel();
            self.note = format!("focus fine-tune: lens {p}");
        }
        true
    }

    /// Set camera controls, keep the panel's copy in step, say what happened.
    pub(super) fn apply_hw(&mut self, device: &str, pairs: &[(String, i64)]) {
        match hwctl::apply_with(&hwctl::program(), device, pairs) {
            Ok(()) => {
                if let Some(c) = &mut self.hw.controls { c.note_applied(pairs); }
                self.note = format!("camera: {}", pairs.iter().map(|(n, v)| format!("{n}={v}")).collect::<Vec<_>>().join(" "));
            }
            Err(e) => self.note = e,
        }
    }
}

/// Luma as filled bars, R/G/B as lines — a camera's RGB histogram.
fn histogram(p: &egui::Painter, a: &develop::Analysis, r: Rect) {
    p.rect_filled(r, 4.0, Color32::from_black_alpha(140));
    let inner = r.shrink(4.0);
    let peak = a.hist.iter().flat_map(|h| h.iter()).copied().max().unwrap_or(1).max(1) as f32;
    let n = develop::BINS as f32;
    let bw = inner.width() / n;
    for (i, v) in a.hist[0].iter().enumerate() {
        let h = (*v as f32 / peak).sqrt() * inner.height();
        let x = inner.left() + i as f32 * bw;
        p.rect_filled(Rect::from_min_max(Pos2::new(x, inner.bottom() - h), Pos2::new(x + bw, inner.bottom())), 0.0, Color32::from_white_alpha(70));
    }
    for (ch, col) in [(1, Color32::from_rgb(255, 80, 80)), (2, Color32::from_rgb(80, 230, 80)), (3, Color32::from_rgb(90, 140, 255))] {
        let pts: Vec<Pos2> = a.hist[ch].iter().enumerate().map(|(i, v)| {
            Pos2::new(inner.left() + (i as f32 + 0.5) * bw, inner.bottom() - (*v as f32 / peak).sqrt() * inner.height())
        }).collect();
        p.add(egui::Shape::line(pts, Stroke::new(1.0_f32, col)));
    }
}
