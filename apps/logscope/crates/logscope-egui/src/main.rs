//! GUI entry point. All modules live in the library (lib.rs); this binary just launches eframe.
use logscope_egui::app::LogscopeApp;
use logscope_egui::theme;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([720.0, 480.0])
            .with_title("Logscope"),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native(
        "logscope",
        options,
        Box::new(|cc| {
            match std::env::var("LOGSCOPE_THEME").ok()
                .and_then(|name| theme::PALETTES.iter().find(|p| p.name.eq_ignore_ascii_case(&name)).copied())
            {
                Some(p) => theme::install_palette(&cc.egui_ctx, p),
                None => theme::install(&cc.egui_ctx),
            }
            Ok(Box::new(LogscopeApp::default()))
        }),
    )
}
