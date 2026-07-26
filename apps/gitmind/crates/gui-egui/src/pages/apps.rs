//! Apps page — installed apps (from gitmind's bin-pin store): run or remove them. `run_local`
//! launches on the background Task; `list_apps`/`remove` are quick local ops.
use crate::app::App;

#[derive(Default)]
pub struct State {
    pub apps: Vec<String>,
    pub loaded: bool,
    pub note: String,
}

pub fn ui(app: &mut App, ui: &mut egui::Ui) {
    ui.heading("Apps");
    ui.label(egui::RichText::new("installed apps — run or remove").weak().small());
    ui.separator();

    if ui.button("↻ refresh").clicked() || !app.apps.loaded {
        app.apps.loaded = true;
        app.apps.apps = gitmind_core::binpin::list_apps();
    }

    if app.apps.apps.is_empty() {
        ui.label(egui::RichText::new("no apps installed yet — build some from the Fleet tab").weak());
    }
    for name in app.apps.apps.clone() {
        ui.horizontal(|ui| {
            // the same per-app glyph the Fleet store rows use — recognisable at a glance
            ui.label(crate::app::app_icon(&name));
            ui.label(&name);
            if ui.add_enabled(!app.task.is_running(), egui::Button::new("run").small()).clicked() {
                let n = name.clone();
                app.task.run(move || match cockpit::flow::run_local(&n, "") {
                    Ok(m) => m,
                    Err(e) => format!("error: {e}"),
                });
            }
            if ui.button("remove").clicked() {
                let (a, l) = name.split_once('@').unwrap_or((name.as_str(), ""));
                match gitmind_core::binpin::remove(a, l) {
                    Ok(()) => {
                        app.apps.note = format!("removed {name}");
                        app.apps.loaded = false; // reload the list
                    }
                    Err(e) => app.apps.note = format!("error: {e}"),
                }
            }
        });
    }

    if let Some(r) = app.task.take_result() {
        app.apps.note = r;
    }
    if app.task.is_running() {
        ui.ctx().request_repaint();
    }
    if !app.apps.note.is_empty() {
        ui.add_space(6.0);
        ui.separator();
        ui.label(egui::RichText::new(&app.apps.note).monospace().small());
    }
}
