//! bot-arena-egui — the arena dashboard, ported to egui with swipe navigation.
//!
//! The GTK4/libadwaita UI becomes an egui dashboard whose views (Arena / Standings /
//! Replays) are **swipe-paged** using the shared gesture layer (`uikit-egui::GestureLayer`
//! over `uikit-gesture`), laid out by the adaptive layer (a phone swipes between full-screen
//! views; a desktop also shows a tab row), and voice/a11y-navigable. bot-arena's engine is
//! already GTK-free, so on-device this front-end renders its live state directly; here it

//! carries a small view model so the crate builds and the paging logic is unit-tested.

/// CorpusStudio: the corpus-LM pipeline as a live stepper.
pub mod studio;

use egui::{Align, Layout, Ui};
use uikit_egui::{Adaptive, GestureLayer, PaletteColors};

/// A competitor row (mirrors bot-arena's engine standings).
#[derive(Debug, Clone)]
pub struct Bot {
    pub name: String,
    pub score: i32,
    pub status: String,
}

/// A recorded match.
#[derive(Debug, Clone)]
pub struct Replay {
    pub title: String,
    pub turns: u32,
}

/// The dashboard views, in swipe order.
pub const VIEWS: [&str; 3] = ["Arena", "Standings", "Replays"];

/// Move the view index by a swipe sign (-1 / +1), clamped to the ends.
pub fn page(idx: usize, sign: i32, n: usize) -> usize {
    (idx as i32 + sign).clamp(0, n as i32 - 1) as usize
}

/// Dashboard state + the swipe gesture recognizer.
pub struct Dashboard {
    pub view_idx: usize,
    pub bots: Vec<Bot>,
    pub replays: Vec<Replay>,
    gesture: GestureLayer,
    palette: String,
}

impl Dashboard {
    pub fn new(bots: Vec<Bot>, replays: Vec<Replay>) -> Self {
        Dashboard { view_idx: 0, bots, replays, gesture: GestureLayer::horizontal_page(320.0), palette: String::new() }
    }

    /// Apply a committed swipe (from the gesture layer) to change view. Exposed for tests.
    pub fn swipe(&mut self, sign: i32) {
        self.view_idx = page(self.view_idx, sign, VIEWS.len());
    }

    /// Adopt cross-app preferences: theme + text/button size on the context, and the swipe
    /// feel (sensitivity / commit / flick / detent animation) on this dashboard's gesture
    /// layer. Call after `Settings::poll()` reports a change so bot-arena tracks the fleet.
    pub fn apply_settings(&mut self, ctx: &egui::Context, prefs: &settings_kit::Prefs) {
        uikit_egui::apply_prefs(ctx, prefs);
        *self.gesture.config_mut() = prefs.gesture_config(320.0);
        self.palette = prefs.palette.clone();
    }

    /// Test/introspection: the gesture layer's current takeover-lock (reflects detent animation).
    pub fn detent_lock_ms(&mut self) -> f64 {
        self.gesture.config_mut().takeover_lock_ms
    }

    pub fn current(&self) -> &'static str {
        VIEWS[self.view_idx]
    }

    /// Render the dashboard. Swipe left/right pages between views; on desktop a tab row is
    /// also shown (adaptive — the phone relies on the swipe).
    pub fn ui(&mut self, ui: &mut Ui, adaptive: &Adaptive) {
        let colors = uikit_egui::palette_colors(&self.palette);

        // swipe paging via the shared gesture layer
        let surface = ui.available_rect_before_wrap();
        let frame = self.gesture.update(ui, surface, true);
        if let Some(dir) = frame.page_jump {
            self.swipe(dir.sign());
        }

        // desktop shows a tab row; phone swipes
        if !adaptive.is_compact() {
            ui.horizontal(|ui| {
                for (i, name) in VIEWS.iter().enumerate() {
                    let selected = i == self.view_idx;
                    if ui.selectable_label(selected, *name).clicked() {
                        self.view_idx = i;
                    }
                }
            });
            ui.separator();
        } else {
            // a small "swipe" affordance + current view name for the compact layout
            ui.horizontal(|ui| {
                ui.colored_label(colors.text_weak, "‹ swipe ›");
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.colored_label(colors.accent, self.current());
                });
            });
        }

        match self.view_idx {
            0 => self.arena_view(ui, &colors),
            1 => self.standings_view(ui, &colors),
            _ => self.replays_view(ui, &colors),
        }

        // paint the swipe barrier progress if a swipe is in flight
        if let Some(edge) = frame.barrier_edge {
            uikit_egui::paint_barrier(ui, surface, frame.barrier_fraction, edge, colors.accent);
        }
    }

    fn arena_view(&self, ui: &mut Ui, c: &PaletteColors) {
        ui.heading("Arena");
        ui.colored_label(c.text_weak, "Live match — bots competing this round:");
        for b in self.bots.iter().take(4) {
            ui.colored_label(c.text, format!("{}  ·  {}", b.name, b.status));
        }
    }

    fn standings_view(&self, ui: &mut Ui, c: &PaletteColors) {
        ui.heading("Standings");
        let mut ranked = self.bots.clone();
        ranked.sort_by(|a, b| b.score.cmp(&a.score));
        for (i, b) in ranked.iter().enumerate() {
            ui.colored_label(if i == 0 { c.accent } else { c.text }, format!("{}. {}  —  {}", i + 1, b.name, b.score));
        }
    }

    fn replays_view(&self, ui: &mut Ui, c: &PaletteColors) {
        ui.heading("Replays");
        if self.replays.is_empty() {
            ui.colored_label(c.text_weak, "No replays yet.");
        }
        for r in &self.replays {
            ui.colored_label(c.text, format!("{}  ({} turns)", r.title, r.turns));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paging_clamps_at_ends() {
        assert_eq!(page(0, -1, 3), 0, "can't page before the first view");
        assert_eq!(page(0, 1, 3), 1);
        assert_eq!(page(2, 1, 3), 2, "can't page past the last view");
        assert_eq!(page(1, -1, 3), 0);
    }

    #[test]
    fn swipe_moves_through_views() {
        let mut d = Dashboard::new(vec![], vec![]);
        assert_eq!(d.current(), "Arena");
        d.swipe(1);
        assert_eq!(d.current(), "Standings");
        d.swipe(1);
        assert_eq!(d.current(), "Replays");
        d.swipe(1); // clamped
        assert_eq!(d.current(), "Replays");
        d.swipe(-1);
        assert_eq!(d.current(), "Standings");
    }

    #[test]
    fn standings_are_ranked_by_score() {
        let bots = vec![
            Bot { name: "a".into(), score: 3, status: "idle".into() },
            Bot { name: "b".into(), score: 9, status: "idle".into() },
        ];
        let mut ranked = bots.clone();
        ranked.sort_by(|x, y| y.score.cmp(&x.score));
        assert_eq!(ranked[0].name, "b");
    }

    #[test]
    fn apply_settings_adopts_gesture_feel_and_theme() {
        use settings_kit::{AnimationStyle, Prefs};
        let ctx = egui::Context::default();
        let mut d = Dashboard::new(vec![], vec![]);
        let mut prefs = Prefs::default();
        prefs.gesture.animation = AnimationStyle::Heavy;
        d.apply_settings(&ctx, &prefs);
        // heavy detent animation → a long takeover lock came through from the shared settings
        assert_eq!(d.detent_lock_ms(), 220.0);
        // and applying doesn't disturb the current view
        assert_eq!(d.current(), "Arena");
    }
}
