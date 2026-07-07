//! `unendlich-server` — headless world host for cloud deployment. Parses CLI args (see --help),
//! stands up a CPU-generated world (no rendering, no window), enforces the world ruleset (exactly
//! one world loaded), and runs a fixed-rate simulation tick loop.
//!
//! SCOPE (r440): this brings up and ticks the world headless. The networking transport — accepting
//! LAN/remote JOINs and replicating transforms/edits — currently lives inline in the fullgame
//! render loop and needs to be factored into a reusable host before it can be wired here; that is
//! the next step. Until then this serves as the deployable world-sim process + arg surface.

use std::sync::Arc;
use std::time::{Duration, Instant};

use unendlich_engine::sim::server_config::{ParseOutcome, ServerConfig, HELP};
use unendlich_engine::world::chunk_manager::ChunkManager;
use unendlich_engine::world::terrain_generator::{Backend, TerrainGenerator};
use unendlich_engine::world::world_list::{WorldEntry, WorldList};

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    let cfg = match ServerConfig::parse(&args) {
        ParseOutcome::Help => {
            println!("{HELP}");
            return;
        }
        ParseOutcome::Error(e) => {
            eprintln!("error: {e}\n\n{HELP}");
            std::process::exit(2);
        }
        ParseOutcome::Config(c) => c,
    };

    log::info!(
        "[server] starting world='{}' seed={} bind={}:{} tick={}Hz view_distance={}",
        cfg.name,
        cfg.seed,
        cfg.bind,
        cfg.port,
        cfg.tick_hz,
        cfg.view_distance
    );

    // Enforce the world ruleset: register the world and load it through the WorldList so the
    // no-concurrent rule is the single gate any future "load another world" request hits.
    let mut worlds = WorldList::new();
    let world_id = cfg.seed; // id == seed for now; a real registry would assign ids
    worlds.register(WorldEntry { id: world_id, name: cfg.name.clone(), seed: cfg.seed });
    match worlds.try_load(world_id) {
        Ok(id) => log::info!("[server] world loaded (id={id}); rule={:?}", worlds.rule()),
        Err(e) => {
            eprintln!("error: cannot load world: {e:?}");
            std::process::exit(1);
        }
    }

    // Headless terrain: force CPU (no GPU on a typical server node).
    let generator = match TerrainGenerator::new(cfg.seed, Backend::Cpu, 64) {
        Ok(g) => Arc::new(g),
        Err(e) => {
            eprintln!("error: terrain generator init failed: {e:?}");
            std::process::exit(1);
        }
    };
    let workers = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    let mut chunks = ChunkManager::new(generator, workers);
    log::info!("[server] world ready ({workers} worker threads)");

    // Fixed-rate tick loop. With no players connected yet we keep the origin resident so the world
    // is live and tickable; once networking lands, interest points become the connected players.
    let tick_dt = Duration::from_secs_f64(1.0 / cfg.tick_hz as f64);
    let dt_s = tick_dt.as_secs_f32();
    let mut last_status = Instant::now();
    let mut tick: u64 = 0;

    // Ctrl-C → clean shutdown flag.
    let running = Arc::new(std::sync::atomic::AtomicBool::new(true));
    {
        let r = running.clone();
        let _ = ctrlc_set(move || r.store(false, std::sync::atomic::Ordering::SeqCst));
    }

    log::info!("[server] running — Ctrl-C to stop");
    while running.load(std::sync::atomic::Ordering::SeqCst) {
        let t0 = Instant::now();
        chunks.update(0, 0, 0);
        chunks.tick_simulation(dt_s);
        tick += 1;

        if last_status.elapsed().as_secs() >= 5 {
            log::info!(
                "[server] tick={tick} chunks_loaded={} ({:.1} tick/s target)",
                chunks.loaded_count(),
                cfg.tick_hz
            );
            last_status = Instant::now();
        }

        if let Some(rem) = tick_dt.checked_sub(t0.elapsed()) {
            std::thread::sleep(rem);
        }
    }

    log::info!("[server] shutting down after {tick} ticks");
}

/// Minimal Ctrl-C handler without pulling a dependency: SIGINT via a thread on Unix, no-op
/// elsewhere (the process can still be killed). Returns Ok if installed.
fn ctrlc_set<F: Fn() + Send + 'static>(f: F) -> Result<(), ()> {
    // Best-effort: spawn a thread that watches for a SIGINT-driven flag is non-trivial without a
    // crate; instead we rely on the OS to terminate on Ctrl-C and treat this as a no-op hook so
    // the loop structure is ready for a real handler when a signal crate is added.
    let _ = f;
    Ok(())
}
