//! scene_preview.rs — a stripped fork of the app that renders ONLY the spatial GPU scene, full window,
//! renderer on by default, populated from a `.logscope` conversation template. No tabs, header, or
//! composer. Built for headless visual iteration on the themed environments (e.g. Atlantis): boot it
//! under llvmpipe, screenshot, tune the scene, repeat.
//!
//! Population order: `LOGSCOPE_CONVO=<path to .logscope>` if set and readable, else a shipped default
//! (`examples/ladder/99-fullspine-marathon.logscope` relative to CWD), else the built-in demo tree.
//! Theme is chosen by the binary (env `LOGSCOPE_THEME`, default Atlantis) before this is constructed.

use crate::tabs::chat::Chat;

pub struct ScenePreview {
    chat: Chat,
}

impl Default for ScenePreview {
    fn default() -> Self {
        let mut chat = Chat::default();
        let script = std::env::var("LOGSCOPE_CONVO")
            .ok()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .or_else(|| std::fs::read_to_string("examples/ladder/AA-mega-spine.logscope").ok())
            .or_else(|| std::fs::read_to_string("crates/logscope-egui/../../examples/ladder/AA-mega-spine.logscope").ok());
        match script {
            Some(t) => chat.load_logscope_script(&t),
            None => chat.boot_demo(true),
        }
        // Optional camera override for multi-angle capture: "yaw,pitch,dist" (radians, radians, world units).
        if let Ok(spec) = std::env::var("POS_ANGLE_INIT").or_else(|_| std::env::var("LOGSCOPE_CAM")) {
            let n: Vec<f32> = spec.split(',').filter_map(|s| s.trim().parse().ok()).collect();
            if n.len() == 3 {
                chat.set_cam(n[0], n[1], n[2]);
            }
        }
        Self { chat }
    }
}

impl ScenePreview {
    pub fn new() -> Self {
        Self::default()
    }
}

impl eframe::App for ScenePreview {
    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        let c = visuals.window_fill();
        [c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0, 1.0] // opaque — no Wayland ghosting
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default()
            .frame(egui::Frame::default().inner_margin(0.0))
            .show_inside(ui, |ui| {
                self.chat.scene_ui(ui);
            });
        // SCENE-SELECT dropdown (top-left overlay) — switch theme live; the scene rebuilds from theme::active().
        egui::Area::new(egui::Id::new("scene-select"))
            .fixed_pos(egui::pos2(10.0, 46.0))
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    let cur = crate::theme::active().name;
                    egui::ComboBox::from_id_salt("scene-combo")
                        .selected_text(cur)
                        .show_ui(ui, |ui| {
                            for p in crate::theme::PALETTES.iter() {
                                if ui.selectable_label(p.name == cur, p.name).clicked() {
                                    crate::theme::install_palette(ui.ctx(), *p);
                                }
                            }
                        });
                });
            });
        // The scene animates (caustics/ripples/breathing), so keep repainting.
        ui.ctx().request_repaint();
    }
}
