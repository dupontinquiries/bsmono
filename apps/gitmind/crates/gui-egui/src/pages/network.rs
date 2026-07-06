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
    ui.label(egui::RichText::new("daemons — the fleet's background services").strong());
    for d in cockpit::daemons::all() {
        ui.horizontal(|ui| {
            ui.label(format!("● {}", d.name));
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

    // live log panel (streaming daemon logs or captured output)
    let lines = app.task.log();
    if !lines.is_empty() || !app.net.streaming_name.is_empty() {
        ui.add_space(6.0);
        egui::ScrollArea::vertical()
            .max_height(180.0)
            .stick_to_bottom(true)
            .show(ui, |ui| {
                for l in &lines {
                    ui.label(egui::RichText::new(l).monospace().small());
                }
            });
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
