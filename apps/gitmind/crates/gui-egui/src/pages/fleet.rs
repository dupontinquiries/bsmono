//! Fleet page — browse the app catalog (cockpit store groups) and build + install one locally.
//! Direct cockpit calls; the build runs on the background Task so the frame never freezes.
use crate::app::App;
use cockpit::suite::SuiteApp;

#[derive(Default)]
pub struct State {
    pub selected: Option<SuiteApp>,
    pub note: String,
}

pub fn ui(app: &mut App, ui: &mut egui::Ui) {
    ui.heading("Fleet");
    ui.label(egui::RichText::new("build + install BigSilicon apps from the catalog").weak().small());
    ui.separator();

    egui::ScrollArea::vertical().max_height(280.0).show(ui, |ui| {
        for suite in cockpit::flow::store_groups() {
            egui::CollapsingHeader::new(format!("{}  ·  {}", suite.name, suite.description))
                .default_open(suite.name == "coding")
                .show(ui, |ui| {
                    for a in &suite.apps {
                        let sel = app.fleet.selected.as_ref().is_some_and(|s| s.name == a.name);
                        if ui.selectable_label(sel, &a.name).clicked() {
                            app.fleet.selected = Some(a.clone());
                        }
                    }
                });
        }
    });

    ui.separator();
    if let Some(sel) = app.fleet.selected.clone() {
        ui.label(format!("selected: {}  ({})", sel.name, sel.path));
        if !sel.features.is_empty() {
            ui.label(egui::RichText::new(format!("features: {}", sel.features.join(", "))).weak().small());
        }
        let can = !app.task.is_running();
        if ui.add_enabled(can, egui::Button::new("build + install locally")).clicked() {
            let a = sel.clone();
            app.task.run(move || match cockpit::flow::install_suite_app(&a) {
                Ok(i) => format!("✓ installed {} → {}", i.name, i.path),
                Err(e) => format!("error: {e}"),
            });
        }
    } else {
        ui.label(egui::RichText::new("pick an app above").weak());
    }

    if let Some(r) = app.task.take_result() {
        app.fleet.note = r;
    }
    if app.task.is_running() {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label("building…");
        });
        ui.ctx().request_repaint();
    }
    if !app.fleet.note.is_empty() {
        ui.add_space(6.0);
        ui.label(egui::RichText::new(&app.fleet.note).monospace().small());
    }
}
