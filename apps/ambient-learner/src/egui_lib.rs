//! The egui build's library root: the Live view plus the state a running app
//! needs around it.
//!
//! Separate from `main.rs` because that file is GTK-only — 7,048 lines that do
//! not compile without gtk4, adw and glib. This root pulls in only what the
//! egui frontend uses, so the egui build has no GTK dependency at all rather
//! than a satisfied-but-unused one.

pub mod ui {
    pub mod egui;
}

pub use ui::egui::{confidence_tint, format_temp, norm, LiveSnapshot};

/// The running Live view: a snapshot, plus the eased values that make it feel
/// alive rather than stepping between readings.
pub struct LiveState {
    pub snapshot: LiveSnapshot,
    /// Displayed temperature, easing toward `snapshot.temperature`.
    shown: f32,
    /// Displayed confidence, easing toward `snapshot.confidence`.
    shown_conf: f32,
}

/// How fast a displayed value catches up, per second. Fast enough not to feel
/// laggy, slow enough that a jitter in the sensor does not become a flicker on
/// screen — which is the whole reason for easing rather than assigning.
pub const EASE_RATE: f32 = 4.0;

impl LiveState {
    pub fn new(snapshot: LiveSnapshot) -> LiveState {
        LiveState { shown: snapshot.temperature, shown_conf: snapshot.confidence, snapshot }
    }

    /// A plausible reading, so the binary starts with something on screen. An
    /// app that opens blank is indistinguishable from one that failed to read
    /// its sensor.
    pub fn demo() -> LiveState {
        LiveState::new(LiveSnapshot {
            location: "Home".into(),
            temperature: 21.4,
            confidence: 0.82,
            min_temp: 20.1,
            max_temp: 22.6,
            series: vec![19.8, 20.2, 20.6, 21.0, 21.1, 21.3, 21.4],
            condition: "steady".into(),
            updated: 0,
        })
    }

    /// Feed a new reading. The display eases toward it rather than jumping.
    pub fn update(&mut self, snapshot: LiveSnapshot) {
        self.snapshot = snapshot;
    }

    /// Advance the eased values by `dt` seconds.
    ///
    /// Exponential rather than linear, so a large correction moves fast and a
    /// small one settles gently — a linear ease crawls across a big jump and
    /// overshoots a small one.
    pub fn tick(&mut self, dt: f32) {
        let k = 1.0 - (-EASE_RATE * dt.max(0.0)).exp();
        self.shown += (self.snapshot.temperature - self.shown) * k;
        self.shown_conf += (self.snapshot.confidence - self.shown_conf) * k;
    }

    pub fn shown_temperature(&self) -> f32 { self.shown }
    pub fn shown_confidence(&self) -> f32 { self.shown_conf }

    /// Is anything still moving? Used to keep repainting; without it the
    /// animation stops whenever the pointer does, which looks like a freeze.
    pub fn animating(&self) -> bool {
        (self.snapshot.temperature - self.shown).abs() > 0.01
            || (self.snapshot.confidence - self.shown_conf).abs() > 0.005
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let (label, tint) = confidence_tint(self.shown_conf);
        ui.vertical_centered(|ui| {
            ui.add_space(18.0);
            ui.label(egui::RichText::new(&self.snapshot.location).size(16.0).weak());
            // The reading is the focal point, through size.
            ui.label(egui::RichText::new(format_temp(self.shown)).size(64.0).strong());
            ui.label(egui::RichText::new(&self.snapshot.condition).size(15.0).weak());
            ui.add_space(10.0);
            ui.label(egui::RichText::new(label).color(tint).size(13.0));
            // confidence bar
            let (rect, _) = ui.allocate_exact_size(
                egui::Vec2::new(ui.available_width() * 0.7, 6.0), egui::Sense::hover());
            ui.painter().rect_filled(rect, 3.0, tint.gamma_multiply(0.22));
            let mut filled = rect;
            filled.set_width(rect.width() * self.shown_conf.clamp(0.0, 1.0));
            ui.painter().rect_filled(filled, 3.0, tint);
            ui.add_space(16.0);
            ui.label(egui::RichText::new(format!("{} – {}",
                format_temp(self.snapshot.min_temp), format_temp(self.snapshot.max_temp)))
                .size(13.0).weak());
            ui.add_space(20.0);
            sparkline(ui, &self.snapshot.series, tint);
        });
    }
}

/// The recent trend. Drawn from the series directly so it cannot disagree with
/// the numbers above it.
fn sparkline(ui: &mut egui::Ui, series: &[f32], tint: egui::Color32) {
    let (rect, _) = ui.allocate_exact_size(
        egui::Vec2::new(ui.available_width() * 0.8, 56.0), egui::Sense::hover());
    if series.len() < 2 { return; }
    let lo = series.iter().cloned().fold(f32::MAX, f32::min);
    let hi = series.iter().cloned().fold(f32::MIN, f32::max);
    let pts: Vec<egui::Pos2> = series.iter().enumerate().map(|(i, v)| {
        let x = rect.min.x + rect.width() * i as f32 / (series.len() - 1).max(1) as f32;
        let y = rect.max.y - rect.height() * norm(*v, lo, hi);
        egui::Pos2::new(x, y)
    }).collect();
    for w in pts.windows(2) {
        ui.painter().line_segment([w[0], w[1]], egui::Stroke::new(2.0, tint));
    }
    if let Some(last) = pts.last() {
        ui.painter().circle_filled(*last, 3.5, tint);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_display_eases_toward_a_new_reading_rather_than_jumping() {
        // A sensor jitters; easing is what stops that becoming a flicker on
        // screen. Assigning directly would show every twitch.
        let mut s = LiveState::new(LiveSnapshot { temperature: 20.0, ..Default::default() });
        s.update(LiveSnapshot { temperature: 25.0, ..Default::default() });
        assert_eq!(s.shown_temperature(), 20.0, "not yet moved");
        s.tick(0.1);
        let mid = s.shown_temperature();
        assert!(mid > 20.0 && mid < 25.0, "part way there: {mid}");
        for _ in 0..100 { s.tick(0.05); }
        assert!((s.shown_temperature() - 25.0).abs() < 0.01, "and it arrives");
    }

    #[test]
    fn a_big_correction_moves_faster_than_a_small_one() {
        // Exponential rather than linear: a linear ease crawls across a big jump
        // and overshoots a small one.
        let step = |from: f32, to: f32| {
            let mut s = LiveState::new(LiveSnapshot { temperature: from, ..Default::default() });
            s.update(LiveSnapshot { temperature: to, ..Default::default() });
            s.tick(0.1);
            (s.shown_temperature() - from).abs()
        };
        let big = step(0.0, 40.0);
        let small = step(0.0, 1.0);
        assert!(big > small * 10.0, "big {big} should dwarf small {small}");
    }

    #[test]
    fn animating_reports_when_a_repaint_is_still_needed() {
        // Without this the animation stops whenever the pointer does, which
        // looks like the app freezing.
        let mut s = LiveState::new(LiveSnapshot { temperature: 20.0, ..Default::default() });
        assert!(!s.animating(), "at rest");
        s.update(LiveSnapshot { temperature: 30.0, ..Default::default() });
        assert!(s.animating());
        for _ in 0..200 { s.tick(0.05); }
        assert!(!s.animating(), "and stops once it has settled");
    }

    #[test]
    fn the_demo_state_is_plausible_so_the_app_does_not_open_blank() {
        // An app that opens blank is indistinguishable from one that failed to
        // read its sensor.
        let s = LiveState::demo();
        assert!(!s.snapshot.location.is_empty());
        assert!(s.snapshot.temperature > -50.0 && s.snapshot.temperature < 60.0);
        assert!(s.snapshot.series.len() >= 2, "the sparkline needs a series");
        assert!(s.snapshot.min_temp <= s.snapshot.temperature);
        assert!(s.snapshot.max_temp >= s.snapshot.temperature);
    }

    #[test]
    fn a_zero_or_negative_timestep_does_not_move_or_explode() {
        // A paused window or a clock adjustment can produce either.
        let mut s = LiveState::new(LiveSnapshot { temperature: 20.0, ..Default::default() });
        s.update(LiveSnapshot { temperature: 30.0, ..Default::default() });
        s.tick(0.0);
        assert_eq!(s.shown_temperature(), 20.0);
        s.tick(-5.0);
        assert!(s.shown_temperature().is_finite());
        assert_eq!(s.shown_temperature(), 20.0, "negative time must not rewind");
    }
}
