//! mic-pearl-view — headless render harness for the physical UI panel + mic pearl (visual verification
//! only; not shipped as a user-facing tool). Constructs a bare `Chat` and calls the real `scene_ui`, so
//! this renders the ACTUAL production panel/mic-pearl code, not a reimplementation.
//!
//!   LOGSCOPE_THEME=Atlantis LOGSCOPE_MIC_PEARL_DEBUG=active ./mic-pearl-view

use logscope_egui::tabs::chat::Chat;
use logscope_egui::theme;

struct App { chat: Chat }

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _f: &mut eframe::Frame) {
        self.chat.scene_ui(ui);
        ui.ctx().request_repaint();
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1100.0, 820.0]).with_title("mic-pearl-view").with_transparent(false),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native(
        "mic-pearl-view",
        options,
        Box::new(|cc| {
            let pal = std::env::var("LOGSCOPE_THEME").ok()
                .and_then(|n| theme::PALETTES.iter().find(|p| p.name.eq_ignore_ascii_case(&n)).copied())
                .unwrap_or(theme::ATLANTIS);
            theme::install_palette(&cc.egui_ctx, pal);
            let mut chat = Chat::default();
            if let Ok(text) = std::fs::read_to_string(
                std::env::var("LOGSCOPE_CONVO").unwrap_or_else(|_| "examples/ladder/AA-mega-spine.logscope".into())
            ) { chat.load_logscope_script(&text); }
            Ok(Box::new(App { chat }))
        }),
    )
}
