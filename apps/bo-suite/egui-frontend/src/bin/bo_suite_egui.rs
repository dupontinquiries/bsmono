//! bo_suite_egui.rs — the deployable egui bo-suite app (built behind the `app` feature so a plain
//! `cargo build`/`cargo test` on this crate stays eframe-free). Defaults to the Amber palette (warm
//! gold) matching bo-controller's Qt/QML surface, instead of the shared default.
//!
//! Run:  cargo run --bin bo-suite-egui --features app   (from apps/bo-suite/egui-frontend)

use eframe::egui;
use bo_suite_egui::{Controller, Device, Scene};
use uikit_egui::Adaptive;

struct App {
    controller: Controller,
}

impl App {
    fn new() -> Self {
        let devices = vec![
            Device { name: "Living Room".into(), volume: 62, playing: true },
            Device { name: "Kitchen".into(), volume: 40, playing: false },
            Device { name: "Studio".into(), volume: 78, playing: true },
        ];
        let scenes = vec![
            Scene { name: "Morning".into() },
            Scene { name: "Focus".into() },
            Scene { name: "Evening Wind-down".into() },
        ];
        Self { controller: Controller::new(devices, scenes) }
    }
}

impl eframe::App for App {
    fn ui(&mut self, _ui: &mut egui::Ui, _frame: &mut eframe::Frame) {}
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        uikit_egui::apply_palette(ctx, snapclone_theme::palette_for(snapclone_theme::PaletteId::Amber));
        let mut prefs = settings_kit::Prefs::default();
        prefs.palette = "amber".to_string();
        self.controller.apply_settings(ctx, &prefs);
        egui::CentralPanel::default().show(ctx, |ui| {
            let adaptive = Adaptive::detect(ctx);
            self.controller.ui(ui, &adaptive);
        });
    }
}

/// Must match packaging/org.bosuite.BoSuite.desktop exactly. A mismatch fails
/// SILENTLY: the app runs and has no icon, no dock grouping and the wrong
/// name in the switcher.
const APP_ID: &str = "org.bosuite.BoSuite";

fn main() -> eframe::Result<()> {
    eframe::run_native(
        "bo-suite (egui)",
        eframe::NativeOptions {
        viewport: uikit_egui::desktop::viewport(APP_ID, "Bo Suite", [900.0, 700.0]),
        ..Default::default()
    },
        Box::new(|_cc| Ok(Box::new(App::new()))),
    )
}
