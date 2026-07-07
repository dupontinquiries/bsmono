//! Hello-Vulkan-Triangle.
//!
//! Opens a GLFW window with a Vulkan surface, sets up the smallest possible
//! graphics pipeline, and renders a colored triangle. Validates that the
//! whole stack works on your machine before we start adding chunk rendering.
//!
//! Run:
//!     cargo run --release --features render --bin unendlich-triangle

use glfw::{Action, Key, WindowEvent, WindowHint, ClientApiHint};
use unendlich_engine::render::{TriangleRenderer, VulkanContext};
use std::sync::mpsc::Receiver;

const INITIAL_WIDTH:  u32 = 1024;
const INITIAL_HEIGHT: u32 = 768;
const TITLE: &str = "Meinkraft — Hello Vulkan Triangle";

fn main() {
    env_logger::Builder::from_default_env()
        .filter_level(log::LevelFilter::Info)
        .init();

    log::info!("Starting {}", TITLE);

    let mut glfw = glfw::init(glfw::FAIL_ON_ERRORS).expect("Failed to init GLFW");
    if !glfw.vulkan_supported() {
        eprintln!("ERROR: GLFW reports no Vulkan support on this system.");
        eprintln!("Make sure libvulkan and a working ICD (e.g. mesa-vulkan-drivers) are installed.");
        std::process::exit(1);
    }

    // No GL context — we're using Vulkan.
    glfw.window_hint(WindowHint::ClientApi(ClientApiHint::NoApi));
    glfw.window_hint(WindowHint::Resizable(true));

    let (mut window, events) = glfw.create_window(
        INITIAL_WIDTH, INITIAL_HEIGHT, TITLE, glfw::WindowMode::Windowed,
    ).expect("Failed to create GLFW window");

    window.set_key_polling(true);
    window.set_framebuffer_size_polling(true);

    let ctx = match VulkanContext::new(&glfw, &window) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to initialize Vulkan: {}", e);
            std::process::exit(1);
        }
    };

    let (fb_w, fb_h) = window.get_framebuffer_size();
    let mut renderer = match TriangleRenderer::new(&ctx, (fb_w as u32, fb_h as u32)) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Failed to initialize renderer: {}", e);
            std::process::exit(1);
        }
    };

    log::info!("Renderer ready ({}x{}). Press Esc to quit.", fb_w, fb_h);

    let mut frame_count = 0u64;
    let mut last_log = std::time::Instant::now();

    while !window.should_close() {
        glfw.poll_events();
        let mut needs_resize: Option<(u32, u32)> = None;
        for (_, event) in glfw::flush_messages(&events) {
            handle_event(&mut window, event, &mut needs_resize);
        }

        // If a resize event came in, recreate before drawing.
        if let Some(size) = needs_resize {
            // GLFW reports (0, 0) when the window is minimized. Skip drawing
            // until we get a real size back.
            if size.0 == 0 || size.1 == 0 {
                std::thread::sleep(std::time::Duration::from_millis(16));
                continue;
            }
            if let Err(e) = renderer.recreate_swapchain(&ctx, size) {
                eprintln!("Recreate swapchain failed: {}", e);
                break;
            }
        }

        match renderer.draw_frame(&ctx) {
            Ok(true) => { frame_count += 1; }
            Ok(false) => {
                // Out-of-date or suboptimal — recreate at the current size.
                let (w, h) = window.get_framebuffer_size();
                if w > 0 && h > 0 {
                    if let Err(e) = renderer.recreate_swapchain(&ctx, (w as u32, h as u32)) {
                        eprintln!("Recreate swapchain failed: {}", e);
                        break;
                    }
                }
            }
            Err(e) => {
                eprintln!("Draw error: {}", e);
                break;
            }
        }

        if last_log.elapsed().as_secs() >= 1 {
            log::info!("FPS: {}", frame_count);
            frame_count = 0;
            last_log = std::time::Instant::now();
        }
    }

    log::info!("Shutting down");
    renderer.destroy(&ctx);
    drop(ctx);
}

fn handle_event(
    window: &mut glfw::Window,
    event: WindowEvent,
    needs_resize: &mut Option<(u32, u32)>,
) {
    match event {
        WindowEvent::Key(Key::Escape, _, Action::Press, _) => {
            window.set_should_close(true);
        }
        WindowEvent::FramebufferSize(w, h) => {
            *needs_resize = Some((w as u32, h as u32));
        }
        _ => {}
    }
}

// Suppress unused warnings on platforms where Receiver isn't used directly.
#[allow(dead_code)]
fn _r(_r: &Receiver<()>) {}
