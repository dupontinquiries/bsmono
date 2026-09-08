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
    
    // ASK THE ADAPTER WHAT IT HAS, rather than taking wgpu's defaults.
    //
    // `eframe::wgpu::DeviceDescriptor::default()` requests `Limits::default()`, which wants
    // `max_color_attachments: 8`. Asahi's honeykrisp allows 5, so device creation fails
    // outright and the app never opens a window:
    //
    //   WGPU error: Limit 'max_color_attachments' value 8 is better than allowed 5
    //
    // egui itself needs one colour attachment. The eight was never a requirement, only a
    // default carried in from a descriptor nobody filled in — so the fix is to request the
    // adapter's OWN limits, which are by definition satisfiable, instead of clamping the one
    // limit that happened to be reported first. Clamping just that field would leave the next
    // over-large default to fail the same way on the next device.
    //
    // Through `eframe::wgpu` and `eframe::egui_wgpu` rather than direct dependencies: a
    // separately-versioned wgpu compiles and then fails to match the types eframe actually
    // links, which is a worse error than the one being fixed.
    let mut wgpu_options = eframe::egui_wgpu::WgpuConfiguration::default();
    if let eframe::egui_wgpu::WgpuSetup::CreateNew(ref mut create) = wgpu_options.wgpu_setup {
        create.device_descriptor = std::sync::Arc::new(|adapter: &eframe::wgpu::Adapter| {
            eframe::wgpu::DeviceDescriptor {
                label: Some("av1tuner"),
                required_features: eframe::wgpu::Features::empty(),
                required_limits: adapter.limits(),
                memory_hints: Default::default(),
                experimental_features: Default::default(),
                trace: Default::default(),
            }
        });
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([900.0, 640.0])
            .with_min_inner_size([700.0, 500.0])
            .with_title("av1tuner"),
        renderer,
        wgpu_options,
        ..Default::default()
    };
    
    eframe::run_native(
        "av1tuner",
        options,
        Box::new(|cc| Ok(Box::new(app::App::new(cc)))),
    )
}
