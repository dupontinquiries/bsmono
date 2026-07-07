//! Hello-Chunk: render one CPU-generated chunk in a window.
//!
//! Pulls a single chunk from the world generator at the origin, meshes it
//! with naive face culling, uploads vertex/index buffers, and renders with
//! depth + an orbiting camera.
//!
//! Run:
//!     cargo run --release --features render --bin unendlich-chunk

use glfw::{Action, Key, WindowEvent, WindowHint, ClientApiHint};
use unendlich_engine::constants::{CHUNK_SIZE, SEA_LEVEL, WORLD_SEED_DEFAULT};
use unendlich_engine::render::{Camera, ChunkRenderer, VulkanContext};
use unendlich_engine::world::chunk::ChunkCoord;
use unendlich_engine::world::cpu_generator::WorldGenerator;
use nalgebra_glm as glm;
use std::time::Instant;

const INITIAL_WIDTH: u32 = 1280;
const INITIAL_HEIGHT: u32 = 720;
const TITLE: &str = "Meinkraft — Hello Chunk";

fn main() {
    env_logger::Builder::from_default_env()
        .filter_level(log::LevelFilter::Info)
        .init();

    log::info!("Starting {}", TITLE);

    // 1. Generate a 32^3 chunk straddling sea level so we have interesting
    //    geometry (surface band passing through it).
    let gen = WorldGenerator::new(WORLD_SEED_DEFAULT as u32);
    // Sea level is at y=64, chunk size 32 -> chunk y=2 covers world y in [64,96).
    // Pick chunk y=2 so we see the surface in the upper half.
    let coord = ChunkCoord::new(0, SEA_LEVEL / CHUNK_SIZE, 0);
    let chunk = gen.generate_chunk(coord);
    let solid = chunk.blocks.iter().filter(|b| b.is_solid()).count();
    log::info!(
        "Generated chunk {:?}: {} / {} solid blocks ({:.1}%)",
        coord, solid, chunk.blocks.len(),
        100.0 * solid as f32 / chunk.blocks.len() as f32,
    );

    // 2. Open window + Vulkan.
    let mut glfw = glfw::init(glfw::FAIL_ON_ERRORS).expect("init GLFW");
    if !glfw.vulkan_supported() {
        eprintln!("ERROR: GLFW reports no Vulkan support.");
        std::process::exit(1);
    }
    glfw.window_hint(WindowHint::ClientApi(ClientApiHint::NoApi));
    glfw.window_hint(WindowHint::Resizable(true));

    let (mut window, events) = glfw.create_window(
        INITIAL_WIDTH, INITIAL_HEIGHT, TITLE, glfw::WindowMode::Windowed,
    ).expect("create window");
    window.set_key_polling(true);
    window.set_framebuffer_size_polling(true);

    let ctx = VulkanContext::new(&glfw, &window).expect("vulkan context");

    let (fb_w, fb_h) = window.get_framebuffer_size();

    // 3. Build the chunk renderer with a camera orbiting the chunk's center.
    let (ox, oy, oz) = coord.block_origin();
    let half = CHUNK_SIZE as f32 * 0.5;
    let target = glm::vec3(
        ox as f32 + half,
        oy as f32 + half,
        oz as f32 + half,
    );
    let camera = Camera::new(target, 60.0);

    let mut renderer = ChunkRenderer::new(&ctx, (fb_w as u32, fb_h as u32), camera)
        .expect("chunk renderer");
    renderer.upload_chunk(&ctx, &chunk).expect("upload");

    log::info!("Renderer ready ({}x{}). Camera target {:?}. Press Esc to quit.", fb_w, fb_h, target);

    let mut frame_count = 0u64;
    let mut last_log = Instant::now();
    let start = Instant::now();

    while !window.should_close() {
        glfw.poll_events();
        let mut needs_resize: Option<(u32, u32)> = None;
        for (_, event) in glfw::flush_messages(&events) {
            handle_event(&mut window, event, &mut needs_resize);
        }

        if let Some(size) = needs_resize {
            if size.0 == 0 || size.1 == 0 {
                std::thread::sleep(std::time::Duration::from_millis(16));
                continue;
            }
            if let Err(e) = renderer.recreate_swapchain(&ctx, size) {
                eprintln!("Recreate swapchain failed: {}", e);
                break;
            }
        }

        renderer.camera.auto_orbit(start.elapsed().as_secs_f32());

        match renderer.draw_frame(&ctx) {
            Ok(true) => { frame_count += 1; }
            Ok(false) => {
                let (w, h) = window.get_framebuffer_size();
                if w > 0 && h > 0 {
                    if let Err(e) = renderer.recreate_swapchain(&ctx, (w as u32, h as u32)) {
                        eprintln!("Recreate failed: {}", e);
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
            last_log = Instant::now();
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
