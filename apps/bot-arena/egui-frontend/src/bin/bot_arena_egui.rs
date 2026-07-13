//! bot_arena_egui.rs — the deployable egui bot-arena app (built behind the `app` feature so a plain
//! `cargo build`/`cargo test` on this crate stays eframe-free). Defaults to the Arena palette (matching
//! bot-arena's real GTK CSS status colours) instead of the shared default.
//!
//! Run:  cargo run --bin bot-arena-egui --features app   (from apps/bot-arena/egui-frontend)

use eframe::egui;
use bot_arena_egui::{Bot, Dashboard, Replay};
use uikit_egui::Adaptive;

struct App {
    dash: Dashboard,
}

impl App {
    fn new() -> Self {
        let bots = vec![
            Bot { name: "Vex-7".into(), score: 42, status: "running".into() },
            Bot { name: "Corvid".into(), score: 38, status: "passed".into() },
            Bot { name: "Null Pointer".into(), score: 51, status: "running".into() },
            Bot { name: "Grimalkin".into(), score: 12, status: "failed".into() },
        ];
        let replays = vec![
            Replay { title: "Vex-7 vs Corvid".into(), turns: 84 },
            Replay { title: "Null Pointer solo run".into(), turns: 130 },
        ];
        Self { dash: Dashboard::new(bots, replays) }
    }
}

impl eframe::App for App {
    fn ui(&mut self, _ui: &mut egui::Ui, _frame: &mut eframe::Frame) {}
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        uikit_egui::apply_palette(ctx, snapclone_theme::palette_for(snapclone_theme::PaletteId::Arena));
        let mut prefs = settings_kit::Prefs::default();
        prefs.palette = "arena".to_string();
        self.dash.apply_settings(ctx, &prefs);
        egui::CentralPanel::default().show(ctx, |ui| {
            let adaptive = Adaptive::detect(ctx);
            self.dash.ui(ui, &adaptive);
        });
    }
}

fn main() -> eframe::Result<()> {
    eframe::run_native(
        "bot-arena (egui)",
        eframe::NativeOptions::default(),
        Box::new(|_cc| Ok(Box::new(App::new()))),
    )
}
