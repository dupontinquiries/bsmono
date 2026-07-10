//! Binary entry point for early-fps.
//!
//! The actual game shell lives in `app.rs` and only compiles with `--features app`
//! (it pulls in eframe's native windowing/OpenGL stack). Build & play with:
//!
//!     cargo run --release --features app
//!
//! Without that feature the crate is a pure-`egui`, fully headless library that
//! still builds and passes its whole test suite — handy on machines with no display.

#[cfg(feature = "app")]
fn main() -> eframe::Result<()> {
    use eframe::egui;
    use early_fps::app::FpsApp;

    // Seed the world from the wall clock so each launch is a fresh map.
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x1234_5678)
        | 1;

    // Renderer choice: the persisted preference from the Settings page, overridable on the command line with
    // `--gpu` / `--software`. The two backends use different windowing stacks (eframe/glow vs winit/wgpu), so
    // the switch happens here at launch rather than live.
    let db = early_fps::db::Db::load();
    let force_gpu = std::env::args().any(|a| a == "--gpu");
    let force_sw = std::env::args().any(|a| a == "--software");
    let want_gpu = force_gpu || (db.gpu_renderer && !force_sw);
    #[cfg(feature = "gpu")]
    if want_gpu {
        early_fps::gpu::run();
        return Ok(());
    }
    #[cfg(not(feature = "gpu"))]
    if want_gpu {
        eprintln!("GPU renderer is selected but this build lacks the `gpu` feature — rebuild with `--features app,gpu`. Falling back to the software renderer.");
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([800.0, 500.0])
            .with_title("EARLY-FPS"),
        renderer: eframe::Renderer::Glow,
        vsync: true,
        ..Default::default()
    };

    eframe::run_native(
        "EARLY-FPS",
        options,
        Box::new(move |cc| {
            early_fps::hud::theme::apply_visuals(&cc.egui_ctx);
            Ok(Box::new(FpsApp::new(seed)))
        }),
    )
}

#[cfg(not(feature = "app"))]
fn main() {
    println!(
        "early-fps was built without the windowing shell.\n\
         \n\
         To play, build with the `app` feature on a machine with a display:\n\
         \n    cargo run --release --features app\n\
         \n\
         The core library (math, software renderer, level gen, AI, combat) is\n\
         compiled and can be exercised headlessly with:\n\
         \n    cargo test\n"
    );
}
