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

    uikit_egui::scroll::bounded(ui, 280.0, |ui| {
        for suite in cockpit::flow::store_groups() {
            egui::CollapsingHeader::new(format!("{}  ·  {}", suite.name, suite.description))
                .default_open(suite.name == "coding")
                .show(ui, |ui| {
                    for a in &suite.apps {
                        let sel = app.fleet.selected.as_ref().is_some_and(|s| s.name == a.name);
                        if ui.selectable_label(sel, format!("{}  {}", crate::app::app_icon(&a.name), a.name)).clicked() {
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
            app.task.run_logged(move |sink| {
                let mut on_line = |l: &str| sink.push(l);
                match cockpit::flow::install_suite_app_logged(&a, &mut on_line) {
                    Ok(i) => format!("✓ installed {} → {}", i.name, i.path),
                    Err(e) => format!("error: {e}"),
                }
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
    let log = app.task.log();
    if !log.is_empty() {
        uikit_egui::scroll::log(ui, &log, 200.0);
    }
    if !app.fleet.note.is_empty() {
        ui.add_space(6.0);
        ui.label(egui::RichText::new(&app.fleet.note).monospace().small());
    }
}
