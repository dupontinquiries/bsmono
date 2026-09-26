//! Push page — build an app HERE for a device's target and install it there. Shows the deploy
//! strategy (local-native / cross-compile / remote-native) + reason, then builds & pushes on the
//! background Task. Mirrors the Qt Push page, calling cockpit directly.
use crate::app::App;
use cockpit::suite::SuiteApp;

pub struct State {
    pub selected: Option<SuiteApp>,
    /// The catalog is showing although an app is chosen ("change app").
    pub list_open: bool,
    /// The catalog's search text.
    pub query: String,
    pub user: String,
    pub host: String,
    pub target: String,
    /// A folder on the device, for transfers. NOT the app install path:
    /// that is fixed at `$HOME/.local/bin` by
    /// `installer::push_install_script`, which also writes the .desktop
    /// entry pointing there.
    pub dest: String,
    /// The open folder picker, when browsing.
    pub browser: Option<crate::remote_browse::Browser>,
    pub strategy: String,
    pub note: String,
    pub subnet: String,
    pub discovered: Vec<(String, String)>, // (ip, hostname)
    pub discovering: bool,
    pub trusted: Option<bool>, // SSH trust status of the selected host (None = unknown/unchecked)
    /// The selected app's cargo features, flipped via a hold-to-open swipe-toggle grid. Populated from
    /// the app's Cargo.toml when the selection changes; committing writes them back to `selected.features`.
    pub flags: Vec<uikit_egui::toggle_grid::ToggleOption>,
    pub flag_grid: uikit_egui::toggle_grid::ToggleGrid,
    /// The app name `flags` were loaded for, so we only rescan on a selection change.
    pub flags_for: String,
    /// Detected build system for the selected app ("Rust (cargo)", "Python", "CMake", …).
    pub build_system: String,
}

impl Default for State {
    fn default() -> Self {
        Self {
            selected: None,
            list_open: false,
            query: String::new(),
            user: "user".to_string(),
            host: String::new(),
            target: "aarch64-unknown-linux-musl".to_string(),
            dest: String::new(),
            browser: None,
            strategy: String::new(),
            note: String::new(),
            subnet: String::new(),
            discovered: Vec::new(),
            discovering: false,
            trusted: None,
            flags: Vec::new(),
            flag_grid: uikit_egui::toggle_grid::ToggleGrid::new("push-build-flags").columns(3),
            flags_for: String::new(),
            build_system: String::new(),
        }
    }
}

pub fn ui(app: &mut App, ui: &mut egui::Ui) {
    ui.heading("Push");
    ui.label(egui::RichText::new("build here for the device's target, send the binary, install it").weak().small());
    ui.separator();

    // The same catalog list as Fleet (`app_list`), in place of a combo box
    // whose popup showed a handful of rows at a time in its own scroll.
    match app.push.selected.clone() {
        Some(sel) if !app.push.list_open => {
            if crate::app_list::chosen_ui(ui, &sel, &sel.path) { app.push.list_open = true; }
        }
        current => {
            ui.label(egui::RichText::new("app").strong());
            if let Some(a) = crate::app_list::list_ui(ui, "push", &mut app.push.query, current.as_ref().map(|s| s.name.as_str())) {
                app.push.selected = Some(a);
                app.push.list_open = false;
            }
            ui.separator();
        }
    }

    // discover devices on the LAN and pick one (fills host), instead of typing it
    egui::CollapsingHeader::new("discover devices").show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label("subnet");
            ui.add(egui::TextEdit::singleline(&mut app.push.subnet).id_salt("push-app_push_subnet"));
            ui.label(egui::RichText::new("(blank = auto)").weak().small());
            if ui.add_enabled(!app.task.is_running(), egui::Button::new("scan")).clicked() {
                app.push.discovering = true;
                let subnet = app.push.subnet.clone();
                app.task.run(move || {
                    // A typed subnet is an instruction; otherwise EVERY attached
                    // private /24. Auto-detect used to follow the default route
                    // only, so a phone on a USB tether -- a second interface --
                    // was invisible on a machine cabled to it.
                    let prefixes: Vec<String> = if subnet.trim().is_empty() {
                        cockpit::deploy::local_subnet24s()
                    } else {
                        vec![subnet.trim().to_string()]
                    };
                    if prefixes.is_empty() {
                        return serde_json::json!({
                            "error": cockpit::deploy::explain_empty_scan(&prefixes)
                        }).to_string();
                    }
                    let devs: Vec<_> = prefixes.iter()
                        .flat_map(|p| cockpit::deploy::scan_ssh(p, std::time::Duration::from_millis(300), 64))
                        .collect();
                    let arr: Vec<_> = devs.iter().map(|d| serde_json::json!({"ip": d.ip, "hostname": d.hostname})).collect();
                    serde_json::json!({"devices": arr}).to_string()
                });
            }
        });
        for (ip, hostname) in app.push.discovered.clone() {
            let label = if hostname.is_empty() { ip.clone() } else { format!("{ip}  ·  {hostname}") };
            if ui.selectable_label(app.push.host == ip, label).clicked() {
                app.push.trusted = Some(cockpit::deploy::host_trusted(&ip));
                app.push.host = ip;
            }
        }
    });

    ui.horizontal(|ui| {
        ui.label("user");
        ui.add(egui::TextEdit::singleline(&mut app.push.user).id_salt("push-app_push_user"));
        ui.label("host");
        ui.add(egui::TextEdit::singleline(&mut app.push.host).id_salt("push-app_push_host"));
    });
    if !app.push.host.trim().is_empty() {
        ui.horizontal(|ui| {
            if ui.button("check trust").clicked() {
                app.push.trusted = Some(cockpit::deploy::host_trusted(&app.push.host));
            }
            match app.push.trusted {
                Some(true) => {
                    ui.colored_label(egui::Color32::from_rgb(0x9e, 0xce, 0x6a), "✓ host key trusted");
                }
                Some(false) => {
                    ui.colored_label(egui::Color32::from_rgb(0xe0, 0xaf, 0x68), "host key not in known_hosts");
                    if ui.add_enabled(!app.task.is_running(), egui::Button::new("trust host key")).clicked() {
                        let h = app.push.host.clone();
                        app.task.run(move || match cockpit::deploy::trust_host(&h) {
                            Ok(m) => format!("trusted host key: {m}"),
                            Err(e) => format!("error: {e}"),
                        });
                        app.push.trusted = None; // re-check after the keyscan finishes
                    }
                }
                None => {}
            }
        });
    }
    ui.horizontal_wrapped(|ui| {
        crate::narrow::field(ui, "target", "push-app_push_target", &mut app.push.target, 0.45);
        if ui.button("check strategy").clicked() {
            let host_native = cockpit::buildflags::normalize_native(&cockpit::ops::native_triple_local());
            let path = std::env::var("PATH").unwrap_or_default();
            let have_cross = cockpit::sysdeps::rust_target_installed(&app.push.target, &path);
            let have_podman = cockpit::podman::which_podman(&path);
            let reachable = !app.push.host.trim().is_empty();
            let c = cockpit::buildflags::recommend_strategy(&host_native, &app.push.target, have_cross, have_podman, reachable);
            app.push.strategy = format!("{:?} — {}", c.strategy, c.reason);
        }
    });
    // WHERE ON THE DEVICE IT LANDS. `remote_browse` has existed with
    // tests since the port and nothing constructed it.
    //
    // READ THIS BEFORE WIRING `dest` INTO THE INSTALL. The app install
    // path does NOT take a destination and should not: `deploy_app_remote`
    // scp's to `/tmp/<app>` as a staging step and then runs
    // `installer::push_install_script`, which moves it to
    // `$HOME/.local/bin` and writes a .desktop entry beside it. That
    // pairing is the contract — a binary somewhere else would not be on
    // PATH and its launcher entry would point at nothing.
    //
    // So this browser is for the OTHER destinations the page deals in
    // (artifact and file transfer), not for relocating an installed app.
    // Labelled accordingly rather than left as a general "install to",
    // which is what I first wrote and which would have promised
    // something the install path cannot honour.
    ui.horizontal(|ui| {
        ui.label("device folder");
        ui.add(egui::TextEdit::singleline(&mut app.push.dest).id_salt("push-app_push_dest"));
        if ui.button("browse…").clicked() {
            let mut b = crate::remote_browse::Browser::new(&app.push.user, &app.push.host);
            // Starting at the destination already typed rather than at
            // `/`: someone who half-remembers the path gets to correct
            // it instead of walking the tree from the root.
            let start = if app.push.dest.trim().is_empty() { "/" } else { app.push.dest.trim() };
            b.open(start, cockpit::deploy::list_remote_dirs);
            app.push.browser = Some(b);
        }
    });
    let (mut picked, mut dismiss) = (None::<String>, false);
    if let Some(b) = app.push.browser.as_mut() {
        // The closure cannot touch `app` — it already holds `b`, which
        // borrows `app.push.browser` (E0500). So the buttons record an
        // INTENT and the decisions are applied after the borrow ends.
        // Same shape as the borrow splits in the chat tab and the fleet
        // page; fourth time this week.
        let (mut chosen, mut close) = (None::<String>, false);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(b.current().unwrap_or("/")).monospace().small());
            if b.can_go_up() && ui.small_button("up").clicked() {
                b.up(cockpit::deploy::list_remote_dirs);
            }
            if ui.small_button("use this").clicked() {
                chosen = b.choose().map(str::to_string);
                close = true;
            }
            if ui.small_button("cancel").clicked() {
                close = true;
            }
        });
        // Entries drawn after the controls so a long listing cannot push
        // "cancel" off the bottom — the one button a person needs when a
        // browse goes somewhere unexpected.
        //
        // Every state is shown, including Failed. `remote_browse` keeps a
        // failure as a STATE rather than a popup precisely so the path
        // stack survives, and swallowing it here would waste that: a
        // permission error two levels deep should say so and leave the
        // navigation intact.
        match &b.state {
            crate::remote_browse::State::Loading => {
                ui.label(egui::RichText::new("listing…").weak().small());
            }
            crate::remote_browse::State::Failed(e) => {
                ui.label(egui::RichText::new(e).small().color(egui::Color32::from_rgb(220, 120, 90)));
            }
            crate::remote_browse::State::Idle => {}
            crate::remote_browse::State::Ready(listing) => {
                let dirs: Vec<String> = listing.dirs.clone();
                egui::ScrollArea::vertical().max_height(160.0).show(ui, |ui| {
                    if dirs.is_empty() {
                        ui.label(egui::RichText::new("no subdirectories").weak().small());
                    }
                    for name in dirs {
                        if ui.selectable_label(false, &name).clicked() {
                            b.enter(&name, cockpit::deploy::list_remote_dirs);
                        }
                    }
                });
            }
        }
        // Applied here, outside the closure but still inside the borrow
        // — so the writes go through locals captured above rather than
        // through `app`.
        if let Some(p) = chosen.take() {
            picked = Some(p);
        }
        dismiss = close;
    }
    if let Some(p) = picked {
        app.push.dest = p;
    }
    if dismiss {
        app.push.browser = None;
    }
    if !app.push.strategy.is_empty() {
        ui.label(egui::RichText::new(&app.push.strategy).color(egui::Color32::from_rgb(0x7a, 0xa2, 0xf7)).small());
    }

    // Bootstrap rustup on the device over SSH — for RemoteNative builds, the device needs rustup to add
    // its target's std ("rustup isn't installed" on a fresh device is fixed here, no manual ssh).
    ui.horizontal(|ui| {
        let can_boot = !app.task.is_running() && !app.push.host.trim().is_empty();
        if ui.add_enabled(can_boot, egui::Button::new("bootstrap rustup on device")).clicked() {
            let (u, h, t) = (app.push.user.clone(), app.push.host.clone(), app.push.target.clone());
            app.task.run(move || match cockpit::deploy::bootstrap_rustup_remote(&u, &h, &t) {
                Ok(o) => format!("✓ rustup ready on {h}\n{o}"),
                Err(e) => format!("error: {e}"),
            });
        }
        crate::narrow::desc(ui, "installs rustup + the target's std on the device (idempotent)");
    });

    // Populate the flag grid from the SELECTED app's real cargo features (rescanned on change).
    if let Some(sel) = &app.push.selected {
        if app.push.flags_for != sel.name {
            let toml = std::fs::read_to_string(format!("{}/Cargo.toml", sel.path)).unwrap_or_default();
            let feats = cockpit::buildflags::scan_cargo_features(&toml);
            let defaults = cockpit::buildflags::arch_default_features(&toml, &app.push.target);
            let on_set: std::collections::HashSet<&String> = sel.features.iter().collect();
            app.push.flags = feats
                .all
                .iter()
                .map(|f| {
                    // on if the app already enables it, or (no explicit set) it's an arch default
                    let on = on_set.contains(f) || (sel.features.is_empty() && defaults.contains(f));
                    uikit_egui::toggle_grid::ToggleOption::new(f.clone(), f.clone(), on)
                })
                .collect();
            // detect the build system (Rust cargo / Python / Node / Go / CMake / Make)
            app.push.build_system = cockpit::buildsystem::detect(std::path::Path::new(&sel.path))
                .map(|d| d.kind.label().to_string())
                .unwrap_or_default();
            app.push.flags_for = sel.name.clone();
        }
    }
    if !app.push.build_system.is_empty() {
        ui.horizontal(|ui| {
            ui.label("build system");
            ui.label(egui::RichText::new(&app.push.build_system).strong().color(egui::Color32::from_rgb(0x7a, 0xa2, 0xf7)));
        });
    }

    // A ONE-TAP PRESET for tideshell specifically: wayland+layer together is
    // what makes it a real docked shell under COSMIC rather than a preview
    // window with no input method (see cockpit::buildflags's doc on why those
    // two features do not imply each other). Shown only for tideshell, since
    // the pairing is meaningless for any other app.
    if app.push.selected.as_ref().is_some_and(|s| s.name.contains("tideshell")) {
        let mut on = cockpit::buildflags::is_cosmic_tideshell(
            app.push.selected.as_ref().map(|s| s.features.as_slice()).unwrap_or(&[]));
        if ui.checkbox(&mut on, "COSMIC + tideshell (wayland + layer)").changed() {
            if let Some(sel) = &mut app.push.selected {
                sel.features = if on {
                    cockpit::buildflags::with_cosmic_tideshell(sel.features.clone())
                } else {
                    sel.features.iter().cloned()
                        .filter(|f| !cockpit::buildflags::COSMIC_TIDESHELL_FEATURES.contains(&f.as_str()))
                        .collect()
                };
                sel.no_default = true;
                // Keep the swipe-grid checkboxes in sync, so a person who
                // opens it afterwards sees the same state this switch set.
                let now_on: std::collections::HashSet<&String> = sel.features.iter().collect();
                for f in &mut app.push.flags {
                    f.on = now_on.contains(&f.id);
                }
            }
        }
        crate::narrow::desc(ui, "turns on the docked panel AND its input method together — one                                  without the other showed a keyboard that never hid, or a shell                                  with no window focus");
    }

    // Build features — hold the button and swipe across the grid to flip features, release to confirm.
    if !app.push.flags.is_empty() {
        ui.horizontal(|ui| {
            ui.label("features");
            let host = ui.add(egui::Button::new("⚑ hold + swipe"));
            if let uikit_egui::toggle_grid::ToggleOutcome::Committed(new) =
                app.push.flag_grid.show(ui, &host, &app.push.flags)
            {
                for (id, on) in &new {
                    if let Some(f) = app.push.flags.iter_mut().find(|f| &f.id == id) {
                        f.on = *on;
                    }
                }
                // thread the choice into the build: SuiteApp.features is what the build consumes
                if let Some(sel) = &mut app.push.selected {
                    sel.features = new.iter().filter(|(_, on)| *on).map(|(id, _)| id.clone()).collect();
                    sel.no_default = true; // explicit feature set → don't also pull defaults
                }
            }
            let on: Vec<&str> = app.push.flags.iter().filter(|f| f.on).map(|f| f.label.as_str()).collect();
            let summary = if on.is_empty() { "(no features)".to_string() } else { on.join(" · ") };
            ui.label(egui::RichText::new(summary).weak().small());
        });
    }

    let can = !app.task.is_running() && app.push.selected.is_some() && !app.push.host.trim().is_empty();
    if ui.add_enabled(can, egui::Button::new("Build & push")).clicked() {
        let a = app.push.selected.clone().unwrap();
        let (t, u, h) = (app.push.target.clone(), app.push.user.clone(), app.push.host.clone());
        app.task.run_logged(move |sink| {
            let mut on_line = |l: &str| sink.push(l);
            match cockpit::flow::deploy_app_remote_logged(&a, &t, &u, &h, &mut on_line) {
                Ok(o) => format!("✓ {o}"),
                Err(e) => format!("error: {e}"),
            }
        });
    }

    // Build ALL apps in the selected app's group, and install them on the device (cockpit build-all).
    if let Some(sel) = &app.push.selected {
        if let Some(group) =
            cockpit::flow::store_groups().into_iter().find(|g| g.apps.iter().any(|a| a.name == sel.name))
        {
            let n = group.apps.len();
            let can_all = !app.task.is_running() && !app.push.host.trim().is_empty() && n > 1;
            if ui
                .add_enabled(can_all, egui::Button::new(format!("Build & push all · {} ({n})", group.name)))
                .clicked()
            {
                let (t, u, h) = (app.push.target.clone(), app.push.user.clone(), app.push.host.clone());
                app.task.run_logged(move |sink| {
                    let mut on_line = |l: &str| sink.push(l);
                    let results = cockpit::flow::install_group_remote_logged(&group, &t, &u, &h, &mut on_line);
                    let ok = results.iter().filter(|r| r.ok).count();
                    format!("✓ {ok}/{} apps installed", results.len())
                });
            }
        }
    }

    if let Some(r) = app.task.take_result() {
        if app.push.discovering {
            app.push.discovering = false;
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&r) {
                if let Some(devs) = v.get("devices").and_then(|d| d.as_array()) {
                    app.push.discovered = devs
                        .iter()
                        .filter_map(|d| {
                            Some((
                                d.get("ip")?.as_str()?.to_string(),
                                d.get("hostname")?.as_str()?.to_string(),
                            ))
                        })
                        .collect();
                    app.push.note = format!("found {} device(s)", app.push.discovered.len());
                } else if let Some(e) = v.get("error").and_then(|e| e.as_str()) {
                    app.push.note = e.to_string();
                }
            }
        } else {
            app.push.note = r;
        }
    }
    if app.task.is_running() {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label("building + pushing…");
        });
        ui.ctx().request_repaint();
    }
    let log = app.task.log();
    if !log.is_empty() {
        // The kit's shape-based classifier, not contains("error"), which
        // painted `Compiling thiserror` red.
        crate::show::task_log(ui, "push", &log, 200.0);
    }
    if !app.push.note.is_empty() {
        ui.add_space(6.0);
        ui.label(egui::RichText::new(&app.push.note).monospace().small());
    }
}
