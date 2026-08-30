//! bot_arena_egui.rs — the deployable egui bot-arena app (built behind the `app` feature so a plain
//! `cargo build`/`cargo test` on this crate stays eframe-free). Defaults to the Arena palette (matching
//! bot-arena's real GTK CSS status colours) instead of the shared default.
//!
//! Run:  cargo run --bin bot-arena-egui --features app   (from apps/bot-arena/egui-frontend)

use bot_arena_egui::Dashboard;
use eframe::egui;
use uikit_egui::Adaptive;

struct App {
    dash: Dashboard,
}

impl eframe::App for App {
    fn ui(&mut self, _ui: &mut egui::Ui, _frame: &mut eframe::Frame) {}
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        uikit_egui::apply_palette(ctx, snapclone_theme::palette_for(snapclone_theme::PaletteId::Arena));
        let mut prefs = settings_kit::Prefs::default();
        prefs.palette = "arena".to_string();
        self.dash.apply_settings(ctx, &prefs);
        // Drain the engine + studio workers; keep painting while either is
        // live so a fast Sim run doesn't finish invisibly between events.
        self.dash.tick();
        if self.dash.busy() {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
        egui::CentralPanel::default().show(ctx, |ui| {
            let adaptive = Adaptive::detect(ctx);
            self.dash.ui(ui, &adaptive);
        });
    }
}

/// Must match packaging/org.botarena.BotArena.desktop exactly — a mismatch fails
/// silently, leaving a working app with no icon.
const APP_ID: &str = "org.botarena.BotArena";

fn main() -> eframe::Result<()> {
    eframe::run_native(
        "bot-arena (egui)",
        eframe::NativeOptions {
        // Glow SAID, not left to eframe: its 0.34 defaults include
        // wgpu but not glow, and wgpu's default device descriptor
        // asks for 8 colour attachments — which dies at startup on
        // a phone GPU allowing 5. A glow-only feature list is not
        // enough; gitmind-egui declared one and still got wgpu
        // through feature unification.
        renderer: eframe::Renderer::Glow,
            viewport: uikit_egui::desktop::viewport(APP_ID, "Bot Arena", [900.0, 700.0]),
            ..Default::default()
        },
        Box::new(|_cc| Ok(Box::new(App { dash: Dashboard::new() }))),
    )
}
