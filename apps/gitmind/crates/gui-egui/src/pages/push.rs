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

    ui.horizontal(|ui| {
        ui.label("user");
        ui.text_edit_singleline(&mut app.push.user);
        ui.label("host");
        ui.text_edit_singleline(&mut app.push.host);
    });
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

    if let Some(r) = app.task.take_result() {
        app.push.note = r;
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
        egui::ScrollArea::vertical()
            .max_height(200.0)
            .stick_to_bottom(true)
            .show(ui, |ui| {
                for l in &log {
                    ui.label(egui::RichText::new(l).monospace().small());
                }
            });
    }
    if !app.push.note.is_empty() {
        ui.add_space(6.0);
        ui.label(egui::RichText::new(&app.push.note).monospace().small());
    }
}
