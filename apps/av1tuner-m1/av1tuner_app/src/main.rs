//! # av1tuner — AV1 batch tuner GUI
//!
//! M1: single video, per-segment isolation, resume, basic UI.
//!
//! Renderer choice: defaults to wgpu (Vulkan/Metal/DX12 via wgpu). Set 
//! `AV1TUNER_RENDERER=glow` to fall back to OpenGL — useful if wgpu 
//! struggles on a particular driver. Asahi + honeykrisp Vulkan: wgpu 
//! is the right pick.

use eframe::egui;

mod app;

fn main() -> Result<(), eframe::Error> {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info")
    ).init();
    
    let renderer = match std::env::var("AV1TUNER_RENDERER").as_deref() {
        Ok("glow") => {
            log::info!("renderer: glow (OpenGL) — set by AV1TUNER_RENDERER");
            eframe::Renderer::Glow
        }
        Ok("wgpu") | Err(_) => {
            log::info!("renderer: wgpu (default)");
            eframe::Renderer::Wgpu
        }
        Ok(other) => {
            log::warn!("AV1TUNER_RENDERER={} not recognized; using wgpu", other);
            eframe::Renderer::Wgpu
        }
    };
    
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([900.0, 640.0])
            .with_min_inner_size([700.0, 500.0])
            .with_title("av1tuner"),
        renderer,
        ..Default::default()
    };
    
    eframe::run_native(
        "av1tuner",
        options,
        Box::new(|cc| Ok(Box::new(app::App::new(cc)))),
    )
}
