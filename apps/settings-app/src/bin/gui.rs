//! bsettings — the graphical settings app.
//!
//! Connects to settingsd, edits the shared Prefs with the reusable [`SettingsEditor`] (egui on
//! uikit-egui), and publishes each change live so every app updates immediately. If the daemon
//! isn't reachable it still runs as a plain local editor (offline; edits just won't persist) —
//! the same graceful-degradation rule the rest of the fleet follows.

use bigsilicon_settings::SettingsEditor;
use uikit_egui::Adaptive;

struct BSettings {
    editor: SettingsEditor,
    settings: Option<bigsilicon_client::Settings>,
    status: String,
}

impl BSettings {
    fn new() -> BSettings {
        let settings = bigsilicon_client::Settings::connect();
        let (prefs, status) = match &settings {
            Some(s) => (s.prefs().clone(), "connected to settingsd".to_string()),
            None => (
                settings_kit::Prefs::default(),
                "offline — settingsd not reachable; edits won't persist".to_string(),
            ),
        };
        BSettings { editor: SettingsEditor::new(prefs), settings, status }
    }
}

impl eframe::App for BSettings {
    fn ui(&mut self, _ui: &mut egui::Ui, _frame: &mut eframe::Frame) {}

    #[allow(deprecated)]
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // absorb changes made elsewhere (another app or the CLI) so we don't show stale values
        if let Some(s) = self.settings.as_mut() {
            let _ = s.poll();
        }
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("BigSilicon settings");
            ui.separator();
            let changes = self.editor.ui(ui, &Adaptive::default());
            if !changes.is_empty() {
                match self.settings.as_mut() {
                    Some(s) => {
                        for (k, v) in &changes {
                            let _ = s.set(k, v);
                        }
                        self.editor.mark_published();
                        self.status = format!("published {} change(s) — every app updated live", changes.len());
                    }
                    None => {
                        self.editor.mark_published();
                        self.status = "offline — change applied locally, not persisted".to_string();
                    }
                }
            }
            ui.separator();
            ui.small(&self.status);
        });
    }
}

fn main() -> eframe::Result<()> {
    let opts = eframe::NativeOptions::default();
    eframe::run_native("bsettings", opts, Box::new(|_cc| Ok(Box::new(BSettings::new()))))
}
