//! shell-view — headless render harness for the FULL app shell (top header + bottom tab switcher +
//! active tab body), at a chosen viewport size. Verification only, not shipped. Unlike panels-view
//! (which drives Chat::ui directly), this drives the real LogscopeApp so the tab switcher itself —
//! the thing narrow-width layout bugs live in — is the actual production widget.
//!
//!   LOGSCOPE_W=390 LOGSCOPE_H=800 ./shell-view   # a phone-ish portrait viewport

use logscope_egui::app::LogscopeApp;

fn main() -> eframe::Result {
    let w: f32 = std::env::var("LOGSCOPE_W").ok().and_then(|v| v.parse().ok()).unwrap_or(390.0);
    let h: f32 = std::env::var("LOGSCOPE_H").ok().and_then(|v| v.parse().ok()).unwrap_or(800.0);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([w, h]).with_title("shell-view").with_transparent(false),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native(
        "shell-view",
        options,
        Box::new(|_cc| {
            let mut app = LogscopeApp::default();
            app.boot_env(); // LOGSCOPE_BOOT=1 LOGSCOPE_TAB=chat, etc.
            Ok(Box::new(app))
        }),
    )
}
