//! panels-view — headless render harness for the FLAT PANELS-MODE chat UI (visual verification
//! only; not shipped). Unlike status-view, this forces spatial_view=false and seeds a doc name so
//! the "Context cards" panel has content, to check the carved-frame texture pass on the sidebar.
//!
//!   LOGSCOPE_THEME=Atlantis ./panels-view

use logscope_egui::analysis::Doc;
use logscope_egui::backend::Inference;
use logscope_egui::model::ModelState;
use logscope_egui::tabs::chat::Chat;
use logscope_egui::theme;

use logscope_egui::diagfeed;
use std::env;

struct App { chat: Chat, model: ModelState, doc: Doc, infer: Inference }

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _f: &mut eframe::Frame) {
        self.chat.ui(ui, &mut self.model, &self.doc, &self.infer, false);
        ui.ctx().request_repaint();
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1300.0, 860.0]).with_title("panels-view").with_transparent(false),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native(
        "panels-view",
        options,
        Box::new(|cc| {
            let pal = std::env::var("LOGSCOPE_THEME").ok()
                .and_then(|n| theme::PALETTES.iter().find(|p| p.name.eq_ignore_ascii_case(&n)).copied())
                .unwrap_or(theme::ATLANTIS);
            theme::install_palette(&cc.egui_ctx, pal);
            let mut chat = Chat::default();
            chat.boot_demo(false); // populates msgs (incl. a code-bearing reply) WITHOUT forcing spatial_view
            chat.debug_force_show_context(true); // force the sidebar open for this screenshot
            chat.debug_force_engine_extras(true); // also show metrics/checkpoints/history cards
            chat.debug_force_checkpoints();
            for _ in 0..5 { diagfeed::record("template mismatch: chat markers not recognized by this tokenizer"); }
            diagfeed::record("load failed: device error initializing backend");
            diagfeed::record("load failed: device error initializing backend");
            diagfeed::record("load failed: device error initializing backend");
            let page = env::var("LOGSCOPE_RIGHT_PAGE").ok().and_then(|v| v.parse::<u8>().ok()).unwrap_or(3);
            chat.debug_force_right_page(page);
            let mut doc = Doc::default();
            doc.name = "sample.log".into();
            doc.lines = vec!["2024-05-07 12:01:15  pool_size=42 active=41".into(), "2024-05-07 12:01:16  pool_size=42 active=42".into()];
            Ok(Box::new(App { chat, model: ModelState::default(), doc, infer: Inference::spawn() }))
        }),
    )
}
