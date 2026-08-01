//! Fleet page — browse the app catalog (cockpit store groups) and build + install one locally.
//! Direct cockpit calls; the build runs on the background Task so the frame never freezes.
//!
//! Build flags: ported from the Qt cockpit's Fleet.qml, which scans an app's real Cargo.toml
//! `[features]` (via `cockpit::buildflags`) and renders a checkbox per feature, pre-checked to
//! the repo's per-arch defaults — rather than the static, hand-curated `SuiteApp.features` list
//! this page used before. Egui port: surfaces `cockpit::appcatalog`'s manifest lookup, so an app
//! with a registered egui port (e.g. voiceterm → its egui-frontend) shows that directly instead
//! of a developer needing to separately know about manifest/projects.toml.
use crate::app::App;
use cockpit::suite::SuiteApp;
use cockpit::{appcatalog, buildflags};

#[derive(Default)]
pub struct State {
    pub selected: Option<SuiteApp>,
    pub note: String,
    pub options_open: bool,
    /// Which path `feature_toggles` was last computed for — re-scan only when the selection
    /// actually changes, not every frame.
    scanned_path: Option<String>,
    feature_toggles: Vec<(String, bool)>,
    /// This app's registered egui port (manifest/projects.toml), if any — `None` also covers
    /// "the manifest wasn't readable," same as `appcatalog::AppEntry::egui_port`.
    egui_port: Option<appcatalog::ManifestEntry>,
    /// Set once we have auto-selected a `primary = true` port for the current selection, so
    /// the swap happens on discovery and not on every frame (which would fight the user the
    /// instant they chose the original).
    primary_swapped: bool,
    /// The entry the primary port replaced, kept so "use the original instead" can put it
    /// back. A default you cannot undo is a trap.
    pub legacy: Option<SuiteApp>,
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
                        ui.horizontal(|ui| {
                            // deterministic identicon tile (ported from the Qt MonoTile), not an emoji
                            uikit_egui::mono_tile::mono_tile(ui, &a.name, 22.0);
                            ui.add_space(6.0);
                            if ui.selectable_label(sel, &a.name).clicked() {
                                app.fleet.selected = Some(a.clone());
                            }
                        });
                    }
                });
        }
    });

    ui.separator();
    if let Some(sel) = app.fleet.selected.clone() {
        ui.label(format!("selected: {}  ({})", sel.name, sel.path));
        // When the manifest's primary port replaced the selection, say so plainly and keep
        // the original one click away — a default that cannot be undone is a trap, and the
        // Qt shell is still the parity reference for the port.
        if let Some(legacy) = app.fleet.legacy.clone() {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("★ egui build — the default for this app")
                    .color(egui::Color32::from_rgb(122, 200, 140)).small());
                if ui.small_button(format!("use the original ({}) instead", legacy.name)).clicked() {
                    app.fleet.selected = Some(legacy);
                    app.fleet.legacy = None;
                    app.fleet.scanned_path = None;
                }
            });
        }

        // re-scan the REAL Cargo.toml + manifest only when the selection actually changed —
        // matches Fleet.qml's local_build_options call on selection, not every frame.
        if app.fleet.scanned_path.as_deref() != Some(sel.path.as_str()) {
            // sel.path is often just the bare app NAME (e.g. "voiceterm", from SuiteApp::simple —
            // store_groups() deliberately doesn't hard-code apps/ vs games/), not a real filesystem
            // path — reading `sel.path` directly as a path (as this used to) silently fails
            // (read_to_string errors, unwrap_or_default() swallows it), so the scan always came back
            // empty and no checkboxes ever showed. resolve_app_dir does the SAME apps/-vs-games/
            // discovery build_locally_logged uses at actual build time.
            let real_dir = cockpit::ops::resolve_app_dir(&sel.path);
            let cargo = std::fs::read_to_string(real_dir.join("Cargo.toml")).unwrap_or_default();
            app.fleet.feature_toggles = buildflags::feature_toggles(&cargo, &sel.target);
            // manifest paths are repo-relative (e.g. "apps/voiceterm"); strip repo_root off the
            // resolved absolute dir to compare like-for-like instead of against the bare sel.path.
            let real_rel = real_dir.strip_prefix(cockpit::ops::repo_root()).ok().map(|p| p.display().to_string());
            app.fleet.egui_port = real_rel.and_then(|rel| {
                std::fs::read_to_string(cockpit::ops::repo_root().join("manifest/projects.toml")).ok().and_then(|manifest| {
                    appcatalog::parse_manifest(&manifest).into_iter().find(|m| {
                        // the app's own path (e.g. "apps/voiceterm") is a prefix of the port's path
                        // (e.g. "apps/voiceterm/egui-frontend") for every port registered this way.
                        m.path.starts_with(rel.trim_end_matches('/')) && m.path != rel
                    })
                })
            });
            app.fleet.scanned_path = Some(sel.path.clone());
            // a genuinely new selection may have its own primary port to honour
            if app.fleet.legacy.as_ref().map(|l| l.path != sel.path).unwrap_or(true) {
                app.fleet.primary_swapped = false;
            }
        }

        if let Some(port) = app.fleet.egui_port.clone() {
            // A port marked `primary = true` in the manifest IS the app: select it on the
            // user's behalf the moment it is discovered, rather than showing the superseded
            // build and asking them to opt in. voiceterm is the first of these — the Qt/QML
            // shell stays available as the parity reference, but "voiceterm" now means the
            // egui build, and the button below offers the original instead.
            // A library entry is catalogued but not buildable — offering it as
            // an app wastes a build and leaves the user wondering why nothing
            // ran. Say what it is instead.
            if !port.is_runnable() {
                ui.label(egui::RichText::new(format!(
                    "📚 {} is a library, embedded by other frontends — nothing to build here",
                    port.app_id))
                    .color(egui::Color32::from_rgb(150, 150, 170)).small());
                return;
            }
            if port.primary && !app.fleet.primary_swapped {
                let mut ported = sel.clone();
                ported.name = format!("{}-egui", sel.name);
                ported.path = port.path.clone();
                app.fleet.legacy = Some(sel.clone());   // remember what we replaced
                app.fleet.selected = Some(ported);
                app.fleet.primary_swapped = true;
                app.fleet.scanned_path = None;          // re-scan against the port's Cargo.toml
                ui.ctx().request_repaint();
                return;
            }
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(format!("🔌 egui port registered: {}", port.path)).color(egui::Color32::from_rgb(122, 162, 247)).small());
                if ui.small_button("build this port instead").clicked() {
                    let mut ported = sel.clone();
                    ported.name = format!("{}-egui", sel.name);
                    ported.path = port.path.clone();
                    app.fleet.selected = Some(ported);
                    app.fleet.scanned_path = None; // force a re-scan against the port's own Cargo.toml
                }
            });
            if !port.notes.is_empty() {
                ui.label(egui::RichText::new(&port.notes).weak().small());
            }
        }

        if !app.fleet.feature_toggles.is_empty() {
            let label = if app.fleet.options_open { "▾ options" } else { "▸ options" };
            if ui.selectable_label(false, egui::RichText::new(label).color(egui::Color32::from_rgb(122, 162, 247))).clicked() {
                app.fleet.options_open = !app.fleet.options_open;
            }
            if app.fleet.options_open {
                for (name, on) in app.fleet.feature_toggles.iter_mut() {
                    ui.checkbox(on, name.as_str());
                }
            }
        } else if !sel.features.is_empty() {
            // not a cargo crate we could scan (or no [features] table) — fall back to the
            // curated static list this page always had, rather than showing nothing.
            ui.label(egui::RichText::new(format!("features: {}", sel.features.join(", "))).weak().small());
        }

        let can = !app.task.is_running();
        if ui.add_enabled(can, egui::Button::new("build + install locally")).clicked() {
            // if the user has actually scanned real features, build with the checked set —
            // otherwise fall back to the app's original static features/no_default exactly as
            // this page always did, so apps without a scannable Cargo.toml keep working.
            let mut a = sel.clone();
            if !app.fleet.feature_toggles.is_empty() {
                a.features = app.fleet.feature_toggles.iter().filter(|(_, on)| *on).map(|(n, _)| n.clone()).collect();
                a.no_default = true;
            }
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
