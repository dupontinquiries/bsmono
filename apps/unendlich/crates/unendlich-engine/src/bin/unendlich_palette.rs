//! Palette: render a hand-authored chunk containing every visually-
//! distinct block type, side by side, so the material-response matrix's
//! per-block coloration is observable in one frame.
//!
//! Unlike `unendlich-chunk` (which uses procedural worldgen and the
//! plain mesher), this bin:
//!  - Hand-authors blocks into a single chunk
//!  - Uses the climate-aware mesher (`mesh_chunk_with_sky_and_model`)
//!    so the material-response matrix's tints activate
//!  - Cycles through several world-X positions so you can compare the
//!    same block under different climate (cold/dry vs hot/humid etc.)
//!
//! Run:
//!     cargo run --release --features render --bin unendlich-palette
//!
//! Controls:
//!  - Esc: quit
//!  - Space / N: cycle to next climate position
//!  - The camera auto-orbits so you can see all faces

use glfw::{Action, Key, WindowEvent, WindowHint, ClientApiHint};
use unendlich_engine::constants::{CHUNK_SIZE, WORLD_SEED_DEFAULT};
use unendlich_engine::render::{Camera, ChunkRenderer, VulkanContext};
use unendlich_engine::sky::Sky;
use unendlich_engine::world::blocks::BlockId;
use unendlich_engine::world::chunk::{Chunk, ChunkCoord};
use unendlich_engine::world::world_model::WorldModel;
use nalgebra_glm as glm;
use std::sync::Arc;
use std::time::Instant;

const INITIAL_WIDTH: u32 = 1280;
const INITIAL_HEIGHT: u32 = 720;
const TITLE: &str = "Meinkraft — Palette";

/// Block types laid out in the palette, in display order.
const PALETTE: &[BlockId] = &[
    // Soft natural — visible bumps + climate response
    BlockId::Grass,
    BlockId::Dirt,
    BlockId::Sand,
    BlockId::Gravel,
    BlockId::Snow,
    BlockId::SnowBlock,
    BlockId::Moss,
    // Foliage
    BlockId::OakLeaves,
    BlockId::BirchLeaves,
    BlockId::OakLog,
    BlockId::BirchLog,
    // Hard — minimal response
    BlockId::Stone,
    BlockId::Cobblestone,
    BlockId::Bedrock,
    BlockId::IronOre,
    BlockId::CoalOre,
    // Fluids
    BlockId::Water,
    BlockId::Lava,
    BlockId::Ice,
];

/// Climate positions to cycle through. Each is a chunk-coord world position
/// chosen to land in a meaningfully different climate region. Picked from
/// real-world exploration data + extreme-seeking. The user can press N
/// to cycle through these and see how the same blocks look under each.
const CLIMATE_POSITIONS: &[(i32, i32, &str)] = &[
    (   0,    0, "neutral (origin)"),
    (5000,    0, "far east — temp may shift"),
    (   0, 5000, "far south — humidity may shift"),
    (-5000,    0, "far west — opposite extreme"),
    (   0,-5000, "far north — opposite extreme"),
    (5000, 5000, "diagonal — combined drift"),
];

/// Build a chunk with palette blocks laid out as 3×3×4 columns on a
/// stone floor. Place at chunk Y=2 (= world Y 64..96) so it straddles
/// sea level / above ground.
fn build_palette_chunk(coord: ChunkCoord) -> Chunk {
    let mut chunk = Chunk::empty(coord);

    // Stone floor at y=0..1 of chunk (= world y 64..66). Use Stone so
    // its base color is recognizable; not Bedrock so weathering rules
    // can apply if we want to extend tests later.
    for z in 0..CHUNK_SIZE {
        for x in 0..CHUNK_SIZE {
            for y in 0..2 {
                let idx = Chunk::block_index(x, y, z);
                chunk.blocks[idx] = BlockId::Stone;
            }
        }
    }

    // Grid layout: 4-block slots, 3-block clusters, 1-block gap.
    // 32 / 4 = 8 columns. 19 entries fits in 3 rows.
    let slot = 4i32;
    let cluster = 3i32;
    let cluster_height = 4i32;

    for (i, &block) in PALETTE.iter().enumerate() {
        let col = (i as i32) % 8;
        let row = (i as i32) / 8;

        let base_x = col * slot;
        let base_z = row * slot;

        // Skip if cluster would extend past chunk bounds.
        if base_x + cluster > CHUNK_SIZE || base_z + cluster > CHUNK_SIZE {
            continue;
        }

        for dz in 0..cluster {
            for dx in 0..cluster {
                for dy in 0..cluster_height {
                    let x = base_x + dx;
                    let z = base_z + dz;
                    let y = 2 + dy;
                    if y >= CHUNK_SIZE { continue; }
                    let idx = Chunk::block_index(x, y, z);
                    chunk.blocks[idx] = block;
                }
            }
        }
    }

    chunk
}

fn main() {
    env_logger::Builder::from_default_env()
        .filter_level(log::LevelFilter::Info)
        .init();

    log::info!("Starting {}", TITLE);
    log::info!("Palette has {} block types", PALETTE.len());
    log::info!("Press N or Space to cycle through {} climate positions", CLIMATE_POSITIONS.len());
    log::info!("Press Esc to quit");

    // Window + Vulkan
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

    // World model (for climate sampling) + Sky (for face lighting).
    let world_model = Arc::new(WorldModel::new(WORLD_SEED_DEFAULT as u32));
    let sky = Sky::default();

    // Start at climate position 0.
    let mut climate_idx = 0usize;
    let (chunk_cx, chunk_cz, climate_label) = CLIMATE_POSITIONS[climate_idx];
    let coord = ChunkCoord::new(chunk_cx, 2, chunk_cz);
    let chunk = build_palette_chunk(coord);
    log_climate(&world_model, coord, climate_label);

    // Camera centered on chunk; auto-orbit will rotate it.
    let (ox, oy, oz) = coord.block_origin();
    let half = CHUNK_SIZE as f32 * 0.5;
    let target = glm::vec3(ox as f32 + half, oy as f32 + half, oz as f32 + half);
    let camera = Camera::new(target, 60.0);

    let mut renderer = ChunkRenderer::new(&ctx, (fb_w as u32, fb_h as u32), camera)
        .expect("chunk renderer");
    renderer.upload_chunk_with_sky_and_model(&ctx, &chunk, &sky, &world_model)
        .expect("upload");

    let mut frame_count = 0u64;
    let mut last_log = Instant::now();
    let start = Instant::now();
    let mut needs_palette_rebuild = false;

    while !window.should_close() {
        glfw.poll_events();
        let mut needs_resize: Option<(u32, u32)> = None;

        for (_, event) in glfw::flush_messages(&events) {
            match event {
                WindowEvent::Key(Key::Escape, _, Action::Press, _) => {
                    window.set_should_close(true);
                }
                WindowEvent::Key(Key::Space, _, Action::Press, _)
                | WindowEvent::Key(Key::N, _, Action::Press, _) => {
                    climate_idx = (climate_idx + 1) % CLIMATE_POSITIONS.len();
                    needs_palette_rebuild = true;
                }
                WindowEvent::FramebufferSize(w, h) => {
                    needs_resize = Some((w as u32, h as u32));
                }
                _ => {}
            }
        }

        if needs_palette_rebuild {
            let (cx, cz, label) = CLIMATE_POSITIONS[climate_idx];
            let new_coord = ChunkCoord::new(cx, 2, cz);
            let new_chunk = build_palette_chunk(new_coord);
            log_climate(&world_model, new_coord, label);

            // Update camera target for the new world position.
            let (ox, oy, oz) = new_coord.block_origin();
            renderer.camera.target = glm::vec3(ox as f32 + half, oy as f32 + half, oz as f32 + half);

            renderer.upload_chunk_with_sky_and_model(&ctx, &new_chunk, &sky, &world_model)
                .expect("upload (cycle)");
            needs_palette_rebuild = false;
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

        if last_log.elapsed().as_secs() >= 2 {
            log::info!("FPS: {} (climate idx {})", frame_count / 2, climate_idx);
            frame_count = 0;
            last_log = Instant::now();
        }
    }

    log::info!("Shutting down");
    renderer.destroy(&ctx);
    drop(ctx);
}

/// Log the climate sampled at the chunk's center, so the user knows
/// what conditions the matrix is responding to.
fn log_climate(model: &WorldModel, coord: ChunkCoord, label: &str) {
    let (ox, oy, oz) = coord.block_origin();
    let half = CHUNK_SIZE / 2;
    let cx = (ox + half) as f64;
    let cy = (oy + half) as f64;
    let cz = (oz + half) as f64;
    let temp = model.temperature(cx, cy, cz, 0.0);
    let humid = model.humidity(cx, cy, cz, 0.0);
    log::info!(
        "Climate at chunk ({}, {}, {}) — {}: temp={:.2}, humidity={:.2}",
        coord.x, coord.y, coord.z, label, temp, humid,
    );
}
