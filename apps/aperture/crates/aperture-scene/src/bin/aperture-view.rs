//! aperture-view — boot. `aperture-view [FILE]` (defaults to its own source,
//! so the first run always has something to read).

use aperture_scene::{app, viewer::Viewer};

const APP_ID: &str = "org.aperture.ApertureView"; // must match a packaged .desktop id

struct A {
    app: app::App,
}

impl eframe::App for A {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        app::frame(ui, &mut self.app);
    }
}

fn main() -> eframe::Result<()> {
    let path = std::env::args().nth(1).map(std::path::PathBuf::from);
    let viewer = match path {
        Some(p) => Viewer::open(p.clone()).unwrap_or_else(|e| {
            eprintln!("aperture-view: {}: {e}", p.display());
            std::process::exit(1)
        }),
        None => Viewer::from_text(
            include_str!("../viewer.rs"),
            aperture_core::syntax::Language::Rust,
            None,
        ),
    };
    eframe::run_native(
        "aperture-view",
        eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_app_id(APP_ID)
                .with_title("aperture-view")
                .with_inner_size([1280.0, 800.0]),
            ..Default::default()
        },
        Box::new(move |_cc| Ok(Box::new(A { app: app::App::new(viewer) }))),
    )
}
