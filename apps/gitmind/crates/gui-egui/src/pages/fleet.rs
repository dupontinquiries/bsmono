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

    // ── the reroll section ──
    /// The remembered fleet. Lives here rather than being rediscovered each
    /// frame: a scan takes seconds and the labels are the operator's, not the
    /// network's.
    pub roster: cockpit::roster::Roster,
    /// What the last scan changed, shown so the list does not mutate silently.
    pub last_changes: Vec<cockpit::roster::Change>,
    /// Apps to push in a reroll. Empty is a legitimate state and the plan says
    /// so rather than pretending to have work.
    pub reroll_apps: Vec<String>,
    /// A run in progress, or the last one that finished.
    pub run: Option<cockpit::runview::RunView>,
}

/// The reroll section: roster, what a run WOULD do, and a run's progress.
///
/// Sits on the Fleet page rather than becoming a twelfth tab. This page is
/// already "get apps onto devices"; a reroll is that for every device at once,
/// and splitting them would mean two places to learn for one intent.
fn reroll_ui(app: &mut App, ui: &mut egui::Ui) {
    use cockpit::{reroll, roster, runview};
    ui.separator();
    ui.heading("Reroll the fleet");
    ui.label(egui::RichText::new(
        "one pass: pair what needs pairing, build once per architecture, install everywhere")
        .weak().small());

    let st = &mut app.fleet;

    ui.horizontal(|ui| {
        if ui.button("Scan LAN").clicked() {
            // Real discovery through the code that already exists. The roster
            // absorbs it — a device that moved is updated, one that vanished
            // goes offline, one that was reflashed loses its enrolment.
            let seen: Vec<roster::Seen> = match cockpit::deploy::local_subnet24() {
                Some(prefix) => cockpit::deploy::scan_ssh(
                    &prefix, std::time::Duration::from_millis(300), 64)
                    .into_iter()
                    .map(|d| roster::Seen {
                        ip: d.ip,
                        hostname: d.hostname,
                        // The scan does not read host keys yet, and None means
                        // "unchanged" rather than "reflashed" — inventing a key
                        // here would reset enrolment on every scan.
                        host_key: None,
                    })
                    .collect(),
                None => Vec::new(),
            };
            st.last_changes = st.roster.observe(&seen);
        }
        ui.label(format!("{} device(s) on the roster", st.roster.len()));
    });

    for c in &st.last_changes {
        let line = match c {
            roster::Change::Appeared { id } => format!("{id} is back"),
            roster::Change::Vanished { id } => format!("{id} went offline"),
            roster::Change::MovedTo { id, ip } => format!("{id} moved to {ip}"),
            roster::Change::Reflashed { id } => format!("{id} looks reflashed — will re-pair"),
            roster::Change::Unknown { hostname, ip } =>
                format!("unclaimed: {hostname} at {ip}"),
        };
        ui.label(egui::RichText::new(line).weak().small());
    }

    // WHAT A RUN WOULD DO, before committing to it. The plan is cheap and
    // pure, so showing it costs nothing and turns a button press from a leap
    // into a confirmation.
    let devices = st.roster.devices();
    let plan = reroll::plan(&devices, &st.reroll_apps);
    ui.label(format!(
        "plan: {} pairing, {} build(s), {} install(s)",
        plan.enroll_count(), plan.build_count(), plan.install_count()));
    for (id, why) in &plan.skipped {
        let reason = match why {
            reroll::Skipped::Offline => "offline",
            reroll::Skipped::NothingToInstall => "no apps selected",
        };
        ui.label(egui::RichText::new(format!("skipping {id} — {reason}")).weak().small());
    }

    // The run, when there is one. Driven entirely by the tested view-model, so
    // this is only drawing.
    if let Some(run) = &st.run {
        ui.add(egui::ProgressBar::new(run.fraction()).text(run.headline()));
        for (id, row) in &run.rows {
            ui.horizontal(|ui| {
                ui.label(id);
                ui.label(egui::RichText::new(row.line()).weak().small());
            });
        }
    } else if !plan.steps.is_empty() {
        // No dead button: the executor needs a Runner over ssh/cargo/scp and
        // that is not wired yet. Saying so is better than a control that does
        // nothing when pressed.
        ui.label(egui::RichText::new(
            "run: not wired yet — needs the ssh/cargo runner")
            .weak().small());
        let _ = runview::RunView::with_devices(&[]);
    }
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
            let cancel = app.task.cancel_flag();
            app.task.run_logged(move |sink| {
                let mut on_line = |l: &str| sink.push(l);
                match cockpit::flow::install_suite_app_cancellable(&a, &mut on_line, Some(cancel)) {
                    Ok(i) => format!("✓ installed {} → {}", i.name, i.path),
                    Err(e) => format!("error: {e}"),
                }
            });
        }

        // TEST, beside build rather than instead of it. The reachability audit
        // makes the case: rusty-board's port has ten modules and 126 tests that
        // the binary never reaches, and every one of those tests passes. A
        // green suite and a working app are different claims, and the only way
        // to notice the gap is to be able to run the suite from here.
        //
        // Same feature set as the build, deliberately — cargo_test_argv derives
        // from the same SuiteApp, because a test run under different features
        // tests different code.
        if ui.add_enabled(can, egui::Button::new("test")).clicked() {
            let mut a = sel.clone();
            if !app.fleet.feature_toggles.is_empty() {
                a.features = app.fleet.feature_toggles.iter().filter(|(_, on)| *on).map(|(n, _)| n.clone()).collect();
                a.no_default = true;
            }
            app.task.run_logged(move |sink| {
                let mut on_line = |l: &str| sink.push(l);
                match cockpit::flow::test_suite_app_logged(&a, &mut on_line) {
                    Ok(r) => r,
                    Err(e) => format!("error: {e}"),
                }
            });
        }
    } else {
        ui.label(egui::RichText::new("pick an app above").weak());
    }

    // ── whole-catalog actions ────────────────────────────────────────────
    //
    // Reported missing from "all apps" and never added. Both walk
    // `store_groups()`, which is the same list the catalog above renders — so
    // "all" cannot drift from what the page shows.
    ui.add_space(8.0);
    ui.separator();
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        let idle = !app.task.is_running();
        if ui.add_enabled(idle, egui::Button::new("build all")).clicked() {
            app.task.run_logged(move |sink| {
                let mut on_line = |l: &str| sink.push(l);
                let (ok, failed) = cockpit::flow::build_all_logged(&mut on_line);
                // Report BOTH counts. "built 23 apps" with three silent
                // failures is the line that makes a broken fleet look healthy.
                if failed.is_empty() { format!("✓ built {ok} apps") }
                else { format!("built {ok}, FAILED {}: {}", failed.len(), failed.join(", ")) }
            });
        }
        if ui.add_enabled(idle, egui::Button::new("test all")).clicked() {
            app.task.run_logged(move |sink| {
                let mut on_line = |l: &str| sink.push(l);
                let (ok, failed) = cockpit::flow::test_all_logged(&mut on_line);
                if failed.is_empty() { format!("✓ {ok} app suites green") }
                else { format!("{ok} green, FAILED {}: {}", failed.len(), failed.join(", ")) }
            });
        }
        ui.label(egui::RichText::new("runs every app in the catalog, one at a time")
            .weak().small());
    });

    if let Some(r) = app.task.take_result() {
        app.fleet.note = r;
    }
    if app.task.is_running() {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(if app.task.cancelled() { "cancelling…" } else { "building…" });
            // CANCEL, next to the spinner rather than somewhere else: the
            // spinner is what the user is looking at when they want to stop.
            //
            // Disabled once pressed, because the flag is checked between output
            // lines — a second press does nothing and an enabled button that
            // does nothing reads as a broken button.
            if ui.add_enabled(!app.task.cancelled(),
                              egui::Button::new("cancel").small()).clicked() {
                app.task.cancel();
            }
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
    // The batch layer, under the single-app controls above.
    reroll_ui(app, ui);
}
