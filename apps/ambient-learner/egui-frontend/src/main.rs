//! The window. Deliberately thin: everything visible is a ported page.
//!
//! This exists because the parity audit reported ambient-learner as `no` binary,
//! `no` app-definition, `NO` manifest while having ported pages — a port nobody
//! could run. Ported surfaces behind an unrunnable front end are worth less
//! than one surface behind a runnable one.

use ambient_learner_egui::{
    ui::egui::{ambience, chrome, history, live, locations, map, model, screen_temps, sync},
    LiveState,
};

// This eframe requires `ui` as well as `update`; the window host draws from
// `update` and leaves `ui` empty, matching voiceterm's binary rather than
// inventing a second convention.

#[derive(PartialEq)]
enum Page { Live, History, Places, Map, Model, Sync, Temps }

struct App {
    live: LiveState,
    colors: uikit_egui::PaletteColors,
    page: Page,
    history: history::History,
    places: locations::LocationsPage,
    sync: sync::SyncPage,
    model: model::ModelPage,
    map: map::MapPage,
    ambience: ambience::Ambience,
    sparkles: ambience::Sparkles,
    started: std::time::Instant,
    temps: screen_temps::ScreenTempsPage,
    /// The ported Live page's own model — sensor readings, health, the ramp.
    /// `LiveState` above draws the big reading; this draws the SENSOR panel
    /// underneath it, which is the half of `LivePage` that was ported (with
    /// thirteen tests) and then never called: egui_lib grew its own inline
    /// version of the headline and stopped there.
    sensors: live::Live,
    /// The breathing dot, which stops when nothing is reporting — a dot that
    /// keeps pulsing while the sensors are dead says the app is fine when it
    /// is not.
    dot: live::BreathingDot,
}

impl App {
    /// Which section the current page belongs to.
    fn section(&self) -> chrome::Section {
        match self.page {
            Page::Live => chrome::Section::Live,
            Page::History => chrome::Section::History,
            Page::Places => chrome::Section::Places,
            Page::Map => chrome::Section::Map,
            Page::Model => chrome::Section::Model,
            Page::Sync => chrome::Section::Sync,
            Page::Temps => chrome::Section::Debug,
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, _ui: &mut egui::Ui, _frame: &mut eframe::Frame) {}

    fn update(&mut self, ctx: &egui::Context, _f: &mut eframe::Frame) {
        // Clamped: a resumed suspend delivers one enormous delta, and feeding
        // it to the breath would jump the dot rather than animate it.
        let dt_ms = ctx.input(|i| i.stable_dt).min(0.1) * 1000.0;
        egui::CentralPanel::default().show(ctx, |ui| {
            // The mood background sits BEHIND everything, and only on Live.
            // On the data pages it would compete with numbers people are
            // reading, which is the opposite of what an ambient layer is for.
            if self.page == Page::Live {
                let t = self.started.elapsed().as_millis() as f32;
                ambience::ambience_ui(ui, &self.ambience, &self.sparkles, t,
                                      ui.max_rect(), &self.colors);
            }
            ui.horizontal(|ui| {
                // Two tabs because two pages are ported. A bar listing pages
                // that do not exist would be the same overstatement the audit
                // caught in the first place.
                ui.selectable_value(&mut self.page, Page::Live, "Live");
                ui.selectable_value(&mut self.page, Page::History, "History");
                ui.selectable_value(&mut self.page, Page::Places, "Places");
                ui.selectable_value(&mut self.page, Page::Map, "Map");
                ui.selectable_value(&mut self.page, Page::Model, "Model");
                ui.selectable_value(&mut self.page, Page::Sync, "Sync");
                ui.selectable_value(&mut self.page, Page::Temps, "Temps");
            });
            ui.separator();
            // The floating section title, which the port had been missing
            // entirely: every page had its card headings and none had a page
            // title, so the tab bar was the only thing saying where you were.
            chrome::section_header(ui, self.section(), self.colors.accent);
            match self.page {
                Page::Live => {
                    self.live.ui(ui);
                    // The ported sensor panel below the headline. The dot
                    // breathes only while something is reporting, so it is
                    // driven from the readings rather than from a timer.
                    // readings() is (label, unit, Reading) — the tuple the
                    // panel draws from, so the availability check reaches into
                    // it rather than assuming a bare Reading.
                    self.dot.live = self.sensors.readings().iter()
                        .any(|(_, _, r)| r.available());
                    self.dot.tick(dt_ms);
                    if self.dot.live { ui.ctx().request_repaint(); }
                    live::live_ui(ui, &self.sensors, &self.dot, &self.colors);
                }
                Page::History => history::history_ui(ui, &self.history, &self.colors),
                Page::Map => map::map_ui(ui, &mut self.map, &self.colors),
                Page::Model => model::model_ui(ui, &self.model, &self.colors),
                Page::Sync => sync::sync_ui(ui, &self.sync, &self.colors),
                Page::Temps => {
                    // No publisher in the demo, so the page shows its
                    // Missing state — which is the honest thing a device with
                    // ambient-learner not running would show.
                    screen_temps::screen_temps_ui(ui, &self.temps, 0, &self.colors);
                }
                Page::Places => {
                    // The action is dropped: this demo has no backend to
                    // perform it against. Applying it locally would show a
                    // place that was never saved, which is the failure the
                    // page's own tests forbid.
                    let _ = locations::locations_ui(ui, &mut self.places, &self.colors);
                }
            }
        });
        // The live view eases toward its target, so it needs frames even when
        // nothing is being touched.
        ctx.request_repaint_after(std::time::Duration::from_millis(33));
    }
}

fn main() -> eframe::Result<()> {
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([420.0, 720.0])
            .with_title("Ambient Learner"),
        ..Default::default()
    };
    eframe::run_native("ambient-learner-egui", opts, Box::new(|_cc| {
        Ok(Box::new(App {
            live: LiveState::demo(),
            colors: uikit_egui::palette_colors("cloudscape"),
            page: Page::Live,
            // Empty rather than fabricated: the history page says "no readings
            // recorded yet", which is true, instead of drawing invented data.
            history: history::History::default(),
            // Empty, like the history: the page says "no saved locations, tap +
            // to add one", which is true, rather than inventing places.
            places: locations::LocationsPage::default(),
            // Zeroes, not invented counts: an empty database is a fact about a
            // device nobody has calibrated yet, and the page renders it as 0
            // rather than as a dash for exactly that reason.
            sync: sync::SyncPage::default(),
            // Zero neural calibrations, so the demo shows the threshold hint
            // doing its job — "0  (need 5 for neural)" is the state a new
            // device is actually in, and the most useful thing the page says.
            model: model::ModelPage::default(),
            // Empty: the page says "no mapped locations yet", which is true of
            // a device that has not been anywhere, rather than plotting
            // invented coordinates on a map that looks real.
            map: map::MapPage::default(),
            // Winter evening, no tilt, no light reading: the defaults the QML
            // itself declares, so an unwired demo shows the state the app
            // actually starts in rather than an invented summer afternoon.
            ambience: ambience::Ambience::default(),
            // 60% humidity — about fifteen droplets. Enough to show the layer
            // doing something; a demo at 0% would look like it was not ported.
            sparkles: ambience::Sparkles { humidity_pct: 60.0 },
            started: std::time::Instant::now(),
            temps: screen_temps::ScreenTempsPage::default(),
            // The sensor panel, seeded from the same demo snapshot the
            // headline uses so the two halves of the page agree. Humidity and
            // light stay Missing: this build reads no sensors, and inventing
            // values would make the health row claim "locked" while the
            // device has nothing attached — precisely the lie the row exists
            // to prevent.
            sensors: live::Live {
                temp_c: live::Reading::new(Some(21.4), 1),
                humidity_pct: live::Reading::missing(),
                pressure_hpa: live::Reading::new(Some(1008.0), 0),
                light: live::Reading::missing(),
                confidence: Some(0.82),
            },
            dot: live::BreathingDot::default(),
        }))
    }))
}
