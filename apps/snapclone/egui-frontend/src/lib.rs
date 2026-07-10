//! snapclone-egui — the founder app, ported to egui, now *consuming* the systems it started.
//!
//! Live backgrounds and page/subpage navigation used to live inside snapclone's QML; they're
//! now shared uikit-egui capabilities ([`livebg`](uikit_egui::livebg), [`nav`](uikit_egui::nav)),
//! so this port pulls them from the kit instead of owning them — and localweather / av1tuner get
//! the same for free. On top: adaptive layout, a11y, swipe between pages, and live cross-app
//! settings. The camera/peer content renders on the carved surface; the snapclone-* engine
//! crates (audio/proto/net-intel) drive it on-device.

use egui::Ui;
use uikit_egui::{Adaptive, BgStyle, GestureLayer, LiveBackground, Nav, PaletteColors};

pub const PAGES: [&str; 3] = ["Camera", "Peers", "Settings"];

/// A connected peer (mirrors snapclone-proto; on-device the real roster feeds this).
#[derive(Debug, Clone)]
pub struct Peer {
    pub name: String,
    pub online: bool,
}

/// The snapclone view: page/subpage nav, a live background, the peer roster, and gesture/theme.
pub struct Snapclone {
    pub nav: Nav,
    pub bg: LiveBackground,
    pub peers: Vec<Peer>,
    gesture: GestureLayer,
}

impl Snapclone {
    pub fn new(peers: Vec<Peer>) -> Snapclone {
        Snapclone {
            nav: Nav::new(PAGES.len()),
            bg: LiveBackground::new(BgStyle::Aurora),
            peers,
            gesture: GestureLayer::horizontal_page(320.0),
        }
    }

    pub fn page_name(&self) -> &'static str {
        PAGES[self.nav.page()]
    }

    /// Swipe between top-level pages (unless drilled into a subpage — then swipe is off so the
    /// content can scroll/interact).
    pub fn swipe_page(&mut self, sign: i32) {
        if self.nav.can_go_back() {
            return;
        }
        let n = PAGES.len() as i32;
        let next = (self.nav.page() as i32 + sign).clamp(0, n - 1) as usize;
        self.nav.go_page(next);
    }

    /// Advance the animation; call once per frame with the frame delta.
    pub fn tick(&mut self, dt: f32) {
        self.bg.tick(dt);
    }

    /// Adopt cross-app settings + the swipe feel, and use the palette's background style pref.
    pub fn apply_settings(&mut self, ctx: &egui::Context, prefs: &settings_kit::Prefs) {
        uikit_egui::apply_prefs(ctx, prefs);
        *self.gesture.config_mut() = prefs.gesture_config(320.0);
    }

    /// Render: live background, then the current page/subpage.
    pub fn ui(&mut self, ui: &mut Ui, adaptive: &Adaptive) {
        let colors = uikit_egui::default_palette();
        let surface = ui.available_rect_before_wrap();

        // live background painted behind everything
        self.bg.paint(ui.painter(), surface, &colors);

        // swipe paging (top-level only)
        let frame = self.gesture.update(ui, surface, true);
        if let Some(dir) = frame.page_jump {
            self.swipe_page(dir.sign());
        }

        // page tabs on desktop; swipe hint on phone
        if !adaptive.is_compact() && !self.nav.can_go_back() {
            ui.horizontal(|ui| {
                for (i, name) in PAGES.iter().enumerate() {
                    if ui.selectable_label(i == self.nav.page(), *name).clicked() {
                        self.nav.go_page(i);
                    }
                }
            });
            ui.separator();
        }

        match self.nav.page() {
            0 => self.camera_page(ui, &colors),
            1 => self.peers_page(ui, &colors),
            _ => self.settings_page(ui, &colors),
        }
    }

    fn camera_page(&mut self, ui: &mut Ui, c: &PaletteColors) {
        self.nav.title_bar(ui, "Camera");
        // the carved preview surface (on-device: device-kit camera feed)
        let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 180.0), egui::Sense::hover());
        ui.painter().rect(rect, egui::CornerRadius::same(10), c.bg_card, egui::Stroke::new(1.0, c.accent_soft), egui::StrokeKind::Inside);
        ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, "live preview", egui::FontId::proportional(14.0), c.text_weak);
    }

    fn peers_page(&mut self, ui: &mut Ui, c: &PaletteColors) {
        self.nav.title_bar(ui, "Peers");
        if let Some(sel) = self.nav.subpage() {
            // subpage: peer detail
            if let Some(p) = self.peers.get(sel) {
                ui.colored_label(c.text, format!("{}  ·  {}", p.name, if p.online { "online" } else { "offline" }));
                ui.colored_label(c.text_weak, "Share photos, connect a session, or send a ping.");
            }
            return;
        }
        // roster: tap a peer to drill into its subpage
        for (i, p) in self.peers.clone().iter().enumerate() {
            let dot = if p.online { c.success } else { c.text_weak };
            if ui.horizontal(|ui| {
                ui.colored_label(dot, "●");
                ui.selectable_label(false, &p.name).clicked()
            }).inner {
                self.nav.push_subpage(i);
            }
        }
    }

    fn settings_page(&mut self, ui: &mut Ui, c: &PaletteColors) {
        self.nav.title_bar(ui, "Settings");
        ui.horizontal_wrapped(|ui| {
            ui.colored_label(c.text_weak, "background:");
            for s in BgStyle::all() {
                if ui.selectable_label(self.bg.style == s, s.as_str()).clicked() {
                    self.bg.style = s;
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> Snapclone {
        Snapclone::new(vec![
            Peer { name: "alice".into(), online: true },
            Peer { name: "bob".into(), online: false },
        ])
    }

    #[test]
    fn swipe_moves_between_pages() {
        let mut s = app();
        assert_eq!(s.page_name(), "Camera");
        s.swipe_page(1);
        assert_eq!(s.page_name(), "Peers");
        s.swipe_page(1);
        s.swipe_page(1); // clamp at Settings
        assert_eq!(s.page_name(), "Settings");
        s.swipe_page(-1);
        assert_eq!(s.page_name(), "Peers");
    }

    #[test]
    fn swipe_disabled_while_drilled_into_subpage() {
        let mut s = app();
        s.nav.go_page(1); // Peers
        s.nav.push_subpage(0); // into a peer detail
        s.swipe_page(1); // should be ignored so content interaction wins
        assert_eq!(s.page_name(), "Peers");
        assert_eq!(s.nav.subpage(), Some(0));
    }

    #[test]
    fn background_tick_and_style_switch() {
        let mut s = app();
        let p0 = s.bg.phase;
        s.tick(0.5);
        assert!(s.bg.phase > p0);
        s.bg.style = BgStyle::Waves;
        assert_eq!(s.bg.style, BgStyle::Waves);
    }

    #[test]
    fn apply_settings_adopts_feel() {
        let ctx = egui::Context::default();
        let mut s = app();
        let mut prefs = settings_kit::Prefs::default();
        prefs.gesture.animation = settings_kit::AnimationStyle::Heavy;
        s.apply_settings(&ctx, &prefs);
        assert_eq!(s.gesture.config_mut().takeover_lock_ms, 220.0);
    }
}
