//! Apps page — installed apps from gitmind's bin-pin store: run, switch version,
//! or remove.
//!
//! ## The bug this page had
//!
//! `list_apps()` returns manifest NAMES (`snapclone`), and the page split each
//! on `@` expecting `app@label`, falling back to an empty label. So remove
//! called `binpin::remove(name, "")` — an empty label matches no version, so
//! `versions.retain()` kept everything, `current` never matched, and the
//! manifest was saved unchanged. The page then printed "removed {name}" and
//! reloaded a list that had not changed.
//!
//! Reporting success while doing nothing is worse than failing: the user tries
//! once, sees it work, and only discovers otherwise later.
//!
//! ## What the list shows now
//!
//! Versions, not just names. An app pinned at three commits is three rows,
//! because that is what the store holds and what "remove" has to be able to
//! name. The current one is marked, since which binary `gitmind run` will use is
//! the question the page exists to answer.

use crate::app::App;

#[derive(Default)]
pub struct State {
    /// (app, label, is_current)
    pub versions: Vec<(String, String, bool)>,
    pub loaded: bool,
    pub note: String,
    /// Set while a removal is awaiting confirmation. Removing a pinned build is
    /// not undoable — the bytes are content-addressed but the pin is the only
    /// record that this app ever had this version — so it asks first.
    pub confirm: Option<(String, String)>,
}

pub fn ui(app: &mut App, ui: &mut egui::Ui) {
    ui.heading("Apps");
    ui.label(egui::RichText::new("installed versions — run, activate or remove").weak().small());
    ui.separator();

    if ui.button("↻ refresh").clicked() || !app.apps.loaded {
        app.apps.loaded = true;
        app.apps.versions = gitmind_core::binpin::list_versions();
    }

    if app.apps.versions.is_empty() {
        ui.label(egui::RichText::new("no apps installed yet — build some from the Fleet tab").weak());
    }

    let rows = app.apps.versions.clone();
    let mut last_app = String::new();
    for (name, label, is_current) in rows {
        // Group by app: three pins of one app under one heading reads as one
        // app with three versions, which is what it is. A flat list of
        // `name@label` rows reads as three apps.
        if name != last_app {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(crate::app::app_icon(&name));
                ui.label(egui::RichText::new(&name).strong());
                if ui.small_button("remove app").clicked() {
                    app.apps.confirm = Some((name.clone(), String::new()));
                }
            });
            last_app = name.clone();
        }
        ui.horizontal(|ui| {
            ui.add_space(18.0);
            if is_current {
                ui.label(egui::RichText::new("●").color(egui::Color32::from_rgb(120, 200, 140)))
                    .on_hover_text("the version `gitmind run` uses");
            } else {
                ui.label(" ");
            }
            ui.label(egui::RichText::new(&label).monospace());

            if ui.add_enabled(!app.task.is_running(), egui::Button::new("run").small()).clicked() {
                // Run THIS version, not whichever is current — the whole point
                // of pinning several is being able to run an old one.
                let target = format!("{name}@{label}");
                app.task.run(move || match cockpit::flow::run_local(&target, "") {
                    Ok(m) => m,
                    Err(e) => format!("error: {e}"),
                });
            }
            if !is_current && ui.small_button("make current").clicked() {
                match gitmind_core::binpin::use_version(&name, &label) {
                    Ok(()) => { app.apps.note = format!("{name} → {label}"); app.apps.loaded = false; }
                    Err(e) => app.apps.note = format!("error: {e}"),
                }
            }
            if ui.small_button("remove").clicked() {
                app.apps.confirm = Some((name.clone(), label.clone()));
            }
        });
    }

    // Confirmation. Asked because removing a pin is not undoable: the bytes are
    // content-addressed, but the pin is the only record that this app ever had
    // this version, and rebuilding it needs the commit it came from.
    if let Some((name, label)) = app.apps.confirm.clone() {
        ui.add_space(6.0);
        ui.separator();
        let what = if label.is_empty() { format!("{name} and every version") }
                   else { format!("{name}@{label}") };
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(format!("Remove {what}?")).strong());
            if ui.button("remove").clicked() {
                let r = if label.is_empty() {
                    gitmind_core::binpin::remove_app(&name)
                } else {
                    gitmind_core::binpin::remove(&name, &label)
                };
                app.apps.note = match r {
                    Ok(()) => format!("removed {what}"),
                    Err(e) => format!("error: {e}"),
                };
                app.apps.confirm = None;
                app.apps.loaded = false;
            }
            if ui.button("cancel").clicked() { app.apps.confirm = None; }
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
