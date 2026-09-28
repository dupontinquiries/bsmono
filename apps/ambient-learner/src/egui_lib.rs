//! parity-audit: not-a-surface
//!
//! GATED AT THE FILE, because Cargo ignores `required-features` on `[lib]`.
//!
//! Without this the target compiles on every `cargo check --all-targets` with
//! the egui dependency absent, producing 121 "cannot find crate" errors that
//! look like broken code and are a manifest that cannot express what it means.
//! With it the module is empty unless the feature is on, which is what the
//! manifest line was trying to say.
#![cfg(feature = "egui")]

//! The egui build's library root.
//!
//! It used to hold its OWN one-screen Live view — temperature, confidence bar,
//! sparkline — drawn from scratch here rather than calling the ported
//! `ui::egui::live` page. That is why the port looked alive from outside while
//! nine of its ten pages had no route in: the visible screen was a duplicate
//! that worked, and the real frontend was unreachable behind it.
//!
//! The duplicate is gone. `App` holds a `Shell`, the Shell owns the page state
//! and dispatches to the renderers, and `LiveState` remains only as the eased
//! reading it always was — the animation, not the screen.
//!
//! Separate from `main.rs` because that file is GTK-only — 7,048 lines that do
//! not compile without gtk4, adw and glib. This root pulls in only what the
//! egui frontend uses, so the egui build has no GTK dependency at all rather
//! than a satisfied-but-unused one.

pub mod ui {
    pub mod egui;
}

/// The GTK-free core, shared with `main.rs`: `Model`, `ThermalPoller`,
/// `Database`, the locations file, the device-state readers. Mounted here so
/// the egui frontend opens the REAL database and polls the REAL sensors —
/// until this line existed it could only show demo numbers, because every
/// piece of the engine lived inside the GTK binary's `main.rs`.
///
/// The modules below it are the engine's own dependencies, none of which
/// touch GTK either (`helpers` gates its two toast helpers behind the `gtk`
/// feature, which this build never enables).
// `dead_code` allowed on the engine modules under THIS root only: they are
// shared with the GTK binary, which uses the maintenance and export paths
// this frontend does not reach yet, and the warnings would say "never used"
// about code that is used — by the other root.
#[allow(dead_code)] pub mod engine;
#[allow(dead_code)] pub mod dbmaint;
#[allow(dead_code)] pub mod db_writer;
#[allow(dead_code)] pub mod environment;
#[allow(dead_code)] pub mod forecast;
#[allow(dead_code)] pub mod helpers;
#[allow(dead_code)] pub mod screen_temp_hints;
#[allow(dead_code)] pub mod screen_temps;
#[allow(dead_code)] pub mod seasons;
#[allow(dead_code)] pub mod systems;
#[allow(dead_code)] pub mod temperature_field;
#[allow(dead_code)] pub mod world_state;

// The worker systems and the DB writer were written against `main.rs`'s
// flat namespace (`crate::Model`, `crate::ThermalPoller`, ...). Re-exported
// at this root so the same files compile unchanged under both roots.
pub use engine::{get_device_state, Model, SensorReading, ThermalPoller};

/// The bridge from the engine to the pages: opens the store, runs the poll
/// loop on its own thread, and turns engine values into page state.
pub mod host;

pub use ui::egui::{confidence_tint, format_temp, norm, LiveSnapshot};
pub use ui::egui::shell::Shell;

/// The whole egui frontend.
///
/// Thin on purpose: it holds the Shell and a clock. Everything a page needs is
/// on the Shell, and everything a page draws is in the page.
pub struct App {
    pub shell: Shell,
    /// Animation clock, in milliseconds since start. Owned here so a test can
    /// freeze it — an animated surface with a clock it cannot stop is one no
    /// test can assert against.
    start: std::time::Instant,
}

impl Default for App {
    fn default() -> Self {
        App { shell: Shell::default(), start: std::time::Instant::now() }
    }
}

impl App {
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let colors = uikit_egui::palette_colors("midnight");
        let t_ms = self.start.elapsed().as_secs_f32() * 1000.0;
        self.shell.ui(ui, &colors, t_ms);
    }

    /// Actions raised by pages this frame, drained for the host to apply.
    ///
    /// Drained rather than read, so the host cannot apply the same add twice
    /// by rendering two frames before handling one.
    pub fn take_actions(&mut self) -> Vec<ui::egui::shell::Action> {
        std::mem::take(&mut self.shell.pending)
    }
}

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
            // A falling barometer, so the demo shows the trend arrow doing
            // something — a gauge sitting at steady teaches nothing about what
            // the page is for.
            pressure: Some(crate::ui::egui::pressure::Pressure {
                hpa: 1008.0, trend_hpa_per_30m: Some(-1.8) }),
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

    /// Draw the headline reading: location, the big eased temperature, the
    /// confidence tier, the day's band, and the recent series.
    ///
    /// This is the method `egui-frontend/main.rs` has been calling all along —
    /// `self.live.ui(ui)`, with a comment beside it saying "LiveState above
    /// draws the big reading; this draws the SENSOR panel". It did not exist,
    /// so the binary did not compile:
    ///
    /// ```text
    /// error[E0599]: no method named `ui` found for struct `LiveState`
    /// ```
    ///
    /// Deliberately NOT delegating to `LiveView::ui`, the crate's other Live
    /// implementation. That one eases through `ctx().animate_value_with_time`
    /// and owns no display state, while this one owns `shown`/`shown_conf` and
    /// is advanced by `tick(dt)` — calling it here would run two easings over
    /// one value and show whichever finished last. `mod.rs` already records
    /// that having two Live pages is an open question; this renders from the
    /// state it has rather than pretending to answer it.
    ///
    /// The pure parts ARE shared: `confidence_tint` and `sparkline_ui` are the
    /// same functions LiveView draws with, the latter extracted for this.
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let snap = &self.snapshot;
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(&snap.location).weak());
        });
        // The eased value, not `snapshot.temperature` — easing it and then
        // drawing the raw reading is a mistake that looks like it works,
        // because the two agree everywhere except during the animation.
        ui.label(egui::RichText::new(crate::ui::egui::format_temp(self.shown)).size(72.0).strong());
        let (tier, tint) = crate::ui::egui::confidence_tint(self.shown_conf);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(format!("{tier} confidence")).color(tint).small());
            ui.label(egui::RichText::new(&snap.condition).weak().small());
        });
        ui.label(
            egui::RichText::new(format!("band {:.1}° – {:.1}°", snap.min_temp, snap.max_temp))
                .weak().small(),
        );
        ui.add_space(6.0);
        crate::ui::egui::sparkline_ui(ui, &snap.series);
        // Keep painting while anything is still moving. Without it the ease
        // stops whenever the pointer does, which reads as a frozen window
        // rather than a settled reading.
        if self.animating() {
            ui.ctx().request_repaint();
        }
    }

    /// Is anything still moving? Used to keep repainting; without it the
    /// animation stops whenever the pointer does, which looks like a freeze.
    pub fn animating(&self) -> bool {
        (self.snapshot.temperature - self.shown).abs() > 0.01
            || (self.snapshot.confidence - self.shown_conf).abs() > 0.005
    }

    // THE DUPLICATE LIVE SCREEN WAS HERE, and removing it is the point of this
    // commit. It drew location, temperature, condition, a confidence bar, the
    // min/max range and a sparkline — a working screen, drawn from scratch,
    // that shadowed the ported `ui::egui::live` page and made a frontend with
    // nine unreachable pages look finished.
    //
    // LiveState keeps its easing (`tick`, `animating`) because that is a real
    // job and nothing else does it: the reading glides toward the sensor value
    // rather than snapping. What it no longer does is draw.
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
