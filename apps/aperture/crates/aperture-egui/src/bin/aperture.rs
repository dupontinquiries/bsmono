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

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // Take our commands off the keyboard. (A crash cannot do this; the
        // keyboard notices the pid is gone within a second.)
        self.state.sync_keyboard_strip(false);
    }
}

fn main() -> eframe::Result<()> {
    // Why the window will not open, said before the panic in a dependency does.
    // `uikit_egui::backend_check`: the usual cause is a static musl build, which cannot
    // dlopen libwayland-client, and wayland-sys's own message points at a package that is
    // already installed. Silent when the setup is fine.
    if let Some(why) = uikit_egui::backend_check::preflight() {
        eprintln!("{why}");
    }
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
        Box::new(move |_cc| {
            let mut state = AppState::new(path, true);
            state.strip_enabled = true;
            Ok(Box::new(App { state }))
        }),
    )
}
