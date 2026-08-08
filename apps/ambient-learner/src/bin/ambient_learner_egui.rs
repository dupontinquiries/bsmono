//! ambient-learner-egui — the runnable egui frontend.
//!
//! The `egui` feature builds the view library; nothing launched it. The parity
//! audit reported this app as `no` binary, `no` app-definition, `NO` manifest
//! while having ported pages, which is a port nobody can run — and the
//! Cargo.toml already referenced a `--features egui-app` that did not exist.
//!
//! Sensor wiring is deliberately absent here. The pages take a snapshot; the
//! host supplies it. This binary shows the demo snapshot so the window is
//! honest about having no readings rather than drawing zeroes.

use ambient_learner_egui::LiveState;

struct App {
    live: LiveState,
    colors: uikit_egui::PaletteColors,
    share: ambient_learner_egui::WeatherShare,
    page: Page,
    history: ambient_learner_egui::ui::egui::history::History,
}

#[derive(PartialEq)]
enum Page { Live, History }

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _f: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                // Two pages, because two are ported. A tab bar listing pages
                // that do not exist yet would be the same lie the audit caught.
                ui.selectable_value(&mut self.page, Page::Live, "Live");
                ui.selectable_value(&mut self.page, Page::History, "History");
            });
            ui.separator();
            match self.page {
                Page::Live => self.live.ui(ui, &mut self.share, &self.colors),
                Page::History => ambient_learner_egui::ui::egui::history::history_ui(
                    ui, &self.history, &self.colors),
            }
        });
        // The live page eases toward its target, so it needs frames even when
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
            share: Default::default(),
            page: Page::Live,
            history: ambient_learner_egui::ui::egui::history::History::default(),
        }))
    }))
}
