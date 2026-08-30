//! aperture — boot. `aperture [FILE]`.
//!
//! Env:
//!   APERTURE_OLLAMA  host:port of an ollama server (default 127.0.0.1:11434)
//!   APERTURE_MODEL   ollama model tag (default qwen2.5-coder:1.5b-base)
//!   APERTURE_CMD     use a subprocess provider instead: argv, whitespace-split
//!
//! Everything above is also editable live in the loop pane (F2).

use aperture_egui::{state::AppState, view};

/// Must match the packaged .desktop id exactly (same silent-failure trap the
/// other egui apps document: wrong id = no icon, no dock grouping).
const APP_ID: &str = "org.aperture.Aperture";

struct App {
    state: AppState,
}

impl eframe::App for App {
    // eframe 0.34: `ui(&mut Ui)` is the required driver; `update(ctx)` still
    // exists but is deprecated back-compat (the runner calls update, then ui).
    // Panels attach through the ctx, so the root `ui` itself goes unused.
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        view::frame(ui, &mut self.state);
    }
}

fn main() -> eframe::Result<()> {
    let path = std::env::args().nth(1).map(std::path::PathBuf::from);
    eframe::run_native(
        "aperture",
        eframe::NativeOptions {
        // Glow SAID, not left to eframe: its 0.34 defaults include
        // wgpu but not glow, and wgpu's default device descriptor
        // asks for 8 colour attachments — which dies at startup on
        // a phone GPU allowing 5. A glow-only feature list is not
        // enough; gitmind-egui declared one and still got wgpu
        // through feature unification.
        renderer: eframe::Renderer::Glow,
            viewport: uikit_egui::desktop::viewport(APP_ID, "aperture", [1100.0, 760.0]),
            ..Default::default()
        },
        Box::new(move |_cc| Ok(Box::new(App { state: AppState::new(path, true) }))),
    )
}
