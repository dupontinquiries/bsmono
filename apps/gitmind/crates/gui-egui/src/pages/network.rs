//! Network page — the fleet's daemons + a target device, calling cockpit directly (no JSON marshal
//! layer the Qt frontend needed). Per-daemon lifecycle control, one-shot logs, and a *live* log
//! stream (via the Task's run_logged + cancel). Remote actions run on the background Task so the
//! frame never freezes.
use crate::app::App;

#[derive(Default)]
pub struct State {
    pub user: String,
    pub host: String,
    pub note: String,
    /// Which daemon is currently streaming ("" = none), so the toggle knows its label.
    pub streaming_name: String,
    /// A session roster of devices (name, user, host) to roll daemon health up across.
    pub health_targets: Vec<(String, String, String)>,
    /// Third-party services discovered on a device (no fleet marker): (name, description).
    pub other_services: Vec<(String, String)>,
}

pub fn ui(app: &mut App, ui: &mut egui::Ui) {
    ui.heading("Network");
    ui.separator();

    ui.horizontal(|ui| {
        ui.label("user");
        ui.text_edit_singleline(&mut app.net.user);
        ui.label("host");
        ui.text_edit_singleline(&mut app.net.host);
    });
    let idle = !app.task.is_running();
    let can = idle && !app.net.host.trim().is_empty();

    ui.add_space(6.0);
    ui.label(egui::RichText::new("fleet daemons · by the developer").strong());
    ui.label(
        egui::RichText::new("first-party services with core-kit integrations — auto-discovered via the X-Fleet-Integration unit marker")
            .weak()
            .small(),
    );
    for d in cockpit::daemons::all() {
        ui.horizontal(|ui| {
            ui.label(format!("● {}", d.name));
            // the developer/integration marker — a coloured chip so first-party daemons read at a glance
            egui::Frame::new()
                .fill(egui::Color32::from_rgb(0x2a, 0x3a, 0x5a))
                .inner_margin(egui::Margin::symmetric(5, 0))
                .corner_radius(egui::CornerRadius::same(4))
                .show(ui, |ui| {
                    ui.label(egui::RichText::new(d.integration).small().color(egui::Color32::from_rgb(0x9a, 0xc2, 0xff)));
                });
            ui.label(egui::RichText::new(d.description).weak().small());
        });
        ui.horizontal(|ui| {
            for action in ["start", "stop", "restart"] {
                if ui.add_enabled(can, egui::Button::new(action).small()).clicked() {
                    let (u, h, n, a) =
                        (app.net.user.clone(), app.net.host.clone(), d.name.to_string(), action.to_string());
                    app.task.run(move || match cockpit::flow::daemon_control_remote(&u, &h, &n, &a) {
                        Ok(o) => format!("{n} {a}: {o}"),
                        Err(e) => format!("error: {e}"),
                    });
                }
            }
            if ui.add_enabled(can, egui::Button::new("logs").small()).clicked() {
                let (u, h, n) = (app.net.user.clone(), app.net.host.clone(), d.name.to_string());
                app.task.run(move || match cockpit::flow::daemon_logs_remote(&u, &h, &n, 200) {
                    Ok(o) => o,
                    Err(e) => format!("error: {e}"),
                });
            }
            let streaming_this = app.net.streaming_name == d.name;
            let label = if streaming_this { "◼ stop" } else { "▶ stream" };
            let stream_enabled = !app.net.host.trim().is_empty() && (idle || streaming_this);
            if ui.add_enabled(stream_enabled, egui::Button::new(label).small()).clicked() {
                if streaming_this {
                    app.task.cancel();
                    app.net.streaming_name.clear();
                } else if idle {
                    let (u, h, n) = (app.net.user.clone(), app.net.host.clone(), d.name.to_string());
                    let cancel = app.task.cancel_flag();
                    app.net.streaming_name = d.name.to_string();
                    app.task.run_logged(move |sink| {
                        let mut on_line = |l: &str| sink.push(l);
                        match cockpit::flow::stream_daemon_logs(&u, &h, &n, cancel, &mut on_line) {
                            Ok(()) => "— stream ended —".to_string(),
                            Err(e) => format!("error: {e}"),
                        }
                    });
                }
            }
        });
    }

    ui.add_space(6.0);
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

    // Third-party services: anything on the system WITHOUT the X-Fleet-Integration marker. These
    // aren't by the developer and carry no core-kit integration, so they're listed apart from the
    // fleet daemons above. Populated by a scan on the device (units whose body fails
    // cockpit::daemons::is_fleet_daemon); gitmind manages the fleet set, not these.
    ui.add_space(8.0);
    ui.label(egui::RichText::new("other services · third-party").strong());
    if app.net.other_services.is_empty() {
        ui.label(
            egui::RichText::new("none discovered — run a scan on a device; services without the fleet marker appear here")
                .weak()
                .small(),
        );
    } else {
        for (name, desc) in &app.net.other_services {
            ui.horizontal(|ui| {
                ui.label(format!("○ {name}"));
                if !desc.is_empty() {
                    ui.label(egui::RichText::new(desc).weak().small());
                }
            });
        }
    }

    // fleet health: roll daemon status up across a roster of devices
    ui.add_space(6.0);
    ui.label(egui::RichText::new("fleet health").strong());
    ui.horizontal(|ui| {
        if ui
            .add_enabled(!app.net.host.trim().is_empty(), egui::Button::new("add this device").small())
            .clicked()
        {
            let (n, u, h) = (app.net.host.clone(), app.net.user.clone(), app.net.host.clone());
            if !app.net.health_targets.iter().any(|(_, _, hh)| *hh == h) {
                app.net.health_targets.push((n, u, h));
            }
        }
        if ui
            .add_enabled(idle && !app.net.health_targets.is_empty(), egui::Button::new("refresh").small())
            .clicked()
        {
            let targets = app.net.health_targets.clone();
            app.task.run(move || {
                let mut out = String::new();
                for d in cockpit::flow::fleet_health_remote(&targets) {
                    if d.reachable {
                        let up = d.daemons.iter().filter(|(_, u)| *u).count();
                        out.push_str(&format!("[up] {} — {}/{} up\n", d.name, up, d.daemons.len()));
                    } else {
                        out.push_str(&format!("[!!] {} — unreachable\n", d.name));
                    }
                }
                if out.is_empty() {
                    out.push_str("(no devices in roster)");
                }
                out
            });
        }
    });
    if !app.net.health_targets.is_empty() {
        let roster: Vec<&str> = app.net.health_targets.iter().map(|(n, _, _)| n.as_str()).collect();
        ui.label(egui::RichText::new(format!("roster: {}", roster.join(", "))).weak().small());
    }
    let lines = app.task.log();
    if !lines.is_empty() || !app.net.streaming_name.is_empty() {
        ui.add_space(6.0);
        uikit_egui::scroll::log(ui, &lines, 180.0);
    }

    if let Some(r) = app.task.take_result() {
        app.net.note = r;
        app.net.streaming_name.clear(); // whatever ran is finished
    }
    if app.task.is_running() {
        ui.ctx().request_repaint();
    }
    if !app.net.note.is_empty() {
        ui.add_space(6.0);
        ui.separator();
        ui.label(egui::RichText::new(&app.net.note).monospace().small());
    }
}
