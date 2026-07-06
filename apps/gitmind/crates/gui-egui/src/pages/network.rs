//! Network page — the fleet's daemons + a target device, calling cockpit directly (no JSON marshal
//! layer the Qt frontend needed). Remote actions run on a background Task so the frame never
//! freezes. This is the reference page for the port.
use crate::app::App;

#[derive(Default)]
pub struct State {
    pub user: String,
    pub host: String,
    pub note: String,
}

pub fn ui(app: &mut App, ui: &mut egui::Ui) {
    ui.heading("Network");
    ui.separator();

    ui.label(egui::RichText::new("daemons — the fleet's background services").strong());
    for d in cockpit::daemons::all() {
        ui.horizontal(|ui| {
            ui.label("●");
            ui.label(d.name);
            ui.label(egui::RichText::new(d.description).weak().small());
        });
    }

    ui.add_space(8.0);
    ui.separator();
    ui.horizontal(|ui| {
        ui.label("user");
        ui.text_edit_singleline(&mut app.net.user);
        ui.label("host");
        ui.text_edit_singleline(&mut app.net.host);
    });

    let can = !app.task.is_running() && !app.net.host.trim().is_empty();
    ui.horizontal(|ui| {
        if ui.add_enabled(can, egui::Button::new("status")).clicked() {
            let (u, h) = (app.net.user.clone(), app.net.host.clone());
            app.task.run(move || match cockpit::flow::daemons_status_remote(&u, &h) {
                Ok(o) => o,
                Err(e) => format!("error: {e}"),
            });
        }
        if ui.add_enabled(can, egui::Button::new("install + enable")).clicked() {
            let (u, h) = (app.net.user.clone(), app.net.host.clone());
            let (bin, home) = (format!("/home/{u}/.local/bin"), format!("/home/{u}"));
            app.task.run(move || match cockpit::flow::install_daemons_remote(&u, &h, &bin, &home) {
                Ok(o) => o,
                Err(e) => format!("error: {e}"),
            });
        }
    });

    if let Some(r) = app.task.take_result() {
        app.net.note = r;
    }
    if app.task.is_running() {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label("working…");
        });
        ui.ctx().request_repaint();
    }
    if !app.net.note.is_empty() {
        ui.add_space(6.0);
        ui.separator();
        ui.label(egui::RichText::new(&app.net.note).monospace().small());
    }
}
