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
    /// The catalog is showing although an app is chosen ("change app").
    pub list_open: bool,
    /// The catalog's search text.
    pub query: String,

    // ── the reroll section ──
    /// The remembered fleet. Lives here rather than being rediscovered each
    /// frame: a scan takes seconds and the labels are the operator's, not the
    /// network's.
    pub roster: cockpit::roster::Roster,
    /// Enrolment: choose a device, trust its key, install ours.
    ///
    /// The roster remembers devices and the planner emits Step::Enroll; this
    /// is the only thing in the port that can actually perform one.
    pub picker: crate::device_picker::Picker,
    /// What the last scan changed, shown so the list does not mutate silently.
    pub last_changes: Vec<cockpit::roster::Change>,
    /// The account the fleet logs in as. Every reroll step needs it, so a
    /// run is gated on it rather than defaulting to the local username —
    /// a deploy that silently used the wrong account would half-succeed.
    pub reroll_user: String,
    /// Password for FIRST enrolment only. Empty means "keys are already
    /// installed", which is the normal case after the first run and is
    /// why this is not required.
    ///
    /// Held in memory for the run and never written: `setup_key`
    /// consumes it and nothing else wants it.
    pub reroll_password: String,
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
/// Draw the enrolment picker.
///
/// Every button is gated by the model rather than by what looks reasonable
/// here: `can_trust` and `can_setup_key` are mutually exclusive by
/// construction, and neither is offered from a failed state. A widget that
/// decided this for itself would be a second copy of the rule that keeps a
/// password off an unverified host.
fn picker_ui(ui: &mut egui::Ui, p: &mut crate::device_picker::Picker) {
    use crate::device_picker::Job;
    ui.separator();
    // FIRST CONTACT, before enrolment can happen at all.
    //
    // `join`'s header says it: a fresh postmarketOS image has
    // `user`/`147147` and NO SSHD RUNNING, so every scan-and-trust step
    // below is unreachable until someone types one command on the phone
    // itself. That module has existed with tests and no caller, so the
    // step the operator actually repeats was the one step the UI did not
    // help with.
    //
    // A command to copy rather than anything automatic: there is no
    // network path to a device that is not listening yet, which is the
    // whole definition of this state.
    join_ui(ui, &mut p.join);
    ui.separator();

    ui.label(egui::RichText::new("Enrol a device").strong());
    ui.label(egui::RichText::new(p.status_line()).weak().small());

    if p.devices.is_empty() {
        return;
    }
    egui::ScrollArea::vertical().max_height(120.0).id_salt("picker").show(ui, |ui| {
        for (i, d) in p.devices.clone().iter().enumerate() {
            let label = if d.hostname.is_empty() { d.ip.clone() }
                        else { format!("{}  ({})", d.hostname, d.ip) };
            if ui.selectable_label(p.selected == Some(i), label).clicked() {
                // Trust is READ HERE, at selection, and never cached on the
                // model — a reflash changes the key and this fleet reflashes
                // constantly.
                let trusted = cockpit::deploy::host_trusted(&d.ip);
                p.select(i, trusted);
            }
        }
    });

    ui.horizontal(|ui| {
        if p.can_trust() && ui.button("Trust host key").clicked() {
            if let Some(ip) = p.selected_ip() {
                p.start(Job::Trust);
                // Synchronous: ssh-keyscan against one host on the LAN is
                // sub-second, and a Task here would need a channel for a
                // result the next line already has.
                let r = cockpit::deploy::trust_host(&ip);
                let ok = r.is_ok();
                let msg = match r { Ok(m) => m, Err(e) => e };
                p.finished(Job::Trust, ok, &msg, cockpit::deploy::host_trusted(&ip));
            }
        }
        if p.can_setup_key() {
            ui.label("password:");
            // Bound to a local, not to the model: Picker derives Debug and
            // ends up in logs.
            let pw_id = ui.id().with("pw");
            let mut pw: String = ui.data(|d| d.get_temp(pw_id).unwrap_or_default());
            ui.add(egui::TextEdit::singleline(&mut pw).id_salt("fleet-pw").password(true).desired_width(120.0));
            ui.data_mut(|d| d.insert_temp(pw_id, pw.clone()));
            if ui.button("Install key").clicked() {
                if let Some(ip) = p.selected_ip() {
                    p.start(Job::SetupKey);
                    let r = cockpit::deploy::setup_key(&p.user, &ip, &pw);
                    let ok = r.is_ok();
                    let msg = match r { Ok(m) => m, Err(e) => e };
                    p.finished(Job::SetupKey, ok, &msg, true);
                    ui.data_mut(|d| d.insert_temp(pw_id, String::new()));
                }
            }
        }
    });
}

fn reroll_ui(app: &mut App, ui: &mut egui::Ui) {
    use cockpit::{reroll, roster, runview};
    ui.separator();
    ui.heading("Reroll the fleet");
    crate::narrow::desc(ui, "one pass: pair what needs pairing, build once per architecture, install everywhere");

    let st = &mut app.fleet;

    ui.horizontal(|ui| {
        if ui.button("Scan LAN").clicked() {
            // Real discovery through the code that already exists. The roster
            // absorbs it — a device that moved is updated, one that vanished
            // goes offline, one that was reflashed loses its enrolment.
            // EVERY attached private /24, not just the default route's. A
            // phone on a USB tether is on a second interface, and scanning
            // only the internet-facing subnet finds nothing on a machine that
            // is cabled to the device.
            let subnets = cockpit::deploy::local_subnet24s();
            let seen: Vec<roster::Seen> = match subnets.is_empty() {
                false => subnets.iter()
                    .flat_map(|prefix| cockpit::deploy::scan_ssh(
                        prefix, std::time::Duration::from_millis(300), 64))
                    .map(|d| roster::Seen {
                        ip: d.ip,
                        hostname: d.hostname,
                        // The scan does not read host keys yet, and None means
                        // "unchanged" rather than "reflashed" — inventing a key
                        // here would reset enrolment on every scan.
                        host_key: None,
                    })
                    .collect(),
                // No private network at all: nothing to scan, and saying so
                // beats an empty list that looks like "no devices".
                true => Vec::new(),
            };
            // The SAME scan feeds the picker, so one sweep serves both the
            // roster's bookkeeping and enrolment. Two scan buttons would race
            // each other over the same subnet and disagree about what is out
            // there.
            st.picker.discovered(
                seen.iter()
                    .map(|s| crate::device_picker::Found {
                        ip: s.ip.clone(),
                        hostname: s.hostname.clone(),
                    })
                    .collect(),
            );
            st.last_changes = st.roster.observe(&seen);
        }
        ui.label(format!("{} device(s) on the roster", st.roster.len()));
    });

    picker_ui(ui, &mut st.picker);

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
    // AND WHETHER THE BUILDS WILL OVERLAP. The plan says how many there are; whether they run
    // at once depends on THIS machine's memory, and a queue that quietly runs one at a time on
    // a 64GB workstation looks like the feature was never added. Saying the number also makes
    // the 24GB threshold visible to someone wondering why their laptop is not stacking.
    let host = cockpit::buildflags::detect_host();
    let par = cockpit::buildflags::host_build_plan(host).concurrent;
    let overlap = if par > 1 && plan.build_count() > 1 {
        format!(" ({par} at a time, {}GB)", host.ram_mb / 1024)
    } else if plan.build_count() > 1 {
        format!(" (one at a time — under {}GB)", cockpit::buildflags::STACK_RAM_MB / 1024)
    } else {
        String::new()
    };
    ui.label(format!(
        "plan: {} pairing, {} build(s){overlap}, {} install(s)",
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
        // THE BUTTON, at last. `ssh_runner::SshRunner` is the adapter the
        // label used to say was missing.
        //
        // Gated on a user and at least one device: `execute` walks the
        // plan and every step needs somewhere to log in, so a run with
        // neither would fail once per step with the same message.
        ui.horizontal(|ui| {
            ui.label("fleet user");
            ui.add(egui::TextEdit::singleline(&mut st.reroll_user).id_salt("fleet-st_reroll_user"));
            ui.label("password");
            // `password(true)` so it is not shouldered off a screen. It
            // is only needed for FIRST enrolment — after that the key is
            // installed and this stays empty, which is why the run is
            // not gated on it.
            ui.add(egui::TextEdit::singleline(&mut st.reroll_password).id_salt("fleet-st_reroll_password")
                .password(true).desired_width(120.0));
        });
        ui.label(egui::RichText::new(
            "password is used once to install the key, then never stored")
            .weak().small());
        // Values copied out of `st` before `app.task` is touched: `st` is
        // `&mut app.fleet`, and reaching for another field of `app` while
        // it lives is a borrow conflict. Same split the chat tab needed
        // for its a11y scene.
        let user = st.reroll_user.trim().to_string();
        let pw = st.reroll_password.clone();
        let can_run = !app.task.is_running()
            && !user.is_empty()
            && !devices.is_empty();
        let n = plan.steps.len();
        if ui
            .add_enabled(can_run, egui::Button::new(format!("Run reroll · {n} steps")))
            .clicked()
        {
            // (id, host, triple) triples, owned: the run takes minutes on
            // a worker thread and must not hold the UI's roster.
            let devs: Vec<(String, String, String)> = devices
                .iter()
                .map(|d| (d.id.clone(), d.host.clone(), d.triple.clone()))
                .collect();
            let plan_owned = plan.clone();
            let (user, pw) = (user.clone(), pw.clone());
            app.task.run_logged(move |sink| {
                let cred = if pw.trim().is_empty() {
                    crate::ssh_runner::Credential::KeyAlreadyInstalled
                } else {
                    crate::ssh_runner::Credential::Password(pw)
                };
                let mut r = crate::ssh_runner::SshRunner::new(&user, devs, cred);
                let report = {
                    let mut on = |e: reroll::Event| match e {
                        reroll::Event::Started { steps } => sink.push(&format!("── {steps} steps ──")),
                        // StepStarted is emitted BEFORE the work, so a
                        // step that hangs shows as the thing that hung.
                        // Logging it is the whole reason it exists.
                        reroll::Event::StepStarted(s) => sink.push(&format!("· {s:?}")),
                        reroll::Event::StepDone(s) => sink.push(&format!("✓ {s:?}")),
                        reroll::Event::StepFailed { step, error } =>
                            sink.push(&format!("✗ {step:?} — {error}")),
                        reroll::Event::StepSkipped { step, because } =>
                            sink.push(&format!("– {step:?} (skipped: {because})")),
                        reroll::Event::Finished => sink.push("── done ──"),
                    };
                    reroll::run(&plan_owned, &mut r, &mut on)
                };
                // The runner's own log AFTER the events: it carries the
                // build output, which is where a compile error actually
                // appears. Dropping it would leave "install failed" with
                // nothing to act on.
                for l in &r.log { sink.push(l); }
                format!("reroll finished — {} device(s) reported", report.per_device.len())
            });
        }
        if !can_run && !app.task.is_running() {
            ui.label(egui::RichText::new(
                "set a fleet user and enrol at least one device to run")
                .weak().small());
        }
        let _ = runview::RunView::with_devices(&[]);
    }
}

pub fn ui(app: &mut App, ui: &mut egui::Ui) {
    ui.heading("Fleet");
    ui.label(egui::RichText::new("build + install BigSilicon apps from the catalog").weak().small());
    ui.separator();

    // THE CATALOG: part of the page's scroll (it was a 280 px box scrolling
    // inside it), searchable, and folded to the chosen app once one is
    // picked so the build controls are right below — see `app_list`.
    let listing = app.fleet.selected.is_none() || app.fleet.list_open;
    if listing {
        let current = app.fleet.selected.as_ref().map(|s| s.name.clone());
        if let Some(a) = crate::app_list::list_ui(ui, "fleet", &mut app.fleet.query, current.as_deref()) {
            app.fleet.selected = Some(a);
            app.fleet.list_open = false;
        }
    }

    ui.separator();
    if let Some(sel) = app.fleet.selected.clone() {
        if !listing && crate::app_list::chosen_ui(ui, &sel, &sel.path) {
            app.fleet.list_open = true;
        }
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
    //
    // Layout: on a narrow screen a row of six buttons (build all / test all /
    // copy log / copy errors / copy times / export) doesn't fit — so narrow
    // unifies all six into ONE press-hold matrix (`fleet_actions::full_menu`,
    // the same `uikit_egui::slide_menu` widget voiceterm-egui's block-row
    // "⋯" menu uses). Wide keeps the two dedicated buttons (muscle memory,
    // one tap instead of hold-then-pick) and adds a small "⋯" matrix next to
    // them for just the four copy/export actions, which never had ANY UI
    // before this — `fleet_actions::utility_menu`.
    ui.add_space(8.0);
    ui.separator();
    ui.add_space(4.0);
    let compact = uikit_egui::adaptive::Adaptive::detect(ui.ctx()).is_compact();
    ui.horizontal(|ui| {
        let idle = !app.task.is_running();
        if compact {
            let host = ui.add_enabled(idle, egui::Button::new("\u{22EF} actions"));
            let id = egui::Id::new("gitmind-fleet-actions-compact");
            if let uikit_egui::slide_menu::SlideOutcome::Selected(chosen, _) =
                uikit_egui::slide_menu::SlideMenu::new(id).columns(3).show_stored(ui, id, &host, &crate::fleet_actions::full_menu())
            {
                if let Some(a) = crate::fleet_actions::FleetAction::from_id(&chosen) {
                    run_fleet_action(a, app, ui);
                }
            }
        } else {
            if ui.add_enabled(idle, egui::Button::new("build all")).clicked() {
                run_fleet_action(crate::fleet_actions::FleetAction::BuildAll, app, ui);
            }
            if ui.add_enabled(idle, egui::Button::new("test all")).clicked() {
                run_fleet_action(crate::fleet_actions::FleetAction::TestAll, app, ui);
            }
            let host = ui.button("\u{22EF}").on_hover_text("Copy or export the last run's log");
            let id = egui::Id::new("gitmind-fleet-actions-utility");
            if let uikit_egui::slide_menu::SlideOutcome::Selected(chosen, _) =
                uikit_egui::slide_menu::SlideMenu::new(id).columns(2).show_stored(ui, id, &host, &crate::fleet_actions::utility_menu())
            {
                if let Some(a) = crate::fleet_actions::FleetAction::from_id(&chosen) {
                    run_fleet_action(a, app, ui);
                }
            }
            ui.label(egui::RichText::new("runs every app in the catalog, one at a time")
                .weak().small());
        }
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
        crate::show::task_log(ui, "fleet", &log, 200.0);
    }
    if !app.fleet.note.is_empty() {
        ui.add_space(6.0);
        ui.label(egui::RichText::new(&app.fleet.note).monospace().small());
    }
    // The batch layer, under the single-app controls above.
    reroll_ui(app, ui);
}

/// Run one [`crate::fleet_actions::FleetAction`]. `BuildAll`/`TestAll` start a
/// new `Task` run exactly as the old dedicated buttons did (same closures,
/// moved here so both layouts — the wide row and the compact matrix — share
/// one implementation instead of two copies that could drift). The four
/// copy/export actions read whatever the `Task` last captured — a build-all,
/// a test-all, or a per-app build/test alike — so they work regardless of
/// which button produced the log being inspected.
fn run_fleet_action(action: crate::fleet_actions::FleetAction, app: &mut App, ui: &mut egui::Ui) {
    use crate::fleet_actions::FleetAction;
    match action {
        FleetAction::BuildAll => {
            app.task.run_logged(move |sink| {
                let mut on_line = |l: &str| sink.push(l);
                let (ok, failed) = cockpit::flow::build_all_logged(&mut on_line);
                // Report BOTH counts. "built 23 apps" with three silent
                // failures is the line that makes a broken fleet look healthy.
                if failed.is_empty() { format!("✓ built {ok} apps") }
                else { format!("built {ok}, FAILED {}: {}", failed.len(), failed.join(", ")) }
            });
        }
        FleetAction::TestAll => {
            app.task.run_logged(move |sink| {
                let mut on_line = |l: &str| sink.push(l);
                let (ok, failed) = cockpit::flow::test_all_logged(&mut on_line);
                if failed.is_empty() { format!("✓ {ok} app suites green") }
                else { format!("{ok} green, FAILED {}: {}", failed.len(), failed.join(", ")) }
            });
        }
        FleetAction::CopyLog => {
            let log = app.task.log().join("\n");
            if log.is_empty() {
                app.fleet.note = "nothing to copy — run build/test first".to_string();
            } else {
                ui.ctx().copy_text(log);
                app.fleet.note = "log copied".to_string();
            }
        }
        FleetAction::CopyErrors => {
            let errs = cockpit::build_report::errors_only(&app.task.timed_log());
            if errs.is_empty() {
                app.fleet.note = "no error lines in the last run".to_string();
            } else {
                ui.ctx().copy_text(errs);
                app.fleet.note = "errors copied".to_string();
            }
        }
        FleetAction::CopyTimes => {
            let times = cockpit::build_report::times_by_crate(&app.task.timed_log());
            if times.is_empty() {
                app.fleet.note = "no per-crate timing found in the last run".to_string();
            } else {
                let text: String = times.iter().map(|(n, ms)| format!("{n}: {:.1}s\n", *ms as f64 / 1000.0)).collect();
                ui.ctx().copy_text(text);
                app.fleet.note = format!("build times copied ({} entries)", times.len());
            }
        }
        FleetAction::Export => {
            let lines = app.task.timed_log();
            if lines.is_empty() {
                app.fleet.note = "nothing to export — run build/test first".to_string();
            } else {
                // Derived from the log, not hard-coded. These were
                // `ok: 0, failed: Vec::new()`, so every exported report
                // opened with "ok: 0 / failed: none" even when the log
                // below it carried `error[E0433]` and `could not
                // compile` — a header contradicting its own body, and
                // the half a reader trusts.
                let (ok, failed) = cockpit::build_report::tally(&lines);
                let meta = cockpit::build_report::ReportMeta {
                    title: "fleet run".to_string(), ok, failed };
                match cockpit::build_report::export(&cockpit::build_report::default_export_dir(), &meta, &lines) {
                    Ok(path) => {
                        let shown = path.display().to_string();
                        ui.ctx().copy_text(shown.clone());
                        app.fleet.note = format!("exported to {shown} (path copied)");
                    }
                    Err(e) => app.fleet.note = format!("export failed: {e}"),
                }
            }
        }
    }
}

/// "Join a new phone": the join server (cockpit::join_server) for a phone with a browser and
/// curl and nothing else. The phone opens the URL, types the code, taps the matching number,
/// and runs the command it is given; this panel shows each step's laptop half.
fn join_ui(ui: &mut egui::Ui, j: &mut crate::device_picker::JoinPanel) {
    use cockpit::join_server::Stage;
    let addrs = cockpit::join_server::local_addresses();
    if j.host.is_empty() {
        j.host = crate::device_picker::preferred_join_host(&addrs, cockpit::deploy::local_ipv4().as_deref()).unwrap_or_default();
    }
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("no sshd yet?").weak().small());
        if j.running.is_none() {
            egui::ComboBox::from_id_salt("join-host").selected_text(if j.host.is_empty() { "no address" } else { j.host.as_str() }).show_ui(ui, |ui| {
                for (ifname, ip) in &addrs {
                    ui.selectable_value(&mut j.host, ip.clone(), format!("{ip}  ({ifname})"));
                }
            });
            if ui.small_button("join a new phone").clicked() {
                j.error.clear();
                match cockpit::deploy::local_identity_pubkey() {
                    None => j.error = "no ~/.ssh key to install on the phone: run `ssh-keygen -t ed25519` first".into(),
                    Some(key) if !j.host.is_empty() => {
                        let user = cockpit::fleet::fleet_user();
                        let r = cockpit::join_server::serve(8099, &j.host, &user, key.trim())
                            .or_else(|_| cockpit::join_server::serve(0, &j.host, &user, key.trim()));
                        match r {
                            Ok(run) => j.running = Some(std::sync::Arc::new(run)),
                            Err(e) => j.error = e,
                        }
                    }
                    Some(_) => j.error = "no local IPv4 address to offer the phone".into(),
                }
            }
        } else if ui.small_button("stop join").clicked() {
            j.running = None;
        }
    });
    if !j.error.is_empty() {
        ui.colored_label(ui.visuals().warn_fg_color, &j.error);
    }
    let Some(run) = &j.running else {
        ui.label(egui::RichText::new("the phone needs only a browser and curl: open the address, type the code, tap the matching number, run the command it shows").weak().small());
        return;
    };
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
    let s = match run.session.lock() { Ok(s) => s, Err(_) => return };
    ui.label(egui::RichText::new(format!("on the phone, open  http://{}:{}/", j.host, run.port)).monospace());
    match &s.stage {
        Stage::Login => {
            ui.label("code for the phone:");
            ui.label(egui::RichText::new(&s.code.code).monospace().size(28.0).strong());
        }
        Stage::Matching { .. } => {
            ui.label("the phone is choosing -- tap this number on it:");
            ui.label(egui::RichText::new(s.laptop_number().map(|n| n.to_string()).unwrap_or_default()).size(40.0).strong());
        }
        Stage::Confirmed { .. } => {
            ui.label("the phone has the command. Script fingerprint -- it must match what the phone shows:");
            ui.label(egui::RichText::new(s.fingerprint().unwrap_or_default()).monospace().size(22.0).strong());
        }
        Stage::Joined { report, peer } => {
            ui.label(egui::RichText::new(format!("joined: {} ({}) at {peer}", report.hostname, report.arch)).strong());
            ui.label(egui::RichText::new(format!("host key {}  --  next: trust it and install the key from the list below, as {}@{peer}", report.host_key, cockpit::fleet::fleet_user())).small());
        }
        Stage::Closed(why) => {
            ui.colored_label(ui.visuals().warn_fg_color, format!("join closed: {why}"));
        }
    }
}

#[cfg(test)]
mod join_panel_tests {
    use super::*;

    #[test]
    #[allow(deprecated)]
    fn the_join_panel_draws_every_stage_of_a_real_session() {
        let run = cockpit::join_server::serve(0, "127.0.0.1", "gitmind", "ssh-ed25519 AAAAkey t").unwrap();
        let mut j = crate::device_picker::JoinPanel { running: Some(std::sync::Arc::new(run)), host: "127.0.0.1".into(), error: String::new() };
        let ctx = egui::Context::default();
        let mut frame = |j: &mut crate::device_picker::JoinPanel| {
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| join_ui(ui, j));
            });
        };
        let now = cockpit::join_server::now_secs();
        let session = j.running.as_ref().unwrap().session.clone();
        frame(&mut j); // Login: the code
        let code = session.lock().unwrap().code.code.clone();
        session.lock().unwrap().handle("POST", "/login", &format!("code={code}"), "x", now);
        frame(&mut j); // Matching: the number
        let n = session.lock().unwrap().laptop_number().unwrap();
        session.lock().unwrap().handle("POST", "/pick", &format!("n={n}"), "x", now);
        frame(&mut j); // Confirmed: the fingerprint
        let token = match session.lock().unwrap().stage.clone() { cockpit::join_server::Stage::Confirmed { token } => token, s => panic!("{s:?}") };
        session.lock().unwrap().handle("POST", &format!("/joined/{token}"), "hostname=fp5&host_key=k&arch=aarch64", "172.16.42.1", now);
        frame(&mut j); // Joined
        assert!(matches!(session.lock().unwrap().stage, cockpit::join_server::Stage::Joined { .. }));
        j.running = None;
        frame(&mut j); // idle, with the hint
    }
}
