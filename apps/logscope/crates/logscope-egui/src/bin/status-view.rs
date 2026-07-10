//! status-view — headless render harness for the chat tab's status bar (visual verification only; not a
//! shipped tool). It drives the REAL `Chat::ui`, so the persona badge + live activity HUD it screenshots
//! are the actual production widgets, not a reimplementation.
//!
//!   LOGSCOPE_THEME=Atlantis ./status-view                       # idle: persona badge only
//!   LOGSCOPE_FAN_DEBUG="generating · 12 queued · 4 rounds left" ./status-view   # forces the activity HUD
//!
//! (LOGSCOPE_FAN_DEBUG is honored by Chat::activity_status, mirroring LOGSCOPE_MIC_PEARL_DEBUG.)

use logscope_egui::analysis::Doc;
use logscope_egui::backend::Inference;
use logscope_egui::model::ModelState;
use logscope_egui::tabs::chat::Chat;
use logscope_egui::theme;

struct App { chat: Chat, model: ModelState, doc: Doc, infer: Inference }

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _f: &mut eframe::Frame) {
        self.chat.ui(ui, &mut self.model, &self.doc, &self.infer, false);
        ui.ctx().request_repaint();
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1280.0, 820.0]).with_title("status-view").with_transparent(false),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native(
        "status-view",
        options,
        Box::new(|cc| {
            let pal = std::env::var("LOGSCOPE_THEME").ok()
                .and_then(|n| theme::PALETTES.iter().find(|p| p.name.eq_ignore_ascii_case(&n)).copied())
                .unwrap_or(theme::ATLANTIS);
            theme::install_palette(&cc.egui_ctx, pal);
            let mut chat = Chat::default();
            if let Ok(text) = std::fs::read_to_string(
                std::env::var("LOGSCOPE_CONVO").unwrap_or_else(|_| "examples/02-lru-fill.logscope".into())
            ) { chat.load_logscope_script(&text); }
            Ok(Box::new(App { chat, model: ModelState::default(), doc: Doc::default(), infer: Inference::spawn() }))
        }),
    )
}
