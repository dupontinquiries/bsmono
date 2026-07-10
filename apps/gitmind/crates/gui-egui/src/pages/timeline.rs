//! Timeline page — recent checkpoints of this repo (gitmind's CheckpointEngine over the git
//! history). Read-only viewer. Local git, so it loads synchronously (fast) on first show / refresh.
use crate::app::App;

#[derive(Default)]
pub struct State {
    pub loaded: bool,
    pub checkpoints: Vec<(String, i64, String)>, // (oid, when, label)
    pub err: String,
}

fn load(state: &mut State) {
    state.loaded = true;
    state.err.clear();
    match gitmind_core::backend::GitCli::discover(cockpit::ops::repo_root()) {
        Some(git) => {
            let eng = gitmind_core::checkpoint::CheckpointEngine::new(&git, "main");
            match eng.list(40) {
                Ok(cps) => {
                    state.checkpoints = cps.into_iter().map(|c| (c.oid, c.when, c.label)).collect();
                }
                Err(e) => state.err = e,
            }
        }
        None => state.err = "not inside a git repo".to_string(),
    }
}

pub fn ui(app: &mut App, ui: &mut egui::Ui) {
    ui.heading("Timeline");
    ui.label(egui::RichText::new("recent checkpoints of this repo").weak().small());
    ui.separator();

    if ui.button("↻ refresh").clicked() || !app.timeline.loaded {
        load(&mut app.timeline);
    }
    if !app.timeline.err.is_empty() {
        ui.colored_label(egui::Color32::from_rgb(0xf7, 0x76, 0x8e), &app.timeline.err);
    }

    egui::ScrollArea::vertical().id_salt("timeline").show(ui, |ui| {
        for (oid, _when, label) in &app.timeline.checkpoints {
            ui.horizontal(|ui| {
                let short = &oid[..oid.len().min(8)];
                ui.label(egui::RichText::new(short).monospace().weak());
                ui.label(label);
            });
        }
    });
}
