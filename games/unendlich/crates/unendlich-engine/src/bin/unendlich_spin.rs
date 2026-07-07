//! Spinning Chunks: 10×10×10 chunks generated with a camera orbiting the cluster.
//!
//! Pulls 1000 chunks from the world generator, uploads each one's mesh, and
//! renders with frustum-culled draw calls while the camera spins around the
//! cluster center. This is a stress test of the multi-chunk render path
//! AND a useful visual sanity check for the new 3D-noise WorldModel
//! generation: you should see a coherent surface band cutting through the
//! middle of the cluster, with deep stone below and air above, and ores
//! visible if you stop the spin and dig in.
//!
//! Run:
//!     cargo run --release --features render --bin unendlich-spin
//!
//! Press Esc to quit.

use glfw::{Action, Key, WindowEvent, WindowHint, ClientApiHint};
use unendlich_engine::constants::{CHUNK_SIZE, SEA_LEVEL, WORLD_SEED_DEFAULT};
use unendlich_engine::render::{FlyCamera, VulkanContext, WorldRenderer};
use unendlich_engine::world::chunk::ChunkCoord;
use unendlich_engine::world::terrain_generator::{Backend, TerrainGenerator};
use nalgebra_glm as glm;
use std::time::Instant;

const INITIAL_WIDTH: u32 = 1280;
const INITIAL_HEIGHT: u32 = 720;
const TITLE: &str = "Meinkraft — Spinning Chunks";

/// Cube edge in chunks. Total chunks = SIDE^3. Default 10
/// (1000 chunks). Env var `UNENDLICH_SIDE` overrides.
const SIDE_DEFAULT: i32 = 10;

/// How many chunks to upload per frame at most. Keeps the first few seconds
/// from being a single multi-second hitch on initial upload. Env var
/// `UNENDLICH_MAX_UPLOADS_PER_FRAME` overrides.
const MAX_UPLOADS_PER_FRAME_DEFAULT: usize = 16;

fn main() {
    if unendlich_engine::runtime_flags::wants_help() {
        unendlich_engine::runtime_flags::print_help("unendlich-spin");
        return;
    }
    if unendlich_engine::runtime_flags::wants_version() {
        unendlich_engine::runtime_flags::print_version("unendlich-spin");
        return;
    }

    env_logger::Builder::from_default_env()
        .filter_level(log::LevelFilter::Info)
        .init();

    log::info!("Starting {}", TITLE);

    // -----------------------------------------------------------------------
    // Runtime knobs (env-overridable)
    // -----------------------------------------------------------------------
    // SIDE up to 32 (32768 chunks) is the practical ceiling on most
    // hardware; beyond that the upload alone takes minutes. Keep the
    // door open if you really want to.
    let side: i32 = unendlich_engine::runtime_flags::env_i32(
        "UNENDLICH_SIDE", SIDE_DEFAULT, 1..=64);
    let max_uploads_per_frame: usize = unendlich_engine::runtime_flags::env_usize(
        "UNENDLICH_MAX_UPLOADS_PER_FRAME",
        MAX_UPLOADS_PER_FRAME_DEFAULT,
        4096,
    );
    log::info!(
        "spin config: SIDE={} ({} chunks), MAX_UPLOADS_PER_FRAME={}",
        side, side*side*side, max_uploads_per_frame,
    );

    // -----------------------------------------------------------------------
    // World setup
    // -----------------------------------------------------------------------
    log::info!("Generating {} chunks...", side * side * side);
    let generator = match TerrainGenerator::new(WORLD_SEED_DEFAULT, Backend::Auto, 16) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("Failed to start terrain generator: {}", e);
            std::process::exit(1);
        }
    };
    log::info!("GPU acceleration: {}", if generator.has_gpu() { "enabled" } else { "disabled (CPU)" });

    // Generate the chunk cluster centered around (0, sea-level chunk, 0). The
    // sea-level chunk is `SEA_LEVEL / CHUNK_SIZE`, and we offset y by half the
    // cube so the surface band passes through the cluster's vertical middle.
    let center_chunk_y = SEA_LEVEL / CHUNK_SIZE;
    let half = side / 2;
    let mut coords = Vec::with_capacity((side * side * side) as usize);
    for cy in (center_chunk_y - half)..(center_chunk_y + half) {
        for cz in -half..half {
            for cx in -half..half {
                coords.push(ChunkCoord::new(cx, cy, cz));
            }
        }
    }
    log::info!("Coord range: x={}..{}, y={}..{}, z={}..{}",
        -half, half,
        center_chunk_y - half, center_chunk_y + half,
        -half, half,
    );

    let gen_start = Instant::now();
    let chunks = generator.generate(&coords);
    let gen_elapsed = gen_start.elapsed().as_secs_f64();
    let total_solid: usize = chunks.iter()
        .map(|c| c.blocks.iter().filter(|b| b.is_solid()).count())
        .sum();
    log::info!(
        "Generated {} chunks in {:.2}s ({:.0} cps, {} solid blocks total)",
        chunks.len(), gen_elapsed,
        chunks.len() as f64 / gen_elapsed,
        total_solid,
    );

    // -----------------------------------------------------------------------
    // Window + Vulkan setup
    // -----------------------------------------------------------------------
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

    // -----------------------------------------------------------------------
    // Camera setup
    // -----------------------------------------------------------------------
    // Cluster center in WORLD-space blocks. The cluster spans:
    //   x ∈ [-half * CHUNK_SIZE, half * CHUNK_SIZE)
    //   y ∈ [(center_y - half) * CHUNK_SIZE, (center_y + half) * CHUNK_SIZE)
    //   z ∈ [-half * CHUNK_SIZE, half * CHUNK_SIZE)
    let cluster_center = glm::vec3(
        0.0,
        (center_chunk_y as f32) * (CHUNK_SIZE as f32),
        0.0,
    );
    let cluster_extent = (side * CHUNK_SIZE) as f32;

    // Orbit radius: 1.4× the cluster's half-diagonal so we frame the whole thing.
    let orbit_radius = cluster_extent * 0.9;

    // Place the camera at the start position. We'll update it each frame.
    let initial_pos = cluster_center + glm::vec3(orbit_radius, 0.0, 0.0);
    let mut camera = FlyCamera::new(initial_pos);
    point_camera_at(&mut camera, &cluster_center);

    let mut renderer = WorldRenderer::new(&ctx, (fb_w as u32, fb_h as u32), camera)
        .expect("world renderer");

    log::info!("Camera orbit: center={:?}, radius={:.1}", cluster_center, orbit_radius);
    log::info!("Press Esc to quit.");

    // -----------------------------------------------------------------------
    // Upload chunks gradually
    // -----------------------------------------------------------------------
    // We'll feed chunks into the renderer over the first few frames — uploading
    // 1000 chunks in one frame would block render for many seconds.

    let start = Instant::now();
    let mut next_to_upload = 0;
    let mut total_uploaded = 0;

    let mut frame_count = 0u64;
    let mut last_log = Instant::now();
    let mut total_frame_count = 0u64;

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

        // Upload some chunks this frame.
        let mut uploads_this_frame = 0;
        while next_to_upload < chunks.len() && uploads_this_frame < max_uploads_per_frame {
            if let Err(e) = renderer.upload_chunk(&ctx, &chunks[next_to_upload]) {
                log::warn!("Upload failed for chunk {:?}: {}", chunks[next_to_upload].coord, e);
            } else {
                total_uploaded += 1;
            }
            next_to_upload += 1;
            uploads_this_frame += 1;
        }

        // Spin the camera around the cluster.
        let t = start.elapsed().as_secs_f32();
        let yaw = t * 0.25; // rad/sec
        let pitch = (t * 0.13).sin() * 0.35; // gentle bob between -0.35 and +0.35 rad

        let pos = cluster_center
            + glm::vec3(
                orbit_radius * yaw.cos() * pitch.cos(),
                orbit_radius * pitch.sin(),
                orbit_radius * yaw.sin() * pitch.cos(),
            );
        renderer.camera.position = pos;
        point_camera_at(&mut renderer.camera, &cluster_center);

        match renderer.draw_frame(&ctx) {
            Ok(true) => {
                frame_count += 1;
                total_frame_count += 1;
            }
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
            log::info!(
                "FPS: {:>4}  uploaded: {}/{}  total frames: {}",
                frame_count, total_uploaded, chunks.len(), total_frame_count,
            );
            frame_count = 0;
            last_log = Instant::now();
        }
    }

    log::info!("Shutting down");
    renderer.destroy(&ctx);
    drop(ctx);
}

/// Set the FlyCamera's yaw/pitch so it looks at `target` from its current position.
fn point_camera_at(camera: &mut FlyCamera, target: &glm::Vec3) {
    let dir = *target - camera.position;
    let horiz_len = (dir.x * dir.x + dir.z * dir.z).sqrt();
    camera.yaw = dir.z.atan2(dir.x);
    camera.pitch = dir.y.atan2(horiz_len);
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
