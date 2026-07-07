//! unendlich-dash — native egui telemetry dashboard.
//!
//! Reads the engine's `run_summaries.txt` (and optional `events.jsonl`), shows
//! per-run pass timings, frame-time curves, achievements, and a two-run compare,
//! and copies a Claude-ready digest. Same data model as the HTML dashboard.

mod app;
mod digest;
mod model;
mod parse;
mod plots;
mod theme;
mod widgets;

use app::DashApp;

fn main() -> eframe::Result {
    // Both backends are compiled in. Prefer wgpu where the GPU supports it, and
    // fall back to glow (OpenGL) only if device init fails — e.g. drivers that
    // don't grant wgpu's requested limits (Asahi/Mesa Vulkan caps
    // max_color_attachments below wgpu's default of 8). A 2D dashboard runs fine
    // on either, so the machine picks whatever it can actually bring up.
    // (eframe 0.34: the app creator returns a Result; Box<DashApp> coerces to
    // Box<dyn App> inside Ok(..).)
    let run = |renderer| {
        let options = eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_inner_size([1320.0, 860.0])
                .with_title("unendlich-dash"),
            renderer,
            ..Default::default()
        };
        eframe::run_native(
            "unendlich-dash",
            options,
            Box::new(|cc| {
                theme::install(&cc.egui_ctx);
                Ok(Box::new(DashApp::default()))
            }),
        )
    };

    match run(eframe::Renderer::Wgpu) {
        Err(e) => {
            eprintln!("wgpu backend unavailable ({e}); falling back to glow (OpenGL)…");
            run(eframe::Renderer::Glow)
        }
        ok => ok,
    }
}
