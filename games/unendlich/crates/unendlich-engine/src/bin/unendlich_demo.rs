//! Headless game-loop demo.
//!
//! Spins up a `ChunkManager` with worker threads, walks a virtual player
//! across a 3D-chunked world, and reports loading/eviction over time.
//! Exercises every code path the real game would use except rendering and input.
//!
//! Run with:
//!     cargo run --bin unendlich-demo

use unendlich_engine::constants::{default_worker_count, CHUNK_SIZE, RENDER_DISTANCE, SEA_LEVEL, WORLD_SEED_DEFAULT};
use unendlich_engine::world::{Backend, ChunkManager, TerrainGenerator};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn main() {
    env_logger::Builder::from_default_env()
        .filter_level(log::LevelFilter::Info)
        .init();

    println!("Meinkraft demo (headless, 3D chunked world)");

    let workers = default_worker_count();
    println!("Worker threads: {}", workers);
    println!("Render distance: {} chunks (cubic)", RENDER_DISTANCE);

    let generator = match TerrainGenerator::new(WORLD_SEED_DEFAULT, Backend::Auto, 16) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("Failed to start terrain generator: {}", e);
            std::process::exit(1);
        }
    };
    println!("GPU acceleration: {}", if generator.has_gpu() { "enabled" } else { "disabled (CPU)" });

    let generator = Arc::new(generator);
    let mut manager = ChunkManager::new(Arc::clone(&generator), workers);

    // Spawn the player near the surface so the loaded set straddles surface chunks.
    let mut player_x = 0i32;
    let mut player_y = SEA_LEVEL;
    let mut player_z = 0i32;

    println!("\nWarming up around player ({}, {}, {})...", player_x, player_y, player_z);
    for _ in 0..40 {
        manager.update(player_x, player_y, player_z);
        std::thread::sleep(Duration::from_millis(25));
    }
    println!("Loaded chunks: {}", manager.loaded_count());

    let biome = manager.biome_at(player_x, player_y, player_z);
    println!(
        "Biome at ({}, {}, {}): {:?}  (T={:.2} H={:.2} E={:.2})",
        player_x, player_y, player_z, biome.biome,
        biome.temperature, biome.humidity, biome.erosion,
    );

    // Use the generator's surface bisection to find the actual ground at (0, 0).
    let surface_y = generator.cpu().surface_y_at(0, 0, -64, 256);
    match surface_y {
        Some(y) => {
            println!("Surface y at (0, 0): {}", y);
            println!("Block at surface:    {:?}", manager.block_at(0, y, 0));
            println!("Block above surface: {:?}", manager.block_at(0, y + 1, 0));
        }
        None => println!("No surface found near (0, 0) in y range [-64, 256]"),
    }

    let total_ticks = 60;
    println!("\nSimulating a {}-tick walk:", total_ticks);
    println!("  tick |  pos                 | loaded | pending | tick ms");
    println!("  -----|----------------------|--------|---------|--------");

    let start = Instant::now();
    for tick in 0..total_ticks {
        let t0 = Instant::now();
        manager.update(player_x, player_y, player_z);
        let tick_ms = t0.elapsed().as_secs_f64() * 1000.0;

        if tick % 5 == 0 || tick == total_ticks - 1 {
            println!(
                "  {:>4} | ({:>5},{:>5},{:>5}) | {:>6} | {:>7} | {:>6.2}",
                tick, player_x, player_y, player_z,
                manager.loaded_count(),
                manager.pending_count(),
                tick_ms,
            );
        }

        // Walk in a 3D pattern: east, then south, then up. Tests x, z, y streaming.
        if tick < 20 {
            player_x += CHUNK_SIZE;
        } else if tick < 40 {
            player_z += CHUNK_SIZE;
        } else {
            player_y += CHUNK_SIZE;
        }

        std::thread::sleep(Duration::from_millis(50));
    }

    let elapsed = start.elapsed().as_secs_f64();
    println!(
        "\nWalked in {:.2}s — final loaded: {}, pending: {}",
        elapsed, manager.loaded_count(), manager.pending_count()
    );

    println!("\nDraining remaining pending chunks...");
    let drain_start = Instant::now();
    while manager.pending_count() > 0 && drain_start.elapsed() < Duration::from_secs(30) {
        manager.update(player_x, player_y, player_z);
        std::thread::sleep(Duration::from_millis(50));
    }
    println!(
        "Drained in {:.2}s — final loaded: {}",
        drain_start.elapsed().as_secs_f64(), manager.loaded_count(),
    );
}
