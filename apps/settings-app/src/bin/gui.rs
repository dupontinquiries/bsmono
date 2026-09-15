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
        // FONTS ONLY, not `apply_palette`. This app themes itself with egui's
        // defaults and sets no Visuals; calling `apply_palette` would install
        // the fonts AND recolour the whole window, which is a bigger change
        // than "stop drawing boxes" and wants an eye on it. `fonts::install` is
        // idempotent behind a context flag, so per-frame is free.
        //
        // See maintenance/tools/font_install_check.py.
        uikit_egui::fonts::install(ctx);
        // absorb changes made elsewhere (another app or the CLI) so we don't show stale values
        if let Some(s) = self.settings.as_mut() {
            let _ = s.poll();
        }
        egui::CentralPanel::default().show(ctx, |ui| {
            // Was a bare CentralPanel with no scroll wrapper at all -- on
            // any window shorter than the settings content (real report:
            // overflows the bottom with no scroll takeover), everything
            // past the visible edge was simply unreachable, not just
            // hidden. `editor.ui()` renders a genuinely long sequence of
            // sliders/toggles with nothing bounding its height, so it
            // needed a real ScrollArea, not a taller default window.
            //
            // Also removed this function's own `ui.heading("BigSilicon
            // settings")` + separator -- `editor.ui()` already renders
            // its own copy of both right at its start (SettingsEditor::ui,
            // "BigSilicon Settings"), so the two were stacking as a
            // visible duplicate heading. Found while reading this exact
            // code to place the ScrollArea correctly, not a separate pass.
            egui::ScrollArea::vertical().show(ui, |ui| {
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
            });
            // Left OUTSIDE the ScrollArea deliberately: a status line
            // ("published 3 change(s)...", "offline...") is exactly the
            // kind of thing that should stay visible rather than scroll
            // away with the content that caused it.
            ui.separator();
            ui.small(&self.status);
        });
    }
}

fn main() -> eframe::Result<()> {
    let opts = eframe::NativeOptions {
        // Glow SAID, not left to eframe.
        //
        // eframe 0.34's defaults include wgpu but not glow, so
        // Renderer::default() picks wgpu — whose default device
        // descriptor asks for 8 colour attachments and dies on a phone
        // GPU allowing 5. A glow-only feature list is not enough:
        // gitmind-egui declared exactly that and still got wgpu through
        // feature unification with something else in the build.
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native("bsettings", opts, Box::new(|_cc| Ok(Box::new(BSettings::new()))))
}
