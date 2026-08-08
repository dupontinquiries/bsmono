//! The window. Deliberately thin: everything visible is a ported page.
//!
//! This exists because the parity audit reported ambient-learner as `no` binary,
//! `no` app-definition, `NO` manifest while having ported pages — a port nobody
//! could run. Ported surfaces behind an unrunnable front end are worth less
//! than one surface behind a runnable one.

use ambient_learner_egui::{ui::egui::{history, locations, model, sync}, LiveState};

// This eframe requires `ui` as well as `update`; the window host draws from
// `update` and leaves `ui` empty, matching voiceterm's binary rather than
// inventing a second convention.

#[derive(PartialEq)]
enum Page { Live, History, Places, Model, Sync }

struct App {
    live: LiveState,
    colors: uikit_egui::PaletteColors,
    page: Page,
    history: history::History,
    places: locations::LocationsPage,
    sync: sync::SyncPage,
    model: model::ModelPage,
}

impl eframe::App for App {
    fn ui(&mut self, _ui: &mut egui::Ui, _frame: &mut eframe::Frame) {}

    fn update(&mut self, ctx: &egui::Context, _f: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                // Two tabs because two pages are ported. A bar listing pages
                // that do not exist would be the same overstatement the audit
                // caught in the first place.
                ui.selectable_value(&mut self.page, Page::Live, "Live");
                ui.selectable_value(&mut self.page, Page::History, "History");
                ui.selectable_value(&mut self.page, Page::Places, "Places");
                ui.selectable_value(&mut self.page, Page::Model, "Model");
                ui.selectable_value(&mut self.page, Page::Sync, "Sync");
            });
            ui.separator();
            match self.page {
                Page::Live => self.live.ui(ui),
                Page::History => history::history_ui(ui, &self.history, &self.colors),
                Page::Model => model::model_ui(ui, &self.model, &self.colors),
                Page::Sync => sync::sync_ui(ui, &self.sync, &self.colors),
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
        }))
    }))
}
