//! Push page — build an app HERE for a device's target and install it there. Shows the deploy
//! strategy (local-native / cross-compile / remote-native) + reason, then builds & pushes on the
//! background Task. Mirrors the Qt Push page, calling cockpit directly.
use crate::app::App;
use cockpit::suite::SuiteApp;

pub struct State {
    pub selected: Option<SuiteApp>,
    pub user: String,
    pub host: String,
    pub target: String,
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
            user: "user".to_string(),
            host: String::new(),
            target: "aarch64-unknown-linux-musl".to_string(),
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

    let cur = app.push.selected.as_ref().map(|s| s.name.clone()).unwrap_or_else(|| "(pick an app)".to_string());
    egui::ComboBox::from_label("app").selected_text(cur).show_ui(ui, |ui| {
        for suite in cockpit::flow::store_groups() {
            for a in suite.apps {
                if ui.selectable_label(false, &a.name).clicked() {
                    app.push.selected = Some(a);
                }
            }
        }
    });

    // discover devices on the LAN and pick one (fills host), instead of typing it
    egui::CollapsingHeader::new("discover devices").show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label("subnet");
            ui.text_edit_singleline(&mut app.push.subnet);
            ui.label(egui::RichText::new("(blank = auto)").weak().small());
            if ui.add_enabled(!app.task.is_running(), egui::Button::new("scan")).clicked() {
                app.push.discovering = true;
                let subnet = app.push.subnet.clone();
                app.task.run(move || {
                    let prefix = if subnet.trim().is_empty() {
                        cockpit::deploy::local_subnet24().unwrap_or_default()
                    } else {
                        subnet.trim().to_string()
                    };
                    if prefix.is_empty() {
                        return serde_json::json!({"error": "could not detect subnet — type one like 192.168.1"}).to_string();
                    }
                    let devs = cockpit::deploy::scan_ssh(&prefix, std::time::Duration::from_millis(300), 64);
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
        ui.text_edit_singleline(&mut app.push.user);
        ui.label("host");
        ui.text_edit_singleline(&mut app.push.host);
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
    ui.horizontal(|ui| {
        ui.label("target");
        ui.text_edit_singleline(&mut app.push.target);
        if ui.button("check strategy").clicked() {
            let host_native = cockpit::buildflags::normalize_native(&cockpit::ops::native_triple_local());
            let path = std::env::var("PATH").unwrap_or_default();
            let have_cross = cockpit::sysdeps::rust_target_installed(&app.push.target, &path);
            let reachable = !app.push.host.trim().is_empty();
            let c = cockpit::buildflags::recommend_strategy(&host_native, &app.push.target, have_cross, reachable);
            app.push.strategy = format!("{:?} — {}", c.strategy, c.reason);
        }
    });
    if !app.push.strategy.is_empty() {
        ui.label(egui::RichText::new(&app.push.strategy).color(egui::Color32::from_rgb(0x7a, 0xa2, 0xf7)).small());
    }

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
        use uikit_egui::log_view::{log_row, LogEntry, LogLevel};
        let level_of = |line: &str| {
            let l = line.to_ascii_lowercase();
            if l.contains("error") || l.contains('✗') || l.contains("failed") {
                LogLevel::Error
            } else if l.contains("warn") {
                LogLevel::Warn
            } else if line.trim_start().starts_with('✓') || l.contains("finished") || l.contains("success") {
                LogLevel::Info
            } else {
                LogLevel::Debug
            }
        };
        egui::ScrollArea::vertical()
            .max_height(200.0)
            .stick_to_bottom(true)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for (i, line) in log.iter().enumerate() {
                    log_row(ui, &LogEntry::new(level_of(line), line.clone(), i as u64));
                }
            });
    }
    if !app.push.note.is_empty() {
        ui.add_space(6.0);
        ui.label(egui::RichText::new(&app.push.note).monospace().small());
    }
}
