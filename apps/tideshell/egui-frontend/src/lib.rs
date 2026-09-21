//! tideshell-egui — paint and input over `tideshell-core`.
//!
//! [`ShellView::ui`] draws every shell surface into one `Ui` and returns the
//! engine's Actions for the caller to execute (see [`effects`]). Raw input is
//! routed before any widget sees it:
//!
//! * a touch that starts in an edge zone belongs to the edge recognizer for
//!   its whole sequence (what a layer-shell edge strip gets on the phone);
//! * a touch that starts on the trackpad pad belongs to the trackpad;
//! * everything else is left to ordinary egui widgets (tiles, cards, bar).
//!
//! In production each surface becomes its own layer surface; in the simulator
//! they share one window. The painting code does not care which.

pub mod effects;

use egui::{Align2, Color32, CornerRadius, FontId, Id, Pos2, Rect, Sense, Stroke, Vec2};
use tideshell_core::bar::{self, BarWidget};
use tideshell_core::control_center::{QuickKind, SliderKind, Stage};
use tideshell_core::hold::HoldOption;
use tideshell_core::prefs::{Layout, ShellPrefs};
use tideshell_core::switcher::Style;
use tideshell_core::{Action, Shell, TouchEvent};
use uikit_egui::PaletteColors;

/// An app the bar can show: a hatch-core desktop entry, reduced.
#[derive(Debug, Clone, PartialEq)]
pub struct AppTile {
    pub id: String,
    pub name: String,
}

/// Per-frame facts the view shows but does not own.
pub struct FrameInfo<'a> {
    /// hatch favorites (or top-ranked entries), for bar app widgets.
    pub apps: &'a [AppTile],
    pub clock: String,
    pub battery: Option<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Owner {
    Edge,
    Pad,
}

#[derive(Default)]
struct SettingsSheet {
    open: bool,
    draft: Option<ShellPrefs>,
    pick: usize,
    notes: Vec<String>,
}

#[derive(Default)]
pub struct ShellView {
    pub show_trace: bool,
    owners: Vec<(i32, Owner)>,
    touch_seen: bool,
    mouse_down: bool,
    card_drag: f32,
    sheet: SettingsSheet,
}

const MOUSE_SLOT: i32 = -1;

fn cr(r: u8) -> CornerRadius {
    CornerRadius::same(r)
}

fn with_alpha(c: Color32, a: f32) -> Color32 {
    let [r, g, b, _] = c.to_srgba_unmultiplied();
    Color32::from_rgba_unmultiplied(r, g, b, (a.clamp(0.0, 1.0) * 255.0) as u8)
}

/// Stable, distinct colour per app id, for letter tiles and fake thumbnails.
pub fn app_color(app_id: &str) -> Color32 {
    let h = app_id.bytes().fold(2166136261u32, |h, b| (h ^ b as u32).wrapping_mul(16777619));
    let hue = (h % 360) as f32 / 360.0;
    egui::ecolor::Hsva::new(hue, 0.45, 0.62, 1.0).into()
}

fn initial(name: &str) -> String {
    name.chars().find(|c| c.is_alphanumeric()).map(|c| c.to_uppercase().collect()).unwrap_or_else(|| "?".into())
}

impl ShellView {
    /// Geometry of the trackpad pad in screen-local coordinates.
    pub fn pad_rect(screen: Vec2, bar_w: f32) -> Rect {
        let h = (screen.y * 0.38).max(180.0);
        Rect::from_min_max(Pos2::new(12.0, screen.y - h - 20.0), Pos2::new(screen.x - bar_w - 12.0, screen.y - 20.0))
    }

    fn bar_width(shell: &Shell) -> f32 {
        match shell.prefs.layout {
            Layout::RightBar if shell.bar_expanded => shell.prefs.bar_width * 2.6,
            Layout::RightBar => shell.prefs.bar_width,
            Layout::PullDown => 0.0,
        }
    }

    /// Width the bar reserves (its exclusive zone). The simulator lays its
    /// fake windows out beside it, as maximized windows would be.
    pub fn reserved_right(shell: &Shell) -> f32 {
        match shell.prefs.layout {
            Layout::RightBar => shell.prefs.bar_width,
            Layout::PullDown => 0.0,
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, shell: &mut Shell, pal: &PaletteColors, info: &FrameInfo) -> Vec<Action> {
        let area = ui.max_rect();
        let origin = area.min;
        let size = area.size();
        if (shell.screen.0 - size.x).abs() > 0.5 || (shell.screen.1 - size.y).abs() > 0.5 {
            shell.set_screen((size.x, size.y));
        }
        let now = (ui.input(|i| i.time) * 1000.0) as u64;
        let mut out = shell.tick(now);
        out.extend(self.route_input(ui, shell, origin, now));

        let p = ui.painter_at(area);
        let to = |x: f32, y: f32| origin + Vec2::new(x, y);

        // Bottom layer first: bar, then overlays above it.
        if shell.prefs.layout == Layout::RightBar {
            out.extend(self.bar(ui, shell, pal, info, origin, size));
        }
        if shell.modes.trackpad {
            let r = Self::pad_rect(size, Self::bar_width(shell)).translate(origin.to_vec2());
            p.rect_filled(r, cr(22), with_alpha(pal.bg_raised, 0.92));
            p.text(r.center_top() + Vec2::new(0.0, 18.0), Align2::CENTER_CENTER, "Trackpad", FontId::proportional(13.0), pal.text_weak);
            let n = shell.trackpad.finger_count();
            if n > 0 {
                p.text(r.center(), Align2::CENTER_CENTER, format!("{n} finger{}", if n > 1 { "s" } else { "" }), FontId::proportional(15.0), pal.accent);
            }
        }
        self.previews(&p, shell, pal, origin, size);
        if shell.switcher.open {
            out.extend(self.switcher(ui, shell, pal, origin, size));
        }
        if shell.drawer.is_visible() {
            out.extend(self.drawer(ui, shell, pal, origin, size));
        }
        if shell.carousel.is_active() {
            let (i, n) = shell.carousel.position();
            let w = n as f32 * 14.0;
            for k in 0..n {
                let c = to(size.x / 2.0 - w / 2.0 + k as f32 * 14.0 + 7.0, 12.0);
                p.circle_filled(c, if k == i { 4.5 } else { 3.0 }, if k == i { pal.accent } else { pal.text_weak });
            }
        }
        if let Some(h) = &shell.hold {
            let o = to(h.origin.0, h.origin.1);
            p.circle_filled(o, h.radius + h.hit, with_alpha(pal.bg_base, 0.55));
            for (opt, (x, y)) in HoldOption::ALL.into_iter().zip(h.centers) {
                let c = to(x, y);
                let on = match opt {
                    HoldOption::Carousel => shell.carousel.is_active(),
                    HoldOption::Keyboard => shell.modes.keyboard,
                    HoldOption::Trackpad => shell.modes.trackpad,
                };
                let hot = h.hover == Some(opt);
                p.line_segment([o, c], Stroke::new(1.0, with_alpha(pal.text_weak, 0.4)));
                p.circle_filled(c, if hot { h.hit } else { h.hit * 0.82 }, if hot { pal.accent } else { pal.bg_raised });
                if on {
                    p.circle_stroke(c, h.hit * 0.82, Stroke::new(2.5, pal.success));
                }
                p.text(c, Align2::CENTER_CENTER, opt.label(), FontId::proportional(12.0), pal.text);
            }
        }
        // Home indicator: the affordance for every bottom-edge gesture.
        let hw = 110.0 + shell.swipe_up * 40.0;
        let hy = size.y - 9.0 - shell.swipe_up * 30.0;
        p.rect_filled(Rect::from_center_size(to(size.x / 2.0 + shell.peek_dx * 0.3, hy), Vec2::new(hw, 5.0)), cr(3), with_alpha(pal.text, 0.75));

        if let Some(prefs) = self.settings_sheet(ui.ctx(), shell) {
            out.extend(shell.set_prefs(prefs));
        }
        if self.show_trace {
            self.trace(&p, shell, pal, origin);
        }
        if !shell.recognizer.is_idle() || shell.drawer.is_animating() || shell.switcher.is_streaking() {
            ui.ctx().request_repaint();
        }
        out
    }

    // ---- input ---------------------------------------------------------------

    fn owner(&self, slot: i32) -> Option<Owner> {
        self.owners.iter().find(|(s, _)| *s == slot).map(|(_, o)| *o)
    }

    fn route_input(&mut self, ui: &egui::Ui, shell: &mut Shell, origin: Pos2, now: u64) -> Vec<Action> {
        let events = ui.input(|i| i.events.clone());
        let mut out = vec![];
        let size = Vec2::new(shell.screen.0, shell.screen.1);
        for ev in events {
            let (slot, phase, pos) = match ev {
                egui::Event::Touch { id, phase, pos, .. } => {
                    self.touch_seen = true;
                    (id.0 as i32, phase, pos)
                }
                // egui also synthesizes pointer events from touches; once a
                // real touch has been seen, those duplicates are ignored.
                _ if self.touch_seen => continue,
                egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, .. } => {
                    self.mouse_down = pressed;
                    (MOUSE_SLOT, if pressed { egui::TouchPhase::Start } else { egui::TouchPhase::End }, pos)
                }
                egui::Event::PointerMoved(pos) if self.mouse_down => (MOUSE_SLOT, egui::TouchPhase::Move, pos),
                _ => continue,
            };
            let l = pos - origin;
            let (x, y) = (l.x, l.y);
            let te = match phase {
                egui::TouchPhase::Start => TouchEvent::Down { slot, x, y, t: now },
                egui::TouchPhase::Move => TouchEvent::Move { slot, x, y, t: now },
                egui::TouchPhase::End => TouchEvent::Up { slot, t: now },
                egui::TouchPhase::Cancel => TouchEvent::Cancel,
            };
            if phase == egui::TouchPhase::Start {
                let pad = Self::pad_rect(size, Self::bar_width(shell));
                let modal = shell.drawer.is_visible() || shell.switcher.open || self.sheet.open;
                let owner = if shell.modes.trackpad && !modal && pad.contains(Pos2::new(x, y)) {
                    Some(Owner::Pad)
                } else if shell.recognizer.edge_at(x, y).is_some() && !self.sheet.open {
                    Some(Owner::Edge)
                } else {
                    None
                };
                if let Some(o) = owner {
                    self.owners.retain(|(s, _)| *s != slot);
                    self.owners.push((slot, o));
                }
            }
            match (self.owner(slot), te) {
                (Some(Owner::Edge), e) => out.extend(shell.on_edge_touch(e)),
                (Some(Owner::Pad), e) => out.extend(shell.on_trackpad_touch(e)),
                (None, TouchEvent::Cancel) => {
                    out.extend(shell.on_edge_touch(TouchEvent::Cancel));
                    out.extend(shell.on_trackpad_touch(TouchEvent::Cancel));
                }
                _ => {}
            }
            if matches!(phase, egui::TouchPhase::End | egui::TouchPhase::Cancel) {
                self.owners.retain(|(s, _)| *s != slot);
            }
        }
        out
    }

    // ---- previews --------------------------------------------------------------

    fn previews(&self, p: &egui::Painter, shell: &Shell, pal: &PaletteColors, origin: Pos2, size: Vec2) {
        let cfg = &shell.recognizer.cfg;
        if shell.swipe_up > 0.0 {
            p.rect_filled(Rect::from_min_size(origin, size), cr(0), Color32::from_black_alpha((shell.swipe_up * 360.0).min(150.0) as u8));
            // Say what lifting now would do: the recognizer's thresholds,
            // made visible, so the gesture can be learned rather than guessed.
            let label = if shell.swipe_up >= cfg.launcher_commit {
                "Release for apps (hatch)"
            } else if shell.swipe_up >= cfg.switcher_commit {
                "Release for switcher - keep going for apps"
            } else {
                "Keep swiping up"
            };
            p.text(origin + Vec2::new(size.x / 2.0, size.y * (1.0 - shell.swipe_up) - 40.0), Align2::CENTER_CENTER, label, FontId::proportional(15.0), pal.text);
        }
        if shell.peek_dx.abs() > 1.0 {
            let label = if shell.carousel.is_active() {
                if shell.peek_dx > 0.0 { "< previous window" } else { "next window >" }
            } else if shell.peek_dx > 0.0 {
                "< previous app"
            } else {
                "more recent app >"
            };
            let reached = shell.peek_dx.abs() >= cfg.quick_switch_px;
            p.text(origin + Vec2::new(size.x / 2.0, size.y - 40.0), Align2::CENTER_CENTER, label, FontId::proportional(14.0), if reached { pal.accent } else { pal.text_weak });
        }
    }

    // ---- drawer ------------------------------------------------------------------

    fn drawer(&mut self, ui: &mut egui::Ui, shell: &mut Shell, pal: &PaletteColors, origin: Pos2, size: Vec2) -> Vec<Action> {
        let mut out = vec![];
        let full_h = (size.y * 0.66).min(620.0);
        let panel_h = shell.drawer.progress * full_h;
        let panel = Rect::from_min_size(origin, Vec2::new(size.x, panel_h));
        // Backdrop below the panel: tap closes.
        let back = Rect::from_min_max(panel.left_bottom(), origin + size);
        let p = ui.painter_at(Rect::from_min_size(origin, size));
        p.rect_filled(back, cr(0), Color32::from_black_alpha((shell.drawer.progress * 140.0) as u8));
        if ui.interact(back, Id::new("ts-drawer-back"), Sense::click()).clicked() {
            shell.dismiss_overlays();
        }
        p.rect_filled(panel, CornerRadius { nw: 0, ne: 0, sw: 22, se: 22 }, with_alpha(pal.bg_card, 0.97));
        let pc = ui.painter_at(panel);
        pc.text(origin + Vec2::new(18.0, 22.0), Align2::LEFT_CENTER, &String::from("Quick settings"), FontId::proportional(13.0), pal.text_weak);

        let pad = 14.0;
        let cols = 5.0;
        let gap = 10.0;
        let tw = (size.x - pad * 2.0 - gap * (cols - 1.0)) / cols;
        let th = 72.0;
        let tiles: Vec<QuickKind> = shell.drawer.tiles.clone();
        let visible = shell.drawer.stage() != Stage::Closed;
        for (i, kind) in tiles.iter().enumerate() {
            let (col, row) = ((i % 5) as f32, (i / 5) as f32);
            let r = Rect::from_min_size(origin + Vec2::new(pad + col * (tw + gap), 40.0 + row * (th + gap)), Vec2::new(tw, th));
            if r.top() > panel.bottom() {
                continue;
            }
            let on = shell.quick_state(*kind);
            pc.rect_filled(r, cr(16), if on { pal.accent } else { pal.bg_raised });
            pc.text(r.center() - Vec2::new(0.0, 8.0), Align2::CENTER_CENTER, initial(kind.label()), FontId::proportional(20.0), if on { pal.bg_base } else { pal.text });
            pc.text(r.center_bottom() - Vec2::new(0.0, 13.0), Align2::CENTER_CENTER, short(kind.label()), FontId::proportional(10.0), if on { pal.bg_base } else { pal.text_weak });
            let hit = r.intersect(panel);
            if visible && hit.height() > 20.0 && ui.interact(hit, Id::new(("ts-tile", i)), Sense::click()).clicked() {
                out.extend(shell.tap_quick(*kind));
            }
        }
        let rows = (tiles.len() as f32 / 5.0).ceil();
        let mut y = 40.0 + rows * (th + gap) + 6.0;
        for kind in [SliderKind::Brightness, SliderKind::Volume] {
            let r = Rect::from_min_size(origin + Vec2::new(pad, y), Vec2::new(size.x - pad * 2.0, 40.0));
            y += 50.0;
            if r.top() > panel.bottom() {
                continue;
            }
            let v = shell.drawer.slider(kind);
            pc.rect_filled(r, cr(20), pal.bg_raised);
            pc.rect_filled(Rect::from_min_size(r.min, Vec2::new(r.width() * v, r.height())), cr(20), with_alpha(pal.accent, 0.45));
            pc.text(r.left_center() + Vec2::new(16.0, 0.0), Align2::LEFT_CENTER, kind.label(), FontId::proportional(13.0), pal.text);
            let resp = ui.interact(r.intersect(panel), Id::new(("ts-slider", kind.token())), Sense::click_and_drag());
            if let Some(pos) = resp.interact_pointer_pos().filter(|_| resp.dragged() || resp.clicked()) {
                out.extend(shell.set_slider(kind, (pos.x - r.left()) / r.width()));
            }
        }
        // Settings entry at the bottom of the full drawer.
        let r = Rect::from_min_size(origin + Vec2::new(pad, y + 4.0), Vec2::new(size.x - pad * 2.0, 36.0));
        if r.bottom() <= panel.bottom() {
            pc.rect_filled(r, cr(18), pal.bg_raised);
            pc.text(r.center(), Align2::CENTER_CENTER, "Shell settings", FontId::proportional(13.0), pal.text);
            if ui.interact(r, Id::new("ts-drawer-settings"), Sense::click()).clicked() {
                self.open_settings(shell);
            }
        }
        // Grab handle.
        pc.rect_filled(Rect::from_center_size(panel.center_bottom() - Vec2::new(0.0, 10.0), Vec2::new(40.0, 4.0)), cr(2), pal.text_weak);
        out
    }

    // ---- right bar ---------------------------------------------------------------

    fn bar(&mut self, ui: &mut egui::Ui, shell: &mut Shell, pal: &PaletteColors, info: &FrameInfo, origin: Pos2, size: Vec2) -> Vec<Action> {
        let mut out = vec![];
        let bw = Self::bar_width(shell);
        let expanded = shell.bar_expanded;
        let rect = Rect::from_min_size(origin + Vec2::new(size.x - bw, 0.0), Vec2::new(bw, size.y));
        let p = ui.painter_at(rect);
        p.rect_filled(rect, cr(0), pal.bg_card);
        let unit = shell.prefs.bar_width.min(60.0);
        let widgets = shell.prefs.bar_widgets.clone();
        let lay = bar::layout(&widgets, size.y - 28.0, unit);
        let cell = |y: f32, h: f32| Rect::from_min_size(rect.min + Vec2::new(4.0, 14.0 + y), Vec2::new(bw - 8.0, h));
        let icon_c = |r: Rect| if expanded { Pos2::new(r.left() + unit / 2.0, r.center().y) } else { r.center() };
        let label = |p: &egui::Painter, r: Rect, s: &str| {
            if expanded {
                p.text(Pos2::new(r.left() + unit + 4.0, r.center().y), Align2::LEFT_CENTER, s, FontId::proportional(12.0), pal.text);
            }
        };
        let app_name = |id: &str| info.apps.iter().find(|a| a.id == id).map(|a| a.name.clone()).unwrap_or_else(|| id.to_string());
        for pl in &lay.placed {
            let r = cell(pl.y, pl.h);
            let wid = Id::new(("ts-bar", pl.index));
            match &widgets[pl.index] {
                BarWidget::Clock => {
                    p.text(r.center(), Align2::CENTER_CENTER, &info.clock, FontId::proportional(if expanded { 22.0 } else { 15.0 }), pal.text);
                }
                BarWidget::Battery => {
                    let s = info.battery.map(|b| format!("{:.0}%", b * 100.0)).unwrap_or_else(|| "--".into());
                    p.text(r.center(), Align2::CENTER_CENTER, s, FontId::proportional(12.0), pal.text_weak);
                }
                BarWidget::Toggle(k) => {
                    let on = shell.quick_state(*k);
                    let c = icon_c(r);
                    p.circle_filled(c, unit * 0.36, if on { pal.accent } else { pal.bg_raised });
                    p.text(c, Align2::CENTER_CENTER, initial(k.label()), FontId::proportional(14.0), if on { pal.bg_base } else { pal.text });
                    label(&p, r, k.label());
                    if ui.interact(r, wid, Sense::click()).clicked() {
                        out.extend(shell.tap_quick(*k));
                    }
                }
                BarWidget::Slider(k) => {
                    let track = Rect::from_center_size(r.center(), Vec2::new(unit * 0.5, r.height() - 12.0));
                    let v = shell.drawer.slider(*k);
                    p.rect_filled(track, cr(12), pal.bg_raised);
                    let fill = Rect::from_min_max(Pos2::new(track.left(), track.bottom() - track.height() * v), track.max);
                    p.rect_filled(fill, cr(12), with_alpha(pal.accent, 0.45));
                    p.text(track.center_bottom() - Vec2::new(0.0, 12.0), Align2::CENTER_CENTER, initial(k.label()), FontId::proportional(12.0), pal.text);
                    let resp = ui.interact(track, wid, Sense::click_and_drag());
                    if let Some(pos) = resp.interact_pointer_pos().filter(|_| resp.dragged() || resp.clicked()) {
                        out.extend(shell.set_slider(*k, (track.bottom() - pos.y) / track.height()));
                    }
                }
                BarWidget::App(id) => {
                    let name = app_name(id);
                    if app_button(ui, &p, r, wid, &name, id, icon_c(r), unit, pal) {
                        out.extend(shell.launch(id));
                    }
                    label(&p, r, &name);
                }
                BarWidget::Favorites(n) => {
                    for (k, app) in info.apps.iter().take(*n as usize).enumerate() {
                        let rr = Rect::from_min_size(r.min + Vec2::new(0.0, k as f32 * unit), Vec2::new(r.width(), unit));
                        if app_button(ui, &p, rr, Id::new(("ts-fav", k)), &app.name, &app.id, icon_c(rr), unit, pal) {
                            out.extend(shell.launch(&app.id));
                        }
                        label(&p, rr, &app.name);
                    }
                }
                BarWidget::Running => {
                    let cards: Vec<(u64, String, String)> =
                        shell.switcher.cards().iter().map(|c| (c.id, c.title.clone(), c.app_id.clone())).collect();
                    let fit = ((r.height() / unit) as usize).max(1);
                    for (k, (id, title, app)) in cards.iter().take(fit).enumerate() {
                        let rr = Rect::from_min_size(r.min + Vec2::new(0.0, k as f32 * unit), Vec2::new(r.width(), unit));
                        if app_button(ui, &p, rr, Id::new(("ts-run", *id)), title, app, icon_c(rr), unit, pal) {
                            out.extend(shell.activate(*id));
                        }
                        label(&p, rr, title);
                    }
                }
                BarWidget::Hatch | BarWidget::Switcher | BarWidget::Settings => {
                    let (txt, lbl) = match &widgets[pl.index] {
                        BarWidget::Hatch => ("Apps", "All apps"),
                        BarWidget::Switcher => ("Tasks", "Switcher"),
                        _ => ("Set", "Settings"),
                    };
                    let c = icon_c(r);
                    p.rect_filled(Rect::from_center_size(c, Vec2::splat(unit * 0.72)), cr(12), pal.bg_raised);
                    p.text(c, Align2::CENTER_CENTER, txt, FontId::proportional(11.0), pal.text);
                    label(&p, r, lbl);
                    if ui.interact(r, wid, Sense::click()).clicked() {
                        match &widgets[pl.index] {
                            BarWidget::Hatch => out.extend(shell.open_launcher()),
                            BarWidget::Switcher => shell.open_switcher(),
                            _ => self.open_settings(shell),
                        }
                    }
                }
                BarWidget::Spacer => {}
            }
        }
        if !lay.overflow.is_empty() {
            p.text(rect.center_bottom() - Vec2::new(0.0, 8.0), Align2::CENTER_CENTER, format!("+{}", lay.overflow.len()), FontId::proportional(11.0), pal.warning);
        }
        out
    }

    // ---- switcher ------------------------------------------------------------------

    fn switcher(&mut self, ui: &mut egui::Ui, shell: &mut Shell, pal: &PaletteColors, origin: Pos2, size: Vec2) -> Vec<Action> {
        let mut out = vec![];
        let bw = Self::reserved_right(shell);
        let full = Rect::from_min_size(origin, Vec2::new(size.x - bw, size.y));
        let p = ui.painter_at(full);
        p.rect_filled(full, cr(0), with_alpha(pal.bg_base, 0.94));
        if ui.interact(full, Id::new("ts-sw-back"), Sense::click()).clicked() {
            shell.dismiss_overlays();
        }
        // Header actions: multitask-first means "close all" and "all apps"
        // are one tap away, not buried.
        let hdr = |x: f32, w: f32| Rect::from_min_size(origin + Vec2::new(x, 14.0), Vec2::new(w, 34.0));
        let (close_r, apps_r) = (hdr(14.0, 96.0), hdr(full.width() - 110.0, 96.0));
        for (r, t) in [(close_r, "Close all"), (apps_r, "All apps")] {
            p.rect_filled(r, cr(17), pal.bg_raised);
            p.text(r.center(), Align2::CENTER_CENTER, t, FontId::proportional(13.0), pal.text);
        }
        if ui.interact(close_r, Id::new("ts-sw-closeall"), Sense::click()).clicked() {
            out.extend(shell.switcher_close_all());
        }
        if ui.interact(apps_r, Id::new("ts-sw-apps"), Sense::click()).clicked() {
            out.extend(shell.open_launcher());
        }
        let top = 60.0;
        let area = Vec2::new(full.width(), size.y - top - 30.0);
        let rects = shell.switcher.layout(area.x, area.y);
        let cards: Vec<(u64, String, String, bool)> =
            shell.switcher.cards().iter().map(|c| (c.id, c.title.clone(), c.app_id.clone(), c.activated)).collect();
        if cards.is_empty() {
            p.text(full.center(), Align2::CENTER_CENTER, "No open windows", FontId::proportional(16.0), pal.text_weak);
        }
        let dragging = shell.switcher.dragging;
        for ((id, title, app, active), r) in cards.iter().zip(rects) {
            let dy = dragging.filter(|(d, _)| d == id).map(|(_, dy)| dy).unwrap_or(0.0);
            let rr = Rect::from_min_size(origin + Vec2::new(r.x, top + r.y + dy), Vec2::new(r.w, r.h));
            let fade = 1.0 - (-dy / (shell.switcher.close_px * 1.6)).clamp(0.0, 0.8);
            p.rect_filled(rr, cr(18), with_alpha(app_color(app), fade));
            if *active {
                p.rect_filled(Rect::from_min_size(rr.min, Vec2::new(rr.width(), 4.0)), cr(2), pal.accent);
            }
            p.text(rr.center(), Align2::CENTER_CENTER, initial(title), FontId::proportional(34.0), with_alpha(Color32::WHITE, 0.85 * fade));
            p.text(rr.center_bottom() + Vec2::new(0.0, 11.0), Align2::CENTER_CENTER, title, FontId::proportional(12.0), pal.text);
            let resp = ui.interact(rr, Id::new(("ts-card", *id)), Sense::click_and_drag());
            if resp.drag_started() {
                self.card_drag = 0.0;
            }
            if resp.dragged() {
                self.card_drag += resp.drag_delta().y;
                shell.switcher_drag(*id, self.card_drag);
            }
            if resp.drag_stopped() {
                out.extend(shell.switcher_release());
                self.card_drag = 0.0;
            } else if resp.clicked() {
                out.extend(shell.switcher_tap(*id));
            }
        }
        let style = match shell.switcher.style {
            Style::Grid => "grid",
            Style::Deck => "deck",
        };
        p.text(full.center_bottom() - Vec2::new(0.0, 36.0), Align2::CENTER_CENTER, format!("{} windows - {style} - swipe a card up to close", cards.len()), FontId::proportional(11.0), pal.text_weak);
        out
    }

    // ---- settings sheet -------------------------------------------------------------

    pub fn open_settings(&mut self, shell: &mut Shell) {
        shell.drawer.close();
        self.sheet.open = true;
        self.sheet.draft = Some(shell.prefs.clone());
        self.sheet.notes.clear();
    }

    pub fn settings_open(&self) -> bool {
        self.sheet.open
    }

    /// The settings sheet. Returns new prefs when the user applies.
    fn settings_sheet(&mut self, ctx: &egui::Context, shell: &Shell) -> Option<ShellPrefs> {
        if !self.sheet.open {
            return None;
        }
        let mut applied = None;
        let mut close = false;
        let catalog = bar::catalog();
        let sheet = &mut self.sheet;
        let draft = sheet.draft.get_or_insert_with(ShellPrefs::default);
        egui::Window::new("Shell settings")
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label("Layout");
                ui.radio_value(&mut draft.layout, Layout::PullDown, "Pull-down control center");
                ui.radio_value(&mut draft.layout, Layout::RightBar, "Right-hand bar");
                ui.separator();
                ui.label("Switcher");
                ui.horizontal(|ui| {
                    ui.radio_value(&mut draft.switcher_style, Style::Grid, "Grid");
                    ui.radio_value(&mut draft.switcher_style, Style::Deck, "Deck");
                });
                ui.separator();
                ui.add_enabled_ui(draft.layout == Layout::RightBar, |ui| {
                    ui.label("Bar widgets (top to bottom)");
                    let mut op: Option<(usize, i32)> = None;
                    egui::ScrollArea::vertical().id_salt("ts-bar-edit").max_height(260.0).show(ui, |ui| {
                        for (i, w) in draft.bar_widgets.iter().enumerate() {
                            ui.horizontal(|ui| {
                                ui.monospace(w.token());
                                if ui.small_button("Up").clicked() {
                                    op = Some((i, -1));
                                }
                                if ui.small_button("Down").clicked() {
                                    op = Some((i, 1));
                                }
                                if ui.small_button("Remove").clicked() {
                                    op = Some((i, 0));
                                }
                            });
                        }
                    });
                    match op {
                        Some((i, 0)) => bar::remove_widget(&mut draft.bar_widgets, i),
                        Some((i, d)) => {
                            let to = (i as i32 + d).clamp(0, draft.bar_widgets.len() as i32 - 1) as usize;
                            bar::move_widget(&mut draft.bar_widgets, i, to);
                        }
                        None => {}
                    }
                    // Same layout call the bar uses, so the warning cannot
                    // disagree with what will actually be drawn.
                    let unit = draft.bar_width.min(60.0);
                    let over = bar::layout(&draft.bar_widgets, shell.screen.1 - 28.0, unit).overflow;
                    if !over.is_empty() {
                        let names: Vec<String> = over.iter().map(|i| draft.bar_widgets[*i].token()).collect();
                        ui.colored_label(ui.visuals().warn_fg_color, format!("Won't fit on this screen: {}", names.join(", ")));
                    }
                    ui.horizontal(|ui| {
                        egui::ComboBox::from_id_salt("ts-bar-add")
                            .selected_text(catalog.get(sheet.pick).map(|w| w.token()).unwrap_or_default())
                            .show_ui(ui, |ui| {
                                for (i, w) in catalog.iter().enumerate() {
                                    ui.selectable_value(&mut sheet.pick, i, w.token());
                                }
                            });
                        if ui.button("Add").clicked() {
                            if let Some(w) = catalog.get(sheet.pick) {
                                draft.bar_widgets.push(w.clone());
                            }
                        }
                    });
                });
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("Apply").clicked() {
                        applied = Some(draft.clone());
                    }
                    if ui.button("Close").clicked() {
                        close = true;
                    }
                });
            });
        if close || applied.is_some() {
            sheet.open = false;
        }
        applied
    }

    // ---- trace -------------------------------------------------------------------------

    fn trace(&self, p: &egui::Painter, shell: &Shell, pal: &PaletteColors, origin: Pos2) {
        let lines: Vec<&String> = shell.trace().collect();
        let tail = &lines[lines.len().saturating_sub(16)..];
        // Bottom, above the home indicator: the top is where the drawer and
        // switcher header live, and the trace must not hide what it explains.
        let h = 26.0 + 14.0 * tail.len() as f32;
        let r = Rect::from_min_size(origin + Vec2::new(6.0, shell.screen.1 - h - 28.0), Vec2::new(shell.screen.0 - 12.0, h));
        p.rect_filled(r, cr(8), Color32::from_black_alpha(190));
        let head = format!(
            "edge:{} drawer:{:.2} kb:{} pad:{} carousel:{} streak:{}",
            shell.recognizer.phase_label(),
            shell.drawer.progress,
            shell.modes.keyboard as u8,
            shell.modes.trackpad as u8,
            shell.carousel.is_active() as u8,
            shell.switcher.is_streaking() as u8
        );
        p.text(r.min + Vec2::new(8.0, 12.0), Align2::LEFT_CENTER, head, FontId::monospace(10.5), pal.accent);
        for (i, l) in tail.iter().enumerate() {
            p.text(r.min + Vec2::new(8.0, 28.0 + 14.0 * i as f32), Align2::LEFT_CENTER, l.as_str(), FontId::monospace(10.0), Color32::from_gray(225));
        }
    }
}

fn short(label: &str) -> String {
    label.chars().take(9).collect()
}

#[allow(clippy::too_many_arguments)]
fn app_button(ui: &mut egui::Ui, p: &egui::Painter, r: Rect, id: Id, name: &str, app_id: &str, c: Pos2, unit: f32, _pal: &PaletteColors) -> bool {
    p.circle_filled(c, unit * 0.36, app_color(app_id));
    p.text(c, Align2::CENTER_CENTER, initial(name), FontId::proportional(14.0), Color32::WHITE);
    ui.interact(r, id, Sense::click()).clicked()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_color_is_stable_and_spreads() {
        assert_eq!(app_color("org.gnome.Console"), app_color("org.gnome.Console"));
        let distinct: std::collections::HashSet<_> =
            ["a", "b", "firefox", "org.gnome.Console", "cosmic-term"].iter().map(|s| app_color(s)).collect();
        assert!(distinct.len() >= 4);
    }

    #[test]
    fn pad_sits_bottom_and_clear_of_bar() {
        let r = ShellView::pad_rect(Vec2::new(408.0, 900.0), 64.0);
        assert!(r.right() <= 408.0 - 64.0 && r.bottom() < 900.0 && r.height() >= 180.0);
    }

    #[test]
    fn initial_skips_punctuation() {
        assert_eq!(initial("-cosmic"), "C");
        assert_eq!(initial(""), "?");
    }
}
