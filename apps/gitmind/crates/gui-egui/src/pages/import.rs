//! Import page — pull a repo bundle or a zipped project into the workspace (cockpit::flow::
//! import_bundle / import). Runs on the background Task.
use crate::app::App;

#[derive(Default)]
pub struct State {
    pub path: String,
    pub note: String,
}

pub fn ui(app: &mut App, ui: &mut egui::Ui) {
    ui.heading("Import");
    ui.label(egui::RichText::new("bring a repo bundle or a zipped project into the workspace").weak().small());
    ui.separator();

    ui.horizontal(|ui| {
        ui.label("path");
        ui.text_edit_singleline(&mut app.import.path);
    });
    let can = !app.task.is_running() && !app.import.path.trim().is_empty();
    ui.horizontal(|ui| {
        if ui.add_enabled(can, egui::Button::new("import bundle")).clicked() {
            let p = app.import.path.clone();
            app.task.run(move || match cockpit::flow::import_bundle(&p) {
                Ok(s) => format!("✓ imported suite '{}' ({} apps)", s.name, s.apps.len()),
                Err(e) => format!("error: {e}"),
            });
        }
        if ui.add_enabled(can, egui::Button::new("import zip")).clicked() {
            let p = app.import.path.clone();
            app.task.run(move || match cockpit::flow::import(&p) {
                Ok(i) => format!("✓ imported {} → {}", i.app, i.dir.display()),
                Err(e) => format!("error: {e}"),
            });
        }
    });

    if let Some(r) = app.task.take_result() {
        app.import.note = r;
    }
    if app.task.is_running() {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label("importing…");
        });
        ui.ctx().request_repaint();
    }
    if !app.import.note.is_empty() {
        ui.add_space(6.0);
        ui.separator();
        ui.label(egui::RichText::new(&app.import.note).monospace().small());
    }
}
