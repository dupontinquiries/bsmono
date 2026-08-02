//! The central settings app, which had no way to be started.
//!
//! `settings-egui` described itself as "the central BigSilicon settings app" and
//! shipped a `[lib]` only — `SettingsApp` was ready to run and nothing could run
//! it. A desktop entry would have pointed at a binary that does not exist, which
//! is a worse failure than no entry: the launcher shows the app and clicking it
//! does nothing.

/// Must match `packaging/org.bigsilicon.Settings.desktop` exactly.
const APP_ID: &str = "org.bigsilicon.Settings";

const PREFS_APP: &str = "bigsilicon";
const PREFS_FILE: &str = "prefs.kv";

fn load_prefs() -> settings_kit::Prefs {
    let Some(text) = config_kit::load(PREFS_APP, PREFS_FILE) else {
        return settings_kit::Prefs::default();
    };
    let kv: Vec<(String, String)> = text.lines()
        .filter_map(|l| l.split_once('=').map(|(k, v)| (k.trim().to_string(), v.trim().to_string())))
        .collect();
    settings_kit::Prefs::from_kv(&kv)
}

fn save_prefs(p: &settings_kit::Prefs) -> std::io::Result<()> {
    let text: String = p.to_kv().into_iter().map(|(k, v)| format!("{k}={v}\n")).collect();
    config_kit::save(PREFS_APP, PREFS_FILE, &text)
}

struct App {
    prefs: settings_kit::Prefs,
    /// Changes since the last write, as the key/values the sharemd bus wants.
    /// Kept rather than published immediately because a slider emits a change
    /// per frame while dragging, and a bus message per frame is a flood.
    pending: Vec<(String, String)>,
    error: Option<String>,
}

impl Default for App {
    fn default() -> Self {
        App { prefs: load_prefs(), pending: Vec::new(), error: None }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _f: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ui.ctx(), |ui| {
            let changed = settings_egui::settings_editor_ui(ui, &mut self.prefs);
            for (k, v) in changed {
                // Replace by key rather than append: dragging a slider would
                // otherwise queue a hundred edits of the same setting, and the
                // bus would replay every intermediate value.
                self.pending.retain(|(pk, _)| pk != &k);
                self.pending.push((k, v));
            }
            ui.separator();
            ui.horizontal(|ui| {
                let dirty = !self.pending.is_empty();
                if ui.add_enabled(dirty, egui::Button::new("Apply")).clicked() {
                    match save_prefs(&self.prefs) {
                        Ok(()) => { self.pending.clear(); self.error = None; }
                        // A read-only config dir is real on locked-down devices,
                        // and an Apply that silently fails is worse than one that
                        // refuses.
                        Err(e) => self.error = Some(format!("could not save: {e}")),
                    }
                }
                if dirty {
                    ui.label(format!("{} unsaved", self.pending.len()));
                }
            });
            if let Some(e) = &self.error {
                ui.colored_label(egui::Color32::from_rgb(230, 160, 60), e);
            }
        });
    }
}

fn main() -> eframe::Result<()> {
    let opts = eframe::NativeOptions {
        viewport: uikit_egui::desktop::viewport(APP_ID, "Settings", [720.0, 640.0]),
        ..Default::default()
    };
    eframe::run_native(APP_ID, opts, Box::new(|_cc| Ok(Box::<App>::default())))
}
