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

/// Is there a cached preview tile for this build?
///
/// `cockpit::preview` captures one after a build and keys it by app + oid, so a
/// rebuilt commit gets a fresh tile and old ones stay around. It also DROPS a
/// near-uniform grab — a headless app draws nothing, and a blank square is
/// worse than no square because it reads as a broken screenshot rather than an
/// app that has no window.
///
/// So a missing file is the normal case, not an error: headless tools never get
/// one, and the glyph is the intended result rather than a fallback from
/// failure.
pub fn tile_for(app: &str, oid: &str) -> Option<std::path::PathBuf> {
    let p = cockpit::preview::shot_path(app, oid);
    p.is_file().then_some(p)
}

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
                // A real tile when the build produced one, the hash glyph
                // otherwise. Both are the same width so the column does not
                // jump between apps that have screenshots and apps that cannot.
                let oid = gitmind_core::binpin::load_manifest(&name)
                    .versions.first().map(|v| v.oid.clone()).unwrap_or_default();
                match tile_for(&name, &oid) {
                    Some(path) => {
                        ui.add(egui::Image::new(format!("file://{}", path.display()))
                            .fit_to_exact_size(egui::vec2(18.0, 18.0)))
                            .on_hover_text("preview from the last build");
                    }
                    None => { ui.label(crate::app::app_icon(&name)); }
                }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_tile_is_the_normal_case_not_an_error() {
        // cockpit::preview DROPS a near-uniform grab, because a headless app
        // draws nothing and a blank square reads as a broken screenshot rather
        // than an app with no window. So most CLI tools never have a tile, and
        // the glyph is the intended result rather than a failure fallback.
        assert!(tile_for("a-tool-that-was-never-captured", "deadbeef").is_none());
    }

    #[test]
    fn the_tile_is_keyed_by_build_not_just_by_app() {
        // A rebuilt commit must get a fresh tile, or the Apps page shows the
        // previous build's window forever — which is worse than no tile,
        // because it looks current.
        let a = cockpit::preview::shot_path("demo", "aaaaaaaaaa11");
        let b = cockpit::preview::shot_path("demo", "bbbbbbbbbb22");
        assert_ne!(a, b);
        // and the same build resolves to the same file, so the cache hits
        assert_eq!(a, cockpit::preview::shot_path("demo", "aaaaaaaaaa11"));
    }

    #[test]
    fn an_app_with_no_recorded_oid_still_resolves_to_a_path() {
        // A manifest can exist before a build has been captured; asking for the
        // tile must not panic on an empty oid.
        let p = cockpit::preview::shot_path("demo", "");
        assert!(p.to_string_lossy().ends_with("demo.png"));
    }
}
