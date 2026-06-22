//! scene-view — render ONLY the spatial themed scene, full window, renderer on by default,
//! populated from a `.logscope` template. Headless visual harness for scene iteration.
//!
//!   LOGSCOPE_THEME=Atlantis LOGSCOPE_CONVO=examples/ladder/99-fullspine-marathon.logscope \
//!     ./scene-view
use logscope_egui::scene_preview::ScenePreview;
use logscope_egui::theme;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 820.0])
            .with_title("Logscope · Scene"),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native(
        "logscope-scene",
        options,
        Box::new(|cc| {
            let pal = std::env::var("LOGSCOPE_THEME")
                .ok()
                .and_then(|n| theme::PALETTES.iter().find(|p| p.name.eq_ignore_ascii_case(&n)).copied())
                .unwrap_or(theme::ATLANTIS);
            theme::install_palette(&cc.egui_ctx, pal);
            Ok(Box::new(ScenePreview::new()))
        }),
    )
}
