//! tomescope desktop entry point: an eframe window around `tomescope::app::TomeApp`.
//! The app core lives in the library so the e-ink shell can drive the same
//! code (`src/bin/tomescope_eink.rs`).

use eframe::egui;
use tomescope::app::TomeApp;

/// Must match `packaging/org.tomescope.Tomescope.desktop` exactly.
const APP_ID: &str = "org.tomescope.Tomescope";

fn main() -> eframe::Result<()> {
    // `Exec=tomescope %f` means a double-clicked EPUB arrives here. Taking the
    // FIRST non-flag argument rather than all of them: %f is single-file by
    // definition (%F is the multi-file form), and opening five readers because
    // someone selected five books is not what they asked for.
    let open_path: Option<std::path::PathBuf> = std::env::args_os()
        .skip(1)
        .map(std::path::PathBuf::from)
        .find(|p| !p.to_string_lossy().starts_with('-'));

    let native_options = eframe::NativeOptions {
        // Glow SAID, not left to eframe: its 0.34 defaults include
        // wgpu but not glow, and wgpu's default device descriptor
        // asks for 8 colour attachments — which dies at startup on
        // a phone GPU allowing 5. A glow-only feature list is not
        // enough; gitmind-egui declared one and still got wgpu
        // through feature unification.
        renderer: eframe::Renderer::Glow,
        viewport: egui::ViewportBuilder::default()
            .with_app_id(APP_ID)
            .with_title("Tomescope")
            .with_inner_size([1180.0, 820.0])
            .with_min_inner_size([420.0, 480.0]),
        ..Default::default()
    };

    eframe::run_native(
        APP_ID,
        native_options,
        Box::new(move |cc| {
            let mut app = TomeApp::new(&cc.egui_ctx);
            if let Some(p) = open_path {
                // A path that does not exist, or is not a book, must not take
                // the app down — a file manager can hand us anything, and the
                // library is a perfectly good place to land.
                app.open_from_path(&p);
            }
            Ok(Box::new(Desktop(app)))
        }),
    )
}

struct Desktop(TomeApp);

impl eframe::App for Desktop {
    // The fleet's eframe convention requires ui(); this app drives
    // everything from update(), which is still called.
    fn ui(&mut self, _ui: &mut egui::Ui, _frame: &mut eframe::Frame) {}

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.0.update(ctx);
    }

    fn save(&mut self, _storage: &mut dyn eframe::Storage) {
        self.0.save();
    }
}
