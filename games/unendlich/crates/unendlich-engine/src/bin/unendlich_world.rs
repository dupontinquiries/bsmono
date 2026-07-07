//! Multi-chunk client: walk around a world rendered with naive mesh
//! face-culling, GPU-generated terrain (CPU fallback), frustum culling,
//! and mouse-look + WASD fly controls.
//!
//! Run:
//!     cargo run --release --features render --bin unendlich-world
//!
//! Cap FPS to monitor refresh (saves fan spin, recommended for laptops):
//!     UNENDLICH_VSYNC=1 cargo run --release --features render --bin unendlich-world
//!
//! Controls:
//!   * Click in the window to capture the mouse.
//!   * WASD-equivalent (defaults to QWERTY layout): move horizontally
//!   * Space / Left-Shift: fly up / down
//!   * Esc: release mouse (first press), close window (second press)
//!   * F4: toggle in-world debug overlay (player AABB, wind arrow,
//!         velocity arrow, plant lifecycle markers, tick-cull boundary).
//!         Watch stderr for the verbose toggle confirmation message.
//!   * F6 / F7: tick speed slower / faster (geometric, sqrt(10) per press).
//!         Lets you fast-forward slow simulation features (plant lifecycle,
//!         leaf decay, weathering) for showcase or debugging. Range
//!         [0.01x, 1000x]. Default 1.0x. Watch stderr for current value.

use glfw::{Action, CursorMode, Key, MouseButton, MouseButtonLeft, WindowEvent, WindowHint, ClientApiHint};
use unendlich_engine::constants::{runtime_render_distance, WORLD_SEED_DEFAULT, WINDOW_WIDTH, WINDOW_HEIGHT, WINDOW_NAME};
use unendlich_engine::input::InputCache;
use unendlich_engine::keybindings::{GameAction, KeyBindings};
use unendlich_engine::physics::{self, PhysicsBody, PhysicsParams};
use unendlich_engine::raycast;
use unendlich_engine::render::{FlyCamera, MeshRequest, MeshingPool, VulkanContext, WorldRenderer};
use unendlich_engine::world::blocks::BlockId;
use unendlich_engine::world::chunk::ChunkCoord;
use unendlich_engine::world::chunk_manager::ChunkManager;
use unendlich_engine::world::lod::LodLevel;
use unendlich_engine::world::terrain_generator::{Backend, TerrainGenerator};
use nalgebra_glm as glm;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;

// Window size and title now live in `unendlich_engine::constants` so they're
// settable from one place. The bin-local `INITIAL_WIDTH`/`INITIAL_HEIGHT`
// /`TITLE` constants that used to shadow them are gone.

/// Default frame-time budget for GPU uploads, in milliseconds. We
/// upload completed meshes one at a time and stop when this budget is
/// exceeded — this keeps frame variance bounded even on bursty load.
/// Overridable at runtime via `UNENDLICH_UPLOAD_BUDGET_MS`.
///
/// v0.24.42-upload-budget: raised from 4.0 to 6.0. After RC5+RC6 the
/// Mid GPU mesher produces ~3,100 chunks/sec sustained; the bottleneck
/// during post-movement transitions is the upload pipeline draining,
/// not the mesher itself. 6 ms gives the per-frame upload phase more
/// room without stretching the worst-case frame past the 8.3 ms
/// vsync budget on 120 Hz displays. Backstop is still
/// `MAX_UPLOADS_PER_FRAME_DEFAULT` and `UPLOAD_BYTES_BUDGET_DEFAULT`.
const UPLOAD_BUDGET_MS_DEFAULT: f64 = 6.0;

/// Default cap on how many uploads we'll do in a single frame even if
/// the budget allows. Prevents pathological cases where uploads are
/// extremely cheap (empty meshes) from dominating the frame.
/// Overridable at runtime via `UNENDLICH_MAX_UPLOADS_PER_FRAME`.
///
/// v0.24.42-upload-budget: raised from 16 to 64. Each upload's CPU
/// overhead (memcpy of vertex+index into the host-visible mapped pool,
/// plus the small bookkeeping work in `upload_chunk_mesh`) is well
/// under 100 µs in practice. At the old cap=16 the upload phase
/// drained the queue at 960 chunks/sec at 60 fps; the mesher peaks
/// near 3,100. New cap matches the mesher's peak so the queue can
/// fully drain in a single frame when bursts hit.
const MAX_UPLOADS_PER_FRAME_DEFAULT: usize = 64;

/// v0.24.3 upload-pressure investigation: default per-frame upload BYTES
/// budget. Caps total mesh bytes (vertex_bytes + index_bytes) memcpy'd
/// into the persistent-mapped pool buffer per frame. Acts alongside
/// `UPLOAD_BUDGET_MS_DEFAULT`; whichever fires first wins.
///
/// Why bytes and not just ms: on M2's unified-memory architecture, the
/// CPU's memcpy into HOST_COHERENT mapped memory triggers cache-line
/// invalidation traffic that the GPU pays for on its next read of the
/// same buffer. Both shadow and main draws read the same pool, so the
/// stall affects both passes proportionally — matching the observed
/// chug pattern from chug-debug-2 (shadow + main spike together).
///
/// Capping bytes (not just time) gives a more direct lever on the
/// memory-contention cost than wall-clock time, which varies with
/// memcpy throughput.
///
/// Default 8 MB. Observed peak in v0.24.3-multi-queue-a was ~30+ MB/s
/// during chug windows; 8 MB caps each frame well under that.
/// Overridable at runtime via `UNENDLICH_UPLOAD_BYTES_BUDGET`.
const UPLOAD_BYTES_BUDGET_DEFAULT: usize = 8 * 1024 * 1024;

/// v0.24.3 upload-pressure investigation: emit an `upload_pressure`
/// event when per-frame upload bytes exceeds this threshold. Hunting
/// frames that pay the memory-contention cost; lower threshold catches
/// more, higher reduces noise. 2 MB picked to catch most chug-correlated
/// frames without flooding events.jsonl during normal play.
const UPLOAD_PRESSURE_EVENT_THRESHOLD_BYTES: usize = 2 * 1024 * 1024;

/// Number of mesh-worker threads. CPU-bound, so leave 2 cores for the OS
/// + main thread. M2 Max has 12 perf+eco cores, so 8 is reasonable.
const MESH_WORKERS: usize = 4;

/// Mesh request queue capacity. ~4× per-frame upload cap so workers stay
/// busy across player movement bursts.
const MESH_QUEUE_CAPACITY: usize = 64;

/// A live, streamed creature roaming the world, with everything its behaviour
/// needs: body + wander state, the chunk it belongs to, whether it's persistent
/// (dragons never despawn/flee), its genome-derived senses/turn character, and
/// its ecology — energy reserve, sex, species, and genome (for grazing,
/// starvation, and mating).
struct Roamer {
    creature: unendlich_engine::creature::Creature,
    wander: unendlich_engine::sim::cognition::wander::Wander,
    home: (i32, i32),
    persistent: bool,
    senses: unendlich_engine::sim::senses::Senses,
    turn_rate: f32,
    turn_profile: unendlich_engine::sim::cognition::steering::TurnProfile,
    vitality: unendlich_engine::sim::feeding::Vitality,
    sex: unendlich_engine::genetics::Sex,
    kind: unendlich_engine::item::CreatureKind,
    genome: unendlich_engine::genetics::Genome,
    forage_memory: unendlich_engine::sim::forage_memory::ForageMemory,
    leap: unendlich_engine::sim::leap::Leap,
    /// In-air ballistic state during a leap: (world velocity, ticks aloft).
    airborne: Option<([f32; 3], u16)>,
    posture: unendlich_engine::sim::posture::Posture,
    stumble: unendlich_engine::sim::stumble::Stumble,
}

fn main() {
    // Handle --help / --version BEFORE doing any setup. This keeps the
    // help printer fast and side-effect-free (no logger init, no Vulkan,
    // no GLFW). The list of flags is shared via the runtime_flags
    // registry — see src/runtime_flags.rs.
    if unendlich_engine::runtime_flags::wants_help() {
        unendlich_engine::runtime_flags::print_help("unendlich-world");
        return;
    }
    if unendlich_engine::runtime_flags::wants_version() {
        unendlich_engine::runtime_flags::print_version("unendlich-world");
        return;
    }

    // Profile presets: UNENDLICH_PROFILE=<name> expands a blessed flag bundle
    // (see runtime_flags::profile). Applied here, BEFORE any env::var read, with
    // set-if-unset semantics so explicit env vars on the command line always win.
    if let Ok(prof) = std::env::var("UNENDLICH_PROFILE") {
        match unendlich_engine::runtime_flags::profile(&prof) {
            Some(pairs) => {
                let (mut applied, mut overridden) = (0usize, 0usize);
                for (k, v) in pairs {
                    if std::env::var(k).is_err() {
                        std::env::set_var(k, v);
                        applied += 1;
                    } else {
                        overridden += 1;
                    }
                }
                eprintln!("[profile] '{prof}': applied {applied} flag(s); {overridden} kept as explicit override(s)");
            }
            None => eprintln!(
                "[profile] unknown profile '{prof}' (known: {:?}) — ignoring",
                unendlich_engine::runtime_flags::profile_names()
            ),
        }
    }

    // v0.24.2c.9: optional --test-spec PATH. Loads a .unendtest file,
    // applies its [env] table as process env vars (so all subsequent
    // env_var reads see them), and arms a result-file writer for
    // shutdown. Spec-driven mode = automated regression test mode;
    // un-set means interactive playtest as usual.
    let loaded_spec: Option<unendlich_shared::test_spec::LoadedSpec> = {
        let argv: Vec<String> = std::env::args().collect();
        let mut spec_path: Option<String> = None;
        let mut i = 1usize;
        while i < argv.len() {
            match argv[i].as_str() {
                "--test-spec" => {
                    if i + 1 >= argv.len() {
                        eprintln!("--test-spec requires a path argument");
                        std::process::exit(2);
                    }
                    spec_path = Some(argv[i + 1].clone());
                    i += 2;
                }
                other if other.starts_with("--test-spec=") => {
                    spec_path = Some(other["--test-spec=".len()..].to_string());
                    i += 1;
                }
                _ => i += 1,
            }
        }
        spec_path.map(|p| {
            match unendlich_shared::test_spec::LoadedSpec::load(&p) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("Failed to load test spec {}: {}", p, e);
                    std::process::exit(2);
                }
            }
        })
    };

    // If we have a spec, apply its env-var overrides BEFORE the UUID
    // and logger init. Subsequent env::var reads (in renderer
    // construction, chunk manager, etc.) see the overrides. Also
    // synthesize MAX_DURATION_MS from engine.duration_ms and the
    // events file path from output.events_path so the rest of main
    // stays spec-agnostic.
    if let Some(ref ls) = loaded_spec {
        for (k, v) in &ls.spec.env {
            std::env::set_var(k, v);
        }
        if let Some(ms) = ls.spec.engine.duration_ms {
            std::env::set_var("UNENDLICH_MAX_DURATION_MS", ms.to_string());
        }
        // c.13-instrumentation: propagate warmup_ms as env var. The
        // main loop reads UNENDLICH_WARMUP_MS and resets the GPU+CPU
        // timer lifetime accumulators when warmup elapses.
        if let Some(ms) = ls.spec.engine.warmup_ms {
            std::env::set_var("UNENDLICH_WARMUP_MS", ms.to_string());
        }
        let ev_path = ls.events_path();
        std::env::set_var("UNENDLICH_EVENTS_PATH", ev_path);
    }

    // v0.24.2c.8: generate a unique run UUID first so it can prefix
    // every log line AND tag every structured event. The UUID lets us
    // cross-reference terminal logs, events.jsonl, and the RunBlock in
    // run_summaries.txt.
    let run_uuid = unendlich_shared::run_id::fresh();

    // Init env_logger with a custom format that prefixes each line
    // with the run UUID. Format mirrors env_logger's default but adds
    // the UUID as the first bracketed field.
    {
        let uuid_for_logger = run_uuid.clone();
        env_logger::Builder::from_default_env()
            .filter_level(log::LevelFilter::Info)
            .format(move |buf, record| {
                use std::io::Write;
                // v0.24.NEXT+16-warn-snipe: tally WARN/ERROR by site so the
                // shutdown block lists which warnings fired this run. No-op
                // unless UNENDLICH_WARN_SNIPE is set.
                if record.level() <= log::Level::Warn {
                    unendlich_engine::warn_snipe::record(
                        record.level().as_str(),
                        record.target(),
                        &record.args().to_string(),
                    );
                }
                writeln!(
                    buf,
                    "[{} {} {} {}] {}",
                    unendlich_shared::run_summary::iso8601_utc_now(),
                    record.level(),
                    uuid_for_logger,
                    record.target(),
                    record.args(),
                )
            })
            .init();
    }

    // Init the structured event sink. Subsequent `events::record(…)`
    // calls go to events.jsonl alongside the human-readable terminal log.
    unendlich_shared::events::init(&run_uuid);
    unendlich_shared::events::record("start", serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "binary": "unendlich-world",
        "cmdline": std::env::args().collect::<Vec<_>>().join(" "),
    }));
    unendlich_engine::runtime_flags::emit_flag_events("unendlich-world");

    log::info!("Starting {} (run {})", WINDOW_NAME, run_uuid);

    // v0.24.18: initialize the thread probe (default-off, opt-in via
    // UNENDLICH_THREAD_PROBE=1). Registers this thread (the render +
    // main loop thread) so the sampler can see its per-tick activity.
    // Started right after logger init so all subsequent thread spawns
    // can register themselves.
    unendlich_engine::thread_probe::init();
    let _render_probe = unendlich_engine::thread_probe::register("render");
    unendlich_engine::thread_probe::start_sampler();

    // v0.24.22-worker-isolation: apply the Render role to this thread.
    // Currently this is a no-op (the main thread is already adequately
    // prioritized by the kernel), but the call is here for symmetry
    // and so the role-assignment log line appears in the trace before
    // any worker threads spawn. Toggled by UNENDLICH_WORKER_ISOLATION;
    // default on.
    unendlich_engine::thread_priority::configure_current_thread(
        unendlich_engine::thread_priority::ThreadRole::Render,
    );

    // -----------------------------------------------------------------------
    // World setup
    // -----------------------------------------------------------------------
    // GPU terrain-gen batch size: how many chunks the GPU generator computes
    // per dispatch. Was hardcoded to 16, which capped world-fill at ~16
    // chunks/frame no matter how high the downstream mesh/upload batches were.
    // Tunable via UNENDLICH_GPU_GEN_BATCH so RD=24 mass-load can open the
    // upstream feeder. Default 16 (unchanged) for parity.
    let gen_batch: u32 = std::env::var("UNENDLICH_GPU_GEN_BATCH")
        .ok()
        .and_then(|s| s.parse::<u32>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(16);
    log::info!("GPU terrain-gen batch size = {} (UNENDLICH_GPU_GEN_BATCH)", gen_batch);
    let generator = match TerrainGenerator::new(WORLD_SEED_DEFAULT, Backend::Auto, gen_batch) {
        Ok(g) => Arc::new(g),
        Err(e) => {
            eprintln!("Failed to create terrain generator: {}", e);
            std::process::exit(1);
        }
    };
    log::info!("Terrain generator ready (gpu={})", generator.has_gpu());

    let num_workers = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    // World persistence path: pristine chunks aren't saved (no overhead);
    // only chunks with player edits write to disk.
    //
    // Two modes:
    //
    // - **Interactive play** (no --test-spec): persist to `./worlds/default/`
    //   so the user's edits survive across launches.
    //
    // - **Spec-driven run** (--test-spec was passed): persist to a per-run
    //   directory under the launcher's run-output area
    //   (`~/.unendlich/runs/<run-uuid>/world/` when UNENDLICH_RESULT_PATH is
    //   set by the launcher, otherwise a sibling of the result file).
    //   This keeps spec runs hermetic — re-running the same spec doesn't
    //   read back edits from a prior run, so cross-version perf comparisons
    //   measure the same workload each time.
    //
    // c.12-e: introduced to fix non-determinism in cross-version compare.
    // Prior to this, repeated `--test-spec` runs accumulated `worlds/default/`
    // state in cwd, so the second run of the same spec started with chunks
    // already edited by the first.
    let world_root = if loaded_spec.is_some() {
        // Derive from where the result file is going. The launcher sets
        // UNENDLICH_RESULT_PATH to a per-run path; we put the world dir
        // alongside the result for self-contained run artifacts.
        let result_dir = std::env::var("UNENDLICH_RESULT_PATH").ok()
            .and_then(|p| std::path::PathBuf::from(p).parent().map(|p| p.to_path_buf()))
            .unwrap_or_else(|| std::env::temp_dir().join(format!("unendlich-spec-{}", run_uuid)));
        let world_dir = result_dir.join("world");
        log::info!("Spec-driven run: world persistence at {} (hermetic)", world_dir.display());
        world_dir
    } else {
        log::info!("Interactive run: world persistence at worlds/default/");
        std::path::PathBuf::from("worlds/default")
    };
    // Field overlays: derived state (wind, moisture, etc.) recomputed lazily
    // per chunk. The moisture field uses a real WorldModel for biome-driven
    // baselines + scans water/lava cells in each chunk for proximity boost.
    // Build a separate WorldModel sharing the same seed (cheap; the model
    // is just region-grid noise samplers).
    let model_for_fields = std::sync::Arc::new(
        unendlich_engine::world::world_model::WorldModel::new(WORLD_SEED_DEFAULT as u32)
    );
    let field_registry = std::sync::Arc::new(
        unendlich_engine::world::fields::FieldRegistry::with_world_model(model_for_fields)
    );
    // Tick registry: register the plant lifecycle handler. Future ticks
    // (leaf decay, fluid flow, mob AI) get registered here too. Each
    // handler is a closure; the registry holds them in a Vec and runs
    // them in order on each tick.
    let mut tick_reg = unendlich_engine::world::ticks::TickRegistry::new();
    tick_reg.register(
        "plant_lifecycle",
        unendlich_engine::world::plant_lifecycle::lifecycle_handler(),
    );
    // Leaf decay is intentionally NOT registered as an auto-firing
    // handler. With age-based decay every leaf dies after ~50-100s
    // real time regardless of context — empties forests. The handler
    // is meant for "leaves whose supporting log is broken decay
    // quickly" semantics, but the support relationship isn't wired
    // yet. Until then, leaves are permanent. Re-enable when the
    // log-removal trigger is in place:
    //
    //   tick_reg.register("leaf_decay",
    //       unendlich_engine::world::leaf_decay::decay_handler());
    let _ = unendlich_engine::world::leaf_decay::decay_handler; // keep symbol live
    let tick_registry = std::sync::Arc::new(tick_reg);
    let mut chunk_manager = ChunkManager::new(generator, num_workers)
        .with_storage(world_root)
        .with_fields(field_registry)
        .with_ticks(tick_registry);
    log::info!("ChunkManager ready ({} workers, render distance {})", num_workers, runtime_render_distance());

    // -----------------------------------------------------------------------
    // Window + Vulkan
    // -----------------------------------------------------------------------
    let mut glfw = glfw::init(glfw::FAIL_ON_ERRORS).expect("init GLFW");
    if !glfw.vulkan_supported() {
        eprintln!("ERROR: GLFW reports no Vulkan support.");
        std::process::exit(1);
    }
    glfw.window_hint(WindowHint::ClientApi(ClientApiHint::NoApi));
    glfw.window_hint(WindowHint::Resizable(true));

    let (mut window, events) = glfw.create_window(
        WINDOW_WIDTH, WINDOW_HEIGHT, WINDOW_NAME, glfw::WindowMode::Windowed,
    ).expect("create window");
    window.set_key_polling(true);
    window.set_mouse_button_polling(true);
    window.set_cursor_pos_polling(true);
    window.set_framebuffer_size_polling(true);
    // Initial cursor mode — visible until the user clicks to capture.
    window.set_cursor_mode(CursorMode::Normal);

    let ctx = VulkanContext::new(&glfw, &window).expect("vulkan context");

    // Capture GPU + driver identity once at startup. Stays constant for
    // the run; gets folded into the result file at shutdown so the
    // launcher's `compare` command can detect driver-epoch drift between
    // compared runs.
    let gpu_info = ctx.gpu_info();
    log::info!(
        "GPU info: vendor=0x{:04x} device=0x{:04x} driver_version_raw=0x{:08x} name={:?} info={:?} hash={}",
        gpu_info.vendor_id, gpu_info.device_id, gpu_info.driver_version_raw,
        gpu_info.driver_name, gpu_info.driver_info, gpu_info.driver_hash,
    );
    // c.13-scout: surface multiview support. Decides whether c.17's
    // multiview-CSM is feasible on this driver. `supports_csm()` checks
    // both that multiview is enabled AND that ≥4 view layers are
    // available (one per cascade).
    log::info!(
        "Multiview: {} | CSM-capable: {}",
        gpu_info.multiview.summary(),
        gpu_info.multiview.supports_csm(),
    );
    // Also emit as a structured event so the analyzer / launcher can
    // surface it programmatically (e.g. compare two runs and detect
    // multiview-capability differences across drivers).
    unendlich_shared::events::record("multiview_probe", serde_json::json!({
        "supported": gpu_info.multiview.multiview,
        "max_view_count": gpu_info.multiview.max_view_count,
        "max_instance_index": gpu_info.multiview.max_instance_index,
        "geometry_shader": gpu_info.multiview.geometry_shader,
        "tessellation_shader": gpu_info.multiview.tessellation_shader,
        "csm_capable": gpu_info.multiview.supports_csm(),
    }));

    // -----------------------------------------------------------------------
    // Camera + renderer
    // -----------------------------------------------------------------------
    // Spawn the camera at the actual terrain surface so the world is
    // immediately visible and walkable. We probe the worldgen for surface_y
    // at (0, 0) and place the camera a few blocks above it.
    let spawn_xz = (0i32, 0i32);
    let probe_y_min = -8 * unendlich_engine::constants::CHUNK_SIZE;
    let probe_y_max = 16 * unendlich_engine::constants::CHUNK_SIZE;
    let probe_y = chunk_manager.generator().cpu()
        .surface_y_at(spawn_xz.0, spawn_xz.1, probe_y_min, probe_y_max)
        .unwrap_or(64);
    let spawn_y = (probe_y + 4) as f32;
    log::info!("Surface y at spawn ({},{}) is {} — spawning camera at y={}",
        spawn_xz.0, spawn_xz.1, probe_y, spawn_y);
    let spawn_pos = glm::vec3(spawn_xz.0 as f32 + 0.5, spawn_y, spawn_xz.1 as f32 + 0.5);
    let mut camera = FlyCamera::new(spawn_pos);
    camera.move_speed = 32.0; // a bit faster than default for exploration
    camera.far = 1024.0;

    let (fb_w, fb_h) = window.get_framebuffer_size();
    let mut renderer = WorldRenderer::new(&ctx, (fb_w as u32, fb_h as u32), camera)
        .expect("world renderer");
    log::info!("Renderer ready ({}x{}). Click to capture mouse.", fb_w, fb_h);

    // Entity registry + a few test mobs near spawn so mob rendering is visible.
    // These sit on the surface (spawn_pos.y is camera height = surface + 4) a few
    // blocks in front of the player. Eventually mobs come from the ambient
    // spawner / spawn eggs; for now they're placed so the render path can be seen.
    let mut entity_store = unendlich_engine::world::entity::EntityStore::new();
    {
        use unendlich_engine::world::entity::EntityKind;
        let ground_y = spawn_pos.y - 4.0;
        for (dx, dz) in [(-2.0, -3.0), (2.0, -3.5), (0.0, -5.0)] {
            entity_store.spawn(EntityKind::Critter, [spawn_pos.x + dx, ground_y, spawn_pos.z + dz]);
        }
        log::info!("Spawned {} test mob(s) near {:?}", entity_store.len(), spawn_pos);
    }
    // Demo creatures removed (operator request): the oversized test Dragon and the
    // three Späher that fed the Sentinel flock were spawn-area scaffolding. Real
    // creatures now come solely from the streamed ambient ecosystem (SpawnStreamer)
    // below. Kept as an empty vec so the loop that renders/updates it is unchanged.
    let mut test_creatures: Vec<unendlich_engine::creature::Creature> = Vec::new();
    log::info!("Test creatures disabled (was demo Dragon + Späher)");

    // Ambient ecosystem, STREAMED around the player. The SpawnStreamer tracks
    // which chunk columns are populated; as the player roams, newly-entered chunks
    // are populated (a few per crossing) and chunks left behind despawn — EXCEPT
    // dragons, which persist (big, impactful: popping them out of existence is
    // jarring). Re-entering a region re-rolls the identical deterministic spawns,
    // so there's no double-spawning. Each roamer is (creature, wander, home_chunk,
    // persistent). Populated lazily in the main loop (first crossing fills around
    // spawn); ROAMER_CAP bounds the per-frame entity bake.
    let mut spawn_streamer = unendlich_engine::world::spawn_streaming::SpawnStreamer::new(3);
    let mut roamers: Vec<Roamer> = Vec::new();
    let mut last_stream_chunk: Option<(i32, i32)> = None;
    // Player position last frame, for estimating how fast (and thus how loudly)
    // the player is moving — what creatures hear.
    let mut last_player_xz: Option<(f32, f32)> = None;
    let mut roamer_tick: u32 = 0;
    const ROAMER_CAP: usize = 96;
    const STREAM_RARITY: u32 = 200;


    // Sentinel flock: each Späher watches for the player and reveals them when it
    // sees or hears them. NOTE: perception runs against open air for now (clear
    // line of sight); terrain occlusion arrives when the live chunk world
    // implements WorldQuery.
    // Sentinel flock removed (operator request): this was the "[Alarmed, Calm,
    // Alarmed]" test flock of static Späher. Kept as an empty Flock so the per-frame
    // tick + posture readout below stay no-ops (postures readout prints []) rather
    // than being deleted — re-enable by adding SentinelAgents here.
    let mut sentinel_flock = unendlich_engine::sim::cognition::flock::Flock::new();
    log::info!("Sentinel flock disabled (test scaffolding removed)");
    // A predator + prey mob (sim Mob -> render bridge) so the ambient mob path is
    // exercised live: Seeker renders reddish + bulky, Grazer small + tan.
    let test_mobs: Vec<unendlich_engine::sim::mob::Mob> = {
        use unendlich_engine::sim::fixed::{Fixed, Vec3};
        use unendlich_engine::sim::mob::{Mob, MobKind};
        let gy = Fixed::from_int((spawn_pos.y - 4.0) as i64 + 1);
        let at = |x: f32, z: f32| Vec3::new(Fixed::from_int(x as i64), gy, Fixed::from_int(z as i64));
        let mut seeker = Mob::new_walker(at(spawn_pos.x + 4.0, spawn_pos.z - 6.0), 20);
        seeker.kind = MobKind::Seeker;
        let mut grazer = Mob::new_walker(at(spawn_pos.x - 4.0, spawn_pos.z - 6.0), 12);
        grazer.kind = MobKind::Grazer;
        vec![seeker, grazer]
    };
    log::info!("Spawned {} test mob(s) via sim bridge", test_mobs.len());

    // -----------------------------------------------------------------------
    // LocalPlayer setup — multiplayer-prep migration.
    //
    // The bin used to mutate `renderer.camera` directly. That path is
    // a single-local-player assumption baked into the loop. By moving
    // the per-player state into a LocalPlayer, the loop is now shaped
    // to support multiple local players (split-screen) without
    // architectural changes — just iterate over a Vec<LocalPlayer>.
    //
    // For now there's exactly one entry; the renderer's draw uses
    // `draw_frame_with_view` with this player's view. The legacy
    // `renderer.camera` field still exists (for compatibility with
    // other bins like unendlich-spin) but the world bin doesn't
    // touch it.
    // -----------------------------------------------------------------------
    let mut local_player = unendlich_engine::local_player::LocalPlayer::at_position(
        unendlich_engine::physics::BodyId::LOCAL_PLAYER,
        spawn_pos,
    );
    local_player.controller.move_speed = 32.0; // match the legacy default
    local_player.view.far = 1024.0;

    // -----------------------------------------------------------------------
    // Heat source map — dynamic point heat sources (fires, lava, ice).
    // Empty at start; player presses F9 to plant a fire at their feet.
    // The map is read by the wind arrow's vertical-convection logic so
    // updrafts visibly form over fires (and downdrafts over ice if any
    // are placed).
    //
    // Future: lava blocks at worldgen seed permanent sources; player
    // structures (furnaces, campfires) seed temporary ones with
    // intensity tied to fuel. For now F9 is a manual demo.
    // -----------------------------------------------------------------------
    let heat_sources = unendlich_engine::heat_source::HeatSourceMap::new();

    // -----------------------------------------------------------------------
    // Async meshing pool — moves CPU mesh work off the render thread.
    // -----------------------------------------------------------------------
    // F3: LodSettings is read from the env var if set, else default
    // (Near=1, Mid=2, Far=4, VeryFar=8). Wrapped in Arc so workers
    // share it without cloning the table per request.
    let lod_settings = Arc::new(unendlich_engine::world::lod::LodSettings::from_env());
    log::info!("LodSettings: factors per tier = {:?}", lod_settings.as_array());

    // v0.24.21-gpu-mesher-prod: opt-in GPU meshing for Near LOD.
    //
    //   UNENDLICH_GPU_MESHER=1  → construct SparseGpuMesher, route
    //                              Near-LOD chunks through the GPU
    //                              dispatcher. Mid/Far/VeryFar stay
    //                              on CPU.
    //   unset or 0              → CPU-only meshing (pre-v0.24.21
    //                              behaviour). Default.
    //
    // DROP ORDERING NOTE: Rust drops local variables in REVERSE order
    // of declaration. We declare `gpu_mesher_for_cleanup` FIRST and
    // `mesh_pool` SECOND so that on scope exit:
    //   1. mesh_pool drops first → its dispatcher thread sees the
    //      closed channel, exits, and releases its Arc clone of the
    //      mesher.
    //   2. gpu_mesher_for_cleanup drops next → the last Arc reference
    //      dies, the mesher's Drop runs and releases Vulkan
    //      resources WHILE the ctx (declared even earlier, dropped
    //      last) is still alive.
    // v0.24.27-octree-mesher: UNENDLICH_GPU_MESHER values:
    //   0 (or unset)  → CPU-only mesher
    //   1            → SparseGpuMesher (CPU-extracts SurfaceRecord, GPU emits)
    //   2            → OctreeGpuMesher (GPU reads octree directly)
    //
    // Both Arcs declared, at most one populated. Drop order keeps
    // the chosen one alive past mesh_pool's drop so the dispatcher
    // exits cleanly.
    let gpu_mesher_mode: u32 = std::env::var("UNENDLICH_GPU_MESHER")
        .ok().and_then(|v| v.parse::<u32>().ok()).unwrap_or(2); // R27: default octree GPU mesher (=0 CPU fallback; failures degrade to CPU)

    // Surface a known-bad setting at startup rather than letting the player
    // silently run the slow, rough experimental mesher.
    if let Some(advice) = unendlich_engine::runtime_flags::gpu_mesher_advisory(gpu_mesher_mode) {
        log::warn!("{advice}");
    }

    // v0.24.37-mesh-cmp: gated runtime falsification gate. When set,
    // every successfully-meshed Near LOD chunk on the GPU also gets
    // CPU-reference-meshed (using the same cross-chunk neighbor data
    // the GPU sees via the pool), and the two results are compared
    // by vertex count and AABB. Mismatches log a warning with chunk
    // coord and counts so the user can localize bugs across the
    // v0.24.29+ ship stack.
    //
    // Cost when enabled: one CPU-reference mesh per Near submission
    // (~100 µs at 32³ × ~30 submissions/sec ≈ 3 ms/sec on the main
    // thread). Free when disabled. Opt-in via env.
    let octree_mesh_gpu_cmp: bool = std::env::var("UNENDLICH_OCTREE_MESH_GPU_CMP")
        .ok().map(|v| v == "1" || v.eq_ignore_ascii_case("true")).unwrap_or(false);
    if octree_mesh_gpu_cmp {
        log::info!("v0.24.37-mesh-cmp: UNENDLICH_OCTREE_MESH_GPU_CMP=1 — \
                    enabling GPU↔CPU mesh equivalence gate (mode 2 only). \
                    Expect ~3 ms/sec on main thread for verification work.");
    }

    // Declared first → dropped LAST.
    let _gpu_mesher_for_cleanup: Option<Arc<unendlich_engine::render::gpu_meshing_sparse::SparseGpuMesher>> =
        if gpu_mesher_mode == 1 {
            log::info!("UNENDLICH_GPU_MESHER=1: constructing SparseGpuMesher");
            match unendlich_engine::render::gpu_meshing_sparse::SparseGpuMesher::new(&ctx) {
                Ok(mesher) => Some(Arc::new(mesher)),
                Err(e) => {
                    log::error!("SparseGpuMesher::new failed: {}. Falling back to CPU mesher.", e);
                    None
                }
            }
        } else { None };

    let _octree_gpu_mesher_for_cleanup: Option<Arc<unendlich_engine::render::gpu_meshing_octree::OctreeGpuMesher>> =
        if gpu_mesher_mode == 2 {
            log::info!("UNENDLICH_GPU_MESHER=2: constructing OctreeGpuMesher with shared octree pool");
            // v0.24.30-shared-octree: mesher reads octree data from
            // the renderer's `OctreeBufferPool` instead of its own
            // internal buffer. Construction order matters — the
            // renderer (line ~386) created the pool; we hand the
            // mesher an Arc clone here.
            match unendlich_engine::render::gpu_meshing_octree::OctreeGpuMesher::new(
                &ctx,
                renderer.octree_pool_handle(),
            ) {
                Ok(mesher) => Some(Arc::new(mesher)),
                Err(e) => {
                    log::error!("OctreeGpuMesher::new failed: {}. Falling back to CPU mesher.", e);
                    None
                }
            }
        } else { None };

    // Declared second → dropped FIRST.
    let mesh_pool = if let Some(arc) = &_octree_gpu_mesher_for_cleanup {
        let pool = MeshingPool::with_octree_gpu_mesher(
            MESH_WORKERS,
            MESH_QUEUE_CAPACITY,
            Arc::clone(&lod_settings),
            Arc::clone(arc),
        );
        log::info!(
            "MeshingPool ready ({} workers + Octree-GPU dispatcher, capacity {})",
            MESH_WORKERS, MESH_QUEUE_CAPACITY
        );
        pool
    } else if let Some(arc) = &_gpu_mesher_for_cleanup {
        let pool = MeshingPool::with_gpu_mesher(
            MESH_WORKERS,
            MESH_QUEUE_CAPACITY,
            Arc::clone(&lod_settings),
            Arc::clone(arc),
        );
        log::info!(
            "MeshingPool ready ({} workers + Sparse-GPU dispatcher, capacity {})",
            MESH_WORKERS, MESH_QUEUE_CAPACITY
        );
        pool
    } else {
        let pool = MeshingPool::new(MESH_WORKERS, MESH_QUEUE_CAPACITY, Arc::clone(&lod_settings));
        log::info!("MeshingPool ready ({} workers, capacity {}; CPU-only; \
                    UNENDLICH_GPU_MESHER=1 for sparse-records GPU mesher, \
                    =2 for octree GPU mesher)",
            MESH_WORKERS, MESH_QUEUE_CAPACITY);
        pool
    };

    // Tracks coords for which a mesh request is in flight (submitted to the
    // pool but result not yet consumed). Prevents double-submission.
    let mut meshing_in_flight: HashSet<ChunkCoord> = HashSet::new();

    // -----------------------------------------------------------------------
    // World time / sun.
    //
    // The sky drives per-face directional shading in the mesher. Tick once
    // per frame with real dt; the `time_scale` field on `Sky` lets us speed
    // up or slow down the day/night cycle (default 1× = 10-min real days).
    // -----------------------------------------------------------------------
    let mut sky_state = unendlich_engine::sky::Sky::default();

    // Weather layer (opt-in via UNENDLICH_WEATHER=1). When enabled it modulates
    // the atmosphere uniforms below; when disabled the accumulator still ticks
    // (harmless, unused) so toggling mid-development stays deterministic.
    let weather_enabled = std::env::var("UNENDLICH_WEATHER")
        .ok()
        .map(|v| v == "1")
        .unwrap_or(false);
    let weather_seed = std::env::var("UNENDLICH_WEATHER_SEED")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0x5eed_1234_abcd_0001);
    let mut weather_state = unendlich_engine::weather::Weather::new(weather_seed);

    // Load-adaptive fog (opt-in via UNENDLICH_LOAD_FOG=1): hide the unloaded
    // render-distance edge in haze while chunks stream in.
    let load_fog_enabled = std::env::var("UNENDLICH_LOAD_FOG")
        .ok()
        .map(|v| v == "1")
        .unwrap_or(false);

    // Diagnostic (UNENDLICH_NO_FOG=1): force fog_density to 0 so distant
    // geometry renders at full contrast instead of fading into the sky-
    // colored haze. Lets us tell a *missing* face apart from one that's
    // merely fogged to the background color. Default off.
    let no_fog_enabled = std::env::var("UNENDLICH_NO_FOG")
        .ok()
        .map(|v| v == "1")
        .unwrap_or(false);
    // r642: fog extinction at the player column for creature vision (same base +
    // humidity the shader fogs with); refreshed periodically below. 0 = clear.
    let mut frame_fog_sigma: f32 = 0.0;

    // Observation flag: force every spawned roamer to a picker ARCHETYPE (the body
    // plans the creature picker shows, e.g. the super-tall builds), bridged into a
    // world creature. Value is an archetype label, e.g.
    // UNENDLICH_SPAWN_ARCHE=dragon | dinosaur | heavy | biped | serpent | ...
    let spawn_arche = std::env::var("UNENDLICH_SPAWN_ARCHE")
        .ok()
        .and_then(|s| unendlich_engine::creature_design::Arche::from_label(s.trim()));
    if let Some(a) = spawn_arche {
        log::warn!("UNENDLICH_SPAWN_ARCHE={:?} — all spawns forced to that picker archetype (observation mode)", a);
    }

    // -----------------------------------------------------------------------
    // Input + keybindings
    // -----------------------------------------------------------------------
    let bindings = KeyBindings::qwerty_preset();
    let mut input = InputCache::default();
    let mut mouse_captured = false;
    // v0.24.NEXT-input-p-toggle: press `p` to enter "panel mode" — cursor is
    // released, click-to-recapture is suppressed, and pointer events route to
    // egui so the creative inventory / settings panels can be clicked. Press
    // `p` again (or Esc) to exit. `ui_mouse_down` is the live left-button
    // state we forward to the renderer; tracked from MouseButton events.
    let mut ui_panel_open = false;
    let mut ui_mouse_down = false;
    let mut p_pressed = false;

    // Rollback input buffer — captures one InputFrame per simulation
    // tick boundary. Today this is passive infrastructure: simulation
    // logic still reads directly from `input` and `bindings`. Once the
    // network layer lands, the buffer becomes the read source so we
    // can replay inputs forward after a server correction.
    //
    // Aggregation: render frames typically happen 100x faster than
    // simulation ticks, so we accumulate held-actions, just-pressed
    // edges, and mouse deltas across all frames within one tick, then
    // emit one InputFrame at the next tick boundary.
    use unendlich_engine::input_capture::{ActionBits, InputBuffer, InputFrame};
    let mut input_buffer = InputBuffer::new();
    // Accumulator for the in-progress (not-yet-finalized) tick frame.
    // Holds the union of held actions, OR of just-pressed flags, and
    // sum of mouse deltas across all render frames since the last tick
    // boundary.
    let mut pending_frame = InputFrame::new(unendlich_engine::world::ticks::TickId::ZERO);
    // Track tick number from last boundary so we know when to flush.
    let mut last_captured_tick = unendlich_engine::world::ticks::TickId::ZERO;

    // Debug overlay scene — populated each frame from current sim
    // state. Renders markers/arrows/AABBs in world space showing
    // engine adaptations: lifecycle stages, wind, physics velocity,
    // tick-cull boundary. Toggle on/off via F4. The scene is purely
    // visual — never sync'd, never snapshotted, never affects sim.
    let mut debug_scene = unendlich_engine::debug_overlay::DebugScene::new();

    // -----------------------------------------------------------------------
    // Player movement mode.
    //
    // `body = None` means fly mode (legacy free camera, no physics).
    // `body = Some(PhysicsBody)` means physics mode (gravity, jump, collide).
    //
    // Toggle with `GameAction::ToggleFlight`. The camera position is the
    // same in both modes — in fly mode the camera moves directly; in physics
    // mode the body moves and the camera follows the body (with eye offset).
    //
    // Default: fly mode. Player has to press toggle to start physics. This
    // keeps the spawn-into-unloaded-chunks experience tolerable — physics
    // mode would have you fall through ungenerated terrain.
    // -----------------------------------------------------------------------
    let mut body: Option<PhysicsBody> = None;
    let mut last_block_action = Instant::now();
    /// Cooldown between place/break actions, to avoid breaking 60 blocks per
    /// second when the user holds the mouse button.
    const BLOCK_ACTION_COOLDOWN_MS: u128 = 200;
    // Block-placement palette: hotbar keys 1..9 select which BlockId right-click
    // places (was hardcoded to Stone). Index with the Hotbar1..Hotbar9 actions.
    const PLACE_PALETTE: [BlockId; 9] = [
        BlockId::Stone,
        BlockId::Dirt,
        BlockId::Cobblestone,
        BlockId::OakLog,
        BlockId::OakLeaves,
        BlockId::Sand,
        BlockId::Glass,
        BlockId::OakPlanks,
        BlockId::Water,
    ];
    let mut selected_block: BlockId = PLACE_PALETTE[0];

    // -----------------------------------------------------------------------
    // Main loop
    // -----------------------------------------------------------------------
    let start = Instant::now();
    // v0.24.2c.4: `UNENDLICH_MAX_DURATION_MS` caps wall-clock runtime
    // for chained `cfg1 ; cfg2 ; cfg3` shell invocations. We compute
    // the deadline once at the top of main rather than repeatedly
    // checking the env var; this also means the cap is recorded in
    // the flags summary block in run_summaries.txt.
    let max_duration: Option<std::time::Duration> = std::env::var("UNENDLICH_MAX_DURATION_MS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .map(std::time::Duration::from_millis);
    if let Some(d) = max_duration {
        log::info!("Max duration cap: {}ms — will auto-shutdown at deadline", d.as_millis());
    }

    // v0.24.2c.7: per-frame upload cap and budget are runtime-tunable
    // via env vars (matching the pattern already used in
    // `unendlich_spin.rs`). Both default to the values used pre-c.7.
    //
    // `UNENDLICH_MAX_UPLOADS_PER_FRAME=N` — hard cap on chunks uploaded
    //   per frame regardless of available time. Lower values reduce
    //   the per-frame work that gets dumped onto the GPU at once,
    //   slowing world fill but smoothing frame pacing.
    //
    // `UNENDLICH_UPLOAD_BUDGET_MS=F` — soft cap on CPU time spent
    //   uploading per frame. Whichever cap trips first wins.
    //
    // Both are diagnostic knobs introduced after the c.5 bisect
    // observed that shadow GPU cost scales with the number of
    // `rendering:` chunks. Slowing the upload rate is a way to keep
    // `rendering:` count lower at the cost of slower world buildout —
    // useful to test whether shadow cost is in fact upload-rate
    // dominated.
    let max_uploads_per_frame: usize = std::env::var("UNENDLICH_MAX_UPLOADS_PER_FRAME")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(MAX_UPLOADS_PER_FRAME_DEFAULT);
    let upload_budget_ms: f64 = std::env::var("UNENDLICH_UPLOAD_BUDGET_MS")
        .ok()
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|&v| v > 0.0)
        .unwrap_or(UPLOAD_BUDGET_MS_DEFAULT);
    // v0.24.3 upload-pressure investigation: bytes-budget alongside
    // ms-budget. Whichever fires first wins. Setting either to a very
    // large value effectively disables it.
    let upload_bytes_budget: usize = std::env::var("UNENDLICH_UPLOAD_BYTES_BUDGET")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|&v| v > 0)
        .unwrap_or(UPLOAD_BYTES_BUDGET_DEFAULT);
    log::info!(
        "Upload knobs: MAX_UPLOADS_PER_FRAME={} BUDGET_MS={:.2} BYTES_BUDGET={} (defaults {},{:.2},{})",
        max_uploads_per_frame, upload_budget_ms, upload_bytes_budget,
        MAX_UPLOADS_PER_FRAME_DEFAULT, UPLOAD_BUDGET_MS_DEFAULT, UPLOAD_BYTES_BUDGET_DEFAULT,
    );

    // v0.24.63-adaptive-uploads: optional AIMD-style throttle that
    // backs off uploads when wait_fences EMA goes high, ramps them
    // up again when it drops. Default OFF — set
    // `UNENDLICH_ADAPTIVE_UPLOADS=1` to enable.
    //
    // When enabled, `max_uploads_per_frame` (above) is used as the
    // INITIAL cap, and the controller adjusts within
    // [base/8, base*2]. The configured floor of base/8 (= 8 at
    // default base 64) ensures world streaming never fully stalls
    // even under sustained pressure.
    //
    // Designed to be safe-by-default: opt-in only, and the controller
    // can only shift the cap within bounded ranges. No effect on
    // GPU queue ordering or descriptor sets — pure CPU-side knob.
    let mut adaptive_uploads: Option<unendlich_engine::render::adaptive_upload::AdaptiveUploadController> =
        if std::env::var("UNENDLICH_ADAPTIVE_UPLOADS").ok().as_deref() != Some("0") { // R27: default ON
            log::info!(
                "adaptive_uploads: ENABLED — initial cap {}, range [{}, {}]",
                max_uploads_per_frame,
                (max_uploads_per_frame / 8).max(4),
                max_uploads_per_frame * 2,
            );
            Some(unendlich_engine::render::adaptive_upload::AdaptiveUploadController::new(
                max_uploads_per_frame,
            ))
        } else {
            None
        };

    // v0.24.64-submit-skip: opt-in optimization for steady-state CPU
    // load. The submit phase below walks every chunk in render-distance
    // each frame (~(2*RD+1)^3 = 2197 at RD=6) doing DashMap reads to
    // discover whether anything needs (re-)meshing. In steady state
    // with the world fully loaded and stable, the walk finds zero
    // candidates but still burns ~800ms/sec on the user's M2 Max.
    //
    // The optimization: cache a "world is stationary" signal derived
    // from three cheap snapshots (no DashMap iteration):
    //   1) world_edit_tick (atomic counter, bumped on every block edit)
    //   2) loaded_count (atomic chunk-count, changes on load/evict)
    //   3) player_chunk (changes on player movement to a new chunk)
    // If all three are unchanged from last frame AND last frame's
    // walk found zero submittable candidates, skip the walk entirely.
    // Any change in any signal forces a re-walk next frame — the
    // condition is a strict superset of "world has visible changes",
    // so it's safe (we never wrongly skip work, only correctly skip
    // pure no-ops).
    //
    // Safety net: env-gated (default OFF). To enable:
    //   UNENDLICH_SUBMIT_SKIP=1
    // Then A/B test: same workload, with and without the flag.
    // Expected: cpu.submit drops from ~800ms/sec to ~5ms/sec when
    // standing still in a loaded world; cpu.submit unchanged when
    // moving (signals change each frame, walk always runs).
    let submit_skip_enabled = std::env::var("UNENDLICH_SUBMIT_SKIP")
        .ok().as_deref() != Some("0"); // R27: default ON
    if submit_skip_enabled {
        log::info!(
            "submit_skip: ENABLED — shell walk skipped when world is stationary"
        );
    }
    let mut submit_skip_last_world_edit_tick: u64 = u64::MAX;
    let mut submit_skip_last_loaded_count: usize = usize::MAX;
    let mut submit_skip_last_player_chunk: ChunkCoord =
        ChunkCoord::new(i32::MIN, i32::MIN, i32::MIN);
    let mut submit_skip_was_idle: bool = false;
    let mut submit_skip_frames_skipped: u64 = 0;
    let mut submit_skip_last_log_frames: u64 = 0;

    // v0.24.65-slack-scheduler: opt-in slack-based upload pacing.
    //
    // Background: the current upload phase enforces a FIXED per-frame
    // time budget (UNENDLICH_UPLOAD_BUDGET_MS, default 8ms). That
    // works as a worst-case cap but doesn't adapt to the frame's
    // actual progress — if other phases ran fast and we have plenty
    // of time left in the frame, we leave that time on the table;
    // if other phases ran slow and we're already over budget, we
    // still spend the full 8ms uploading and stutter even more.
    //
    // The slack scheduler instead asks: "how much time is LEFT in
    // this frame's target?" and uploads until that's exhausted.
    // Latency: zero added — uploads still happen this frame or
    // next, no fixed queue depth. This is the user-flagged
    // "scheduler, not a frame-time queue" approach (Stage 1 from
    // docs/speculative/upload-scheduler-design.md).
    //
    // Knobs:
    //   UNENDLICH_SLACK_SCHEDULER=1       — enable (default off)
    //   UNENDLICH_FRAME_TARGET_MS=8.33    — target frame budget (default 8.33ms = 120 FPS)
    //   UNENDLICH_UPLOAD_MIN_MS=1.0       — minimum upload time even when frame is over budget
    //                                       (prevents pathological starvation; default 1ms)
    //
    // Effective budget per frame:
    //   slack_ms = max(0, frame_target_ms - elapsed_this_frame)
    //   effective_ms = max(upload_min_ms, min(upload_budget_ms, slack_ms))
    //
    // So when the frame is running fast, the existing
    // upload_budget_ms cap dominates (preserves prior behavior).
    // When the frame is running slow, the slack cap dominates
    // (shrinks the upload window to fit). When the frame is
    // already over budget, upload_min_ms preserves forward
    // progress (chunks still land, just slowly).
    let slack_scheduler_enabled = std::env::var("UNENDLICH_SLACK_SCHEDULER")
        .ok().as_deref() != Some("0"); // R27: default ON
    let frame_target_ms: f64 = std::env::var("UNENDLICH_FRAME_TARGET_MS")
        .ok().and_then(|s| s.parse::<f64>().ok())
        .filter(|&v: &f64| v > 0.0)
        .unwrap_or(8.33);
    let upload_min_ms: f64 = std::env::var("UNENDLICH_UPLOAD_MIN_MS")
        .ok().and_then(|s| s.parse::<f64>().ok())
        .filter(|&v: &f64| v > 0.0)
        .unwrap_or(1.0);
    if slack_scheduler_enabled {
        log::info!(
            "slack_scheduler: ENABLED — frame_target_ms={:.2}, upload_min_ms={:.2}, upload_budget_ms (cap)={:.2}",
            frame_target_ms, upload_min_ms, upload_budget_ms,
        );
    }

    // v0.24.66-biased-walk: opt-in forward-biased shell walk.
    //
    // Stage 1 Option D from the upload-scheduler design memo. The
    // existing shell walk visits coords in face-round-robin order
    // (ShellWalker), which is direction-fair. That's correct for
    // average behavior, but it means the chunks the player is
    // actively *looking at* compete for budget slots equally with
    // chunks behind their head.
    //
    // The biased walk re-orders each shell so coords with high
    // forward-dot are processed first within the shell. The
    // per-tier budgets still apply, but they fill with
    // ahead-of-player coords first. Chunks behind get the leftover
    // slots (or wait for the next frame).
    //
    // Latency: zero added. Each chunk that WOULD have been emitted
    // is still emitted; only the order within a shell changes.
    //
    // Knobs:
    //   UNENDLICH_BIASED_SHELL_WALK=1   — enable (default off)
    //
    // Pairs well with the slack scheduler (RC31): together, the
    // chunks the player is looking at land first AND the upload
    // budget tightens when the frame's running long. Both can be
    // A/B-tested independently.
    let biased_shell_walk_enabled = std::env::var("UNENDLICH_BIASED_SHELL_WALK")
        .ok().as_deref() != Some("0"); // R27: default ON
    if biased_shell_walk_enabled {
        log::info!("biased_shell_walk: ENABLED — chunks ahead of player processed first within each shell");
    }
    // v0.24.67-upload-priority: opt-in priority queue over the
    // drained mesh-pool results, ahead of the upload loop. Stage 2
    // from the upload-scheduler design memo.
    //
    // The mesh pool's worker output is FIFO from the shell walk's
    // submission order. With RC30 (biased shell walk), that's
    // already biased toward ahead-of-player Near chunks. But by
    // the time results land in the pool's output channel, the
    // FIFO order can include a Far-LOD chunk for shell 12 ahead
    // of a Near chunk for shell 2 (different worker thread
    // finished first). Sorting at the upload boundary corrects
    // for that — every upload window picks the closest, most
    // visible chunks first.
    //
    // Sort key:
    //   - Primary: LOD (Near=0, Mid=1, Far=2, VeryFar=3)
    //   - Secondary: Chebyshev distance from player_chunk
    //
    // Knobs:
    //   UNENDLICH_UPLOAD_PRIORITY=1   — enable (default off)
    //
    // Cost: O(N log N) sort over the drained Vec. With
    // upload_cap=64 (default), that's ~6 × 64 ≈ 384 comparisons
    // per frame. ~1µs. Negligible.
    let upload_priority_enabled = std::env::var("UNENDLICH_UPLOAD_PRIORITY")
        .ok().as_deref() != Some("0"); // R27: default ON
    if upload_priority_enabled {
        log::info!("upload_priority: ENABLED — upload queue sorted by (LOD, distance) each frame");
    }

    // v0.25-prefetch (T05-R27): when prefetch is on, route prefetched-but-not-
    // yet-visible chunks through the renderer's deferrable upload lane (and, in
    // GPU-mesher mode 2, skip resident bytes for chunks `upload_repr::classify`
    // marks Procedural — the GPU regenerates those from seed). Visible chunks
    // keep the immediate `write_at` path. Shares the umbrella `UNENDLICH_PREFETCH`
    // flag with chunk_manager's load-ahead, so behavior is unchanged when unset.
    let prefetch_upload_enabled = std::env::var("UNENDLICH_PREFETCH")
        .ok().as_deref() == Some("1");
    if prefetch_upload_enabled {
        log::info!("prefetch_upload: ENABLED — ahead-meshed (not-yet-visible) chunks use the deferrable upload lane (+classify skip in mode 2)");
    }

    let mut last_frame = Instant::now();
    let mut frame_count = 0u64;
    // Throttle for sentinel reveal logging (frames between log lines).
    let mut last_reveal_log_frame = 0u64;
    // Player's last voxel cell, for movement-based loudness (still = quiet, so you
    // can sneak; moving = footsteps a sentinel can hear).
    let mut last_player_cell: Option<[i32; 3]> = None;
    let mut last_log = Instant::now();
    // Per-frame timing: collected for the FPS log window. Reset each second.
    let mut frame_times_ms: Vec<f64> = Vec::with_capacity(2048);

    // c.12-e: boot mode — emit per-frame frame_window events for the first
    // few seconds, then switch to 1Hz aggregated mode. Startup is the most
    // jank-prone phase (chunk gen, first paint, mouse capture) and
    // aggregating it into a single 1-second window hides the actual shape
    // of the warmup curve. Per-frame events are tagged `mode: "boot"` so
    // the analyzer can distinguish them from steady-state events.
    //
    // Cost: ~60 frames/sec × 3 sec × ~500 bytes = ~90KB. Negligible.
    let boot_until = Instant::now() + std::time::Duration::from_secs(3);
    let process_start = Instant::now();

    // c.13-instrumentation: warmup window. The engine runs for
    // warmup_ms BEFORE the assertion-relevant samples begin accumulating.
    // At the warmup→steady transition, the GPU and CPU timer LIFETIME
    // accumulators reset, so assertion stats reflect only the steady-state
    // measurement window. The boot-mode per-frame events still apply
    // during warmup (they're orthogonal — boot mode is the first 3s
    // regardless; warmup is the first N ms before measurement begins).
    //
    // For specs that set warmup_ms > 0, the workflow is:
    //   t=0:           run starts
    //   t<warmup_ms:   warmup phase (still rendering, still meshing,
    //                  still building shadow caches), samples discarded
    //   t=warmup_ms:   reset timer accumulators, emit a
    //                  warmup_complete event
    //   t>warmup_ms:   steady-state measurement begins
    //   t=duration:    shutdown
    let warmup_ms: u64 = std::env::var("UNENDLICH_WARMUP_MS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);
    let warmup_until: Option<Instant> = if warmup_ms > 0 {
        log::info!("Warmup window: {}ms — timer stats will reset at warmup end", warmup_ms);
        Some(process_start + std::time::Duration::from_millis(warmup_ms))
    } else {
        None
    };
    let mut warmup_completed = warmup_until.is_none();

    // c.13-instrumentation: workload snapshots for cross-referencing
    // perf data with world-state size. Captured twice:
    //   1. `start_snapshot`: captured at the warmup→steady transition
    //      (or at the boot→steady transition if no warmup). Reflects
    //      world state at the moment steady-state measurement begins.
    //   2. `end_snapshot`: captured at shutdown. Together with start,
    //      the diff tells us how much the world grew during the
    //      measurement window.
    //
    // Both are (loaded_chunks, non_air_blocks) tuples. Cheap (~ms).
    // Future fields (plant_count, creature_count, etc.) can be added
    // opportunistically when working in those subsystems.
    let mut start_snapshot: Option<(usize, u64)> = None;
    // v0.24.2c.7: workload-shape samples taken once per FPS-log window
    // (each second). These are summarised at shutdown so the run-summary
    // block records "what the world looked like during this run" — useful
    // for distinguishing perf differences caused by workload (more loaded
    // chunks = more rendering work) from perf differences caused by code
    // changes. Sampled at the same cadence as the FPS log, not per-frame,
    // to keep the cost negligible.
    let mut loaded_samples: Vec<u64> = Vec::with_capacity(256);
    let mut rendering_samples: Vec<u64> = Vec::with_capacity(256);
    // v0.24.NEXT+12-tick-telemetry: sample the per-tick chunk-apply time so
    // the loading-wave CPU hitch (observed apply=107ms for 489 chunks) shows
    // up in the run summary + dashboard, not just the periodic console line.
    let mut tick_apply_us_samples: Vec<u64> = Vec::with_capacity(256);
    let mut tick_apply_chunks_samples: Vec<u64> = Vec::with_capacity(256);
    let mut last_player_chunk = ChunkCoord::new(i32::MIN, i32::MIN, i32::MIN);
    // Event-arrival diagnostics. If the user reports "keys stop responding"
    // but FPS stays high, the most likely cause is GLFW events not being
    // delivered to the window — typically a focus issue or window-manager
    // state. These counters log per-second:
    //   - events_received: total WindowEvents drained from the queue.
    //   - key_events: subset that were keyboard events.
    //   - last_focused: whether the window had focus this frame (sampled
    //     once per FPS-log window).
    //
    // A freeze with FPS=120 and events_received=0 means GLFW stopped
    // delivering events. The window is alive (we can draw) but input is
    // dead. That's a focus/capture issue.
    let mut events_received: u64 = 0;
    let mut key_events: u64 = 0;
    let mut mouse_events: u64 = 0;
    let mut cursor_events: u64 = 0;

    // v0.21.1: log compression. We suppress the input+climate line
    // when nothing has changed across two consecutive log windows.
    // Idle play sessions (player AFK or watching the world tick) emit
    // hundreds of identical lines per minute otherwise — tokens for
    // Claude to parse, screen real estate for the user. Heartbeat
    // info still goes on the FPS line; only the redundant rows go.
    let mut last_log_temp: f32 = -999.0;
    let mut last_log_humid: f32 = -999.0;
    let mut last_log_focus: bool = false;
    let mut last_log_capture: bool = false;

    // ---- Per-section timing accumulators (reset each FPS-log window) ----
    // Captured to track WHERE per-frame time is going during movement.
    // The user reported "freezes after moving" — these accumulators
    // pinpoint which streaming-loop section is blocking. All in
    // microseconds; converted to ms in the log line.
    let mut t_cloud_remesh_us: u64 = 0;
    let mut t_eviction_us: u64 = 0;
    let mut t_submit_us: u64 = 0;
    let mut t_upload_us: u64 = 0;
    // The chunk-manager update path: drains completed chunks from the
    // loader, applies persisted edits, inserts into the chunks DashMap.
    // This runs on the main thread and BLOCKS the frame; if many chunks
    // complete simultaneously, this can spike. Captured separately
    // because it's the most likely source of perceived "freeze" hitches
    // on movement.
    let mut t_manager_update_us: u64 = 0;
    let mut t_tick_sim_us: u64 = 0;
    // Render thread (vkQueueSubmit + vkAcquireNextImage + vkQueuePresent).
    // Counts wall-clock time inside `draw_frame_with_view` — including
    // GPU sync waits. If the user perceives a freeze but submit/upload
    // metrics look healthy, the time is most likely here (driver-level
    // sync, FIFO swap chain wait, GPU pipeline stall on resource access).
    let mut t_draw_us: u64 = 0;

    // Cloud animation cadence: every N seconds, bump the version of
    // any loaded chunk that intersects the cloud band. The streaming
    // loop's version-aware filter then resubmits those chunks for
    // remeshing with the latest time-shifted cloud noise field.
    // The result: visible cloud DRIFT across the sky in the wind
    // direction. Cadence is a tradeoff: faster = smoother but more
    // CPU; slower = chunky/jumpy.
    let mut last_cloud_remesh = Instant::now();
    const CLOUD_REMESH_INTERVAL: f32 = 2.0;
    // Lava heat-source sync state: tracks which chunks have already been
    // scanned for surface lava (so we don't re-scan every frame). When a
    // chunk is no longer in the manager, its heat sources get torn down
    // by chunk-bbox removal. New chunks get scanned on the next sync tick.
    //
    // Periodic instead of per-frame: scanning a chunk for lava is O(32K)
    // cells, which would be prohibitive every frame even with the
    // "already-registered" set check. Once per second is plenty —
    // delayed by up to 1s before lava starts driving convection, but
    // visually undetectable since the player can't see far underground
    // anyway when chunks are still loading.
    let mut lava_registered: std::collections::HashSet<ChunkCoord> = std::collections::HashSet::new();
    let mut last_lava_sync = Instant::now();
    const LAVA_SYNC_INTERVAL: f32 = 1.0;
    // UX: remind the user to click for mouse capture every N seconds
    // until they do so. Without this, the game LOOKS frozen at start
    // (FPS is fine, world is loading, but the view doesn't change
    // because the player's view is locked at spawn). The "Click to
    // capture mouse" log line on the first frame is easy to miss.
    let mut last_click_hint = Instant::now();
    const CLICK_HINT_INTERVAL: f32 = 5.0;
    log::info!(">>> CLICK on the window to start playing. Mouse + WASD control once captured. Esc to release.");

    // T7 (alloc churn): reusable per-tier streaming-candidate buffers, hoisted
    // out of the frame loop below to kill 4 `Vec::with_capacity` allocations per
    // frame. They hold (coord, Arc<Chunk>, version) and are fully drained on each
    // use (see the streaming block), so no Arc is pinned across frames; only the
    // backing capacity is reused.
    let mut near_cands: Vec<(ChunkCoord, std::sync::Arc<unendlich_engine::world::chunk::Chunk>, u64)> = Vec::with_capacity(MESH_QUEUE_CAPACITY);
    let mut mid_cands:  Vec<(ChunkCoord, std::sync::Arc<unendlich_engine::world::chunk::Chunk>, u64)> = Vec::with_capacity(MESH_QUEUE_CAPACITY);
    let mut far_cands:  Vec<(ChunkCoord, std::sync::Arc<unendlich_engine::world::chunk::Chunk>, u64)> = Vec::with_capacity(MESH_QUEUE_CAPACITY);
    let mut vfar_cands: Vec<(ChunkCoord, std::sync::Arc<unendlich_engine::world::chunk::Chunk>, u64)> = Vec::with_capacity(MESH_QUEUE_CAPACITY);

    while !window.should_close() {
        // v0.24.2c.4: max-duration check. Fires once when the deadline
        // is hit; the loop body proceeds for this frame (so we get a
        // clean shutdown via the normal path, including saved chunks
        // and the run-summary block) and the `while` condition exits
        // before the next iteration.
        if let Some(cap) = max_duration {
            if start.elapsed() >= cap {
                log::info!("Max duration {}ms reached — requesting shutdown", cap.as_millis());
                window.set_should_close(true);
            }
        }

        // ---- Events ----
        glfw.poll_events();
        let mut needs_resize: Option<(u32, u32)> = None;
        let mut esc_pressed = false;

        for (_, event) in glfw::flush_messages(&events) {
            events_received += 1;
            match &event {
                WindowEvent::Key(_, _, _, _) => key_events += 1,
                WindowEvent::MouseButton(_, _, _) => mouse_events += 1,
                WindowEvent::CursorPos(_, _) => cursor_events += 1,
                _ => {}
            }
            input.handle_event(&event);
            match event {
                WindowEvent::Key(Key::Escape, _, Action::Press, _) => {
                    esc_pressed = true;
                }
                WindowEvent::Key(key, scancode, Action::Press, _) => {
                    // #2 (Dvorak): toggle panel mode on the key LABELED 'p' in the
                    // active keyboard layout — not GLFW's physical `Key::P`, which
                    // is the US-QWERTY position and lands on Dvorak's 'l' key.
                    // `get_key_name(None, scancode)` resolves the layout char for
                    // the physical key; fall back to physical P if unavailable.
                    let is_p = match glfw::get_key_name(None, Some(scancode)) {
                        Some(name) => name == "p",
                        None => key == Key::P,
                    };
                    if is_p {
                        p_pressed = true;
                    }
                }
                WindowEvent::MouseButton(MouseButtonLeft, Action::Press, _) => {
                    ui_mouse_down = true;
                }
                WindowEvent::MouseButton(MouseButtonLeft, Action::Release, _) => {
                    ui_mouse_down = false;
                }
                WindowEvent::FramebufferSize(w, h) => {
                    needs_resize = Some((w as u32, h as u32));
                }
                _ => {}
            }
        }

        // Carry-aware: a late-drained Esc shows up as just-pressed next frame.
        esc_pressed |= input.is_key_just_pressed(Key::Escape);

        // P handling: toggle panel mode (cursor visible + UI clicks routed).
        if p_pressed {
            p_pressed = false;
            ui_panel_open = !ui_panel_open;
            if ui_panel_open {
                window.set_cursor_mode(CursorMode::Normal);
                mouse_captured = false;
                log::info!("Panel mode ON (P to close, Esc also closes).");
            } else {
                log::info!("Panel mode OFF. Click to recapture mouse.");
            }
        }

        // Esc handling: in panel mode -> close panel; else first press releases mouse, second quits.
        if esc_pressed {
            // Esc toggles the in-game debug/shaderpack panel. Opening frees the
            // cursor so the panel is clickable; quit is the panel's Quit button.
            renderer.settings_open = !renderer.settings_open;
            if renderer.settings_open {
                window.set_cursor_mode(CursorMode::Normal);
                mouse_captured = false;
                log::info!("Settings panel open (Esc to close).");
            } else {
                log::info!("Settings panel closed.");
            }
        }
        if unendlich_engine::debug_settings::DEBUG
            .request_quit
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            window.set_should_close(true);
        }

        // Click-to-capture mouse — suppressed in panel mode so UI clicks land.
        if !mouse_captured && !ui_panel_open && !renderer.settings_open && input.is_mouse_button_just_pressed(MouseButtonLeft) {
            window.set_cursor_mode(CursorMode::Disabled);
            mouse_captured = true;
            // Reset deltas so we don't snap on first frame.
            input.cursor_delta = (0.0, 0.0);
            log::info!("Mouse captured. Esc to release.");
        }

        // Remind the user to click if they haven't yet. Every 5 seconds
        // after game launch, while the mouse is still uncaptured, repeat
        // the hint. Prevents the "the game looks frozen" failure mode
        // where the user doesn't realize they need to click first.
        if !mouse_captured
            && last_click_hint.elapsed().as_secs_f32() > CLICK_HINT_INTERVAL
        {
            last_click_hint = Instant::now();
            log::info!(">>> Click on the window to start playing (mouse + WASD).");
        }

        // ---- Resize ----
        if let Some(size) = needs_resize {
            if size.0 == 0 || size.1 == 0 {
                std::thread::sleep(std::time::Duration::from_millis(16));
                input.end_frame();
                continue;
            }
            if let Err(e) = renderer.recreate_swapchain(&ctx, size) {
                eprintln!("Recreate swapchain failed: {}", e);
                break;
            }
        }

        // ---- Update ----
        let now = Instant::now();
        let dt = (now - last_frame).as_secs_f32().min(0.1); // cap dt at 100ms
        let dt_ms = (now - last_frame).as_secs_f64() * 1000.0;
        last_frame = now;
        frame_times_ms.push(dt_ms);

        // c.12-e: boot-mode per-frame logging.
        // For the first ~3 seconds, emit one frame_window event PER FRAME
        // (tagged `mode: "boot"`). After that, the regular 1Hz aggregation
        // path below takes over (those events are implicitly `mode: "steady"`
        // since they cover an aggregated window).
        //
        // The boot mode captures the warmup curve at frame resolution —
        // typically first paint at 200ms, then a few 130ms frames as
        // chunks generate, then settles. The aggregated view loses this
        // shape.
        if now < boot_until {
            unendlich_shared::events::record("frame_window", serde_json::json!({
                "mode": "boot",
                "frame": frame_count,
                "since_start_ms": (now - process_start).as_secs_f64() * 1000.0,
                "frame_ms": dt_ms,
            }));
        }

        // c.13-instrumentation: warmup→steady transition.
        // When warmup elapses, reset the GPU+CPU lifetime accumulators so
        // assertion stats reflect only the steady-state measurement window.
        // We also snapshot the world state here for cross-referencing
        // (start_state in the result file's `workload_snapshot`).
        if !warmup_completed {
            if let Some(deadline) = warmup_until {
                if now >= deadline {
                    let snap = chunk_manager.world_state_snapshot();
                    log::info!(
                        "Warmup complete after {}ms — resetting timer stats. \
                         World state: loaded_chunks={} non_air_blocks={}",
                        warmup_ms, snap.0, snap.1,
                    );
                    renderer.reset_timer_lifetime_stats();
                    unendlich_shared::events::record("warmup_complete", serde_json::json!({
                        "warmup_ms": warmup_ms,
                        "loaded_chunks": snap.0,
                        "non_air_blocks": snap.1,
                    }));
                    start_snapshot = Some(snap);
                    warmup_completed = true;
                }
            }
        }

        // c.13-instrumentation: boot→steady transition (no warmup).
        // If there's no warmup configured, we still want a start
        // snapshot for cross-referencing — captured once when the boot
        // window ends. This is just data capture; no accumulator reset
        // (boot samples are already excluded from triage via the
        // `mode: "boot"` tag).
        if start_snapshot.is_none() && now >= boot_until {
            let snap = chunk_manager.world_state_snapshot();
            log::info!(
                "Boot complete after 3000ms — world state: loaded_chunks={} non_air_blocks={}",
                snap.0, snap.1,
            );
            start_snapshot = Some(snap);
        }
        // Long-frame diagnostic. The user reports the canvas freezing for
        // a sustained period (seconds) starting around 1-2s after world
        // load. The 1-Hz FPS log isn't fine-grained enough to capture a
        // single-frame stall in detail. This dumps a warning the moment a
        // frame's wall time exceeds 100ms — typical frames are 8.3ms at
        // vsync, so anything > 100ms is a real hitch worth investigating.
        // The warning includes recent section timings so we can see WHICH
        // part of the frame stretched without waiting for the next 1Hz
        // log boundary.
        if dt_ms > 100.0 && frame_count > 5 {
            log::warn!(
                "LONG FRAME: dt={:.1}ms (sections may have already accumulated some of this; \
                 recent 1s totals: cloud_remesh={:.1}ms evict={:.1}ms submit={:.1}ms \
                 upload={:.1}ms mgr_update={:.1}ms tick_sim={:.1}ms draw={:.1}ms)",
                dt_ms,
                t_cloud_remesh_us as f64 / 1000.0,
                t_eviction_us as f64 / 1000.0,
                t_submit_us as f64 / 1000.0,
                t_upload_us as f64 / 1000.0,
                t_manager_update_us as f64 / 1000.0,
                t_tick_sim_us as f64 / 1000.0,
                t_draw_us as f64 / 1000.0,
            );
        }

        // Advance world time. Mesh requests submitted later this frame will
        // bake the new sun direction into their lighting.
        sky_state.tick(dt);
        weather_state.tick(dt);

        if mouse_captured {
            local_player.apply_mouse(input.cursor_delta.0, input.cursor_delta.1);
            // v0.24.2c.10: zero the delta immediately after consuming
            // so the late-poll (just before render) sees only NEW mouse
            // events. This is the input-at-bottom mechanism — late
            // movement gets folded into the camera matrix used for
            // this frame's render.
            input.cursor_delta = (0.0, 0.0);
        }

        // Toggle between fly and physics modes. Spawn the body roughly at
        // the camera position so the transition is smooth.
        if bindings.is_action_just_pressed(GameAction::ToggleFlight, &input) {
            if !local_player.permissions.can_toggle_flight {
                eprintln!("[F] Permission denied: can_toggle_flight is false");
            } else if body.is_some() {
                log::info!("Switched to FLY mode");
                body = None;
            } else {
                let p = local_player.view.position;
                // Find a clear spawn position near the camera so toggling
                // inside leaves / wall doesn't immediately stick the body.
                let safe_pos = physics::find_safe_spawn(&chunk_manager, p.x, p.y, p.z);
                let new_body = PhysicsBody::new_player(safe_pos);
                log::info!(
                    "Switched to PHYSICS mode at ({:.1}, {:.1}, {:.1}) (camera was at ({:.1}, {:.1}, {:.1}))",
                    safe_pos[0], safe_pos[1], safe_pos[2], p.x, p.y, p.z
                );
                body = Some(new_body);
                // Snap camera to the new body's eye level so the user's
                // view doesn't jump weirdly.
                local_player.view.position = glm::vec3(
                    safe_pos[0],
                    safe_pos[1] + physics::constants::PLAYER_EYE_OFFSET,
                    safe_pos[2],
                );
            }
        }

        // ---- Movement input ----
        let mut move_local = glm::vec3(0.0, 0.0, 0.0);
        if bindings.is_action_pressed(GameAction::MoveForward, &input)  { move_local.x += 1.0; }
        if bindings.is_action_pressed(GameAction::MoveBackward, &input) { move_local.x -= 1.0; }
        if bindings.is_action_pressed(GameAction::MoveRight, &input)    { move_local.z += 1.0; }
        if bindings.is_action_pressed(GameAction::MoveLeft, &input)     { move_local.z -= 1.0; }

        match body.as_mut() {
            None => {
                // Fly mode: direct camera control, no physics. The Jump and
                // Sneak action bindings ALSO act as FlyUp / FlyDown here so
                // that Space (= Jump in physics mode) consistently means
                // "go up" and `;` (= Sneak / FlyDown) consistently means
                // "go down" regardless of which mode you're in.
                if bindings.is_action_pressed(GameAction::FlyUp, &input)
                    || bindings.is_action_pressed(GameAction::Jump, &input) {
                    move_local.y += 1.0;
                }
                if bindings.is_action_pressed(GameAction::FlyDown, &input)
                    || bindings.is_action_pressed(GameAction::Sneak, &input) {
                    move_local.y -= 1.0;
                }
                local_player.apply_move(move_local, dt);
            }
            Some(b) => {
                // Physics mode: compute desired horizontal velocity from input,
                // apply jump from input, step the body, then position the camera
                // at the body's eye level.
                let f = local_player.forward();
                let f_horiz = glm::vec3(f.x, 0.0, f.z);
                let f_horiz = if f_horiz.norm_squared() > 1e-6 {
                    f_horiz.normalize()
                } else {
                    glm::vec3(1.0, 0.0, 0.0)
                };
                let r_horiz = local_player.right();

                let desired_world = f_horiz * move_local.x + r_horiz * move_local.z;
                let mut speed = physics::constants::WALK_SPEED;
                if bindings.is_action_pressed(GameAction::Sprint, &input) {
                    speed *= physics::constants::SPRINT_MULTIPLIER;
                }
                if bindings.is_action_pressed(GameAction::Sneak, &input) {
                    speed *= physics::constants::SNEAK_MULTIPLIER;
                }

                // Look up the slipperiness of the block under our feet.
                // The block 1 below the body's lowest point is the standing
                // surface. If we're on Air (mid-fall, momentarily) the value
                // is irrelevant — apply_input applies air control instead.
                let feet_y = b.aabb_min()[1].floor() as i32 - 1;
                let standing_block = chunk_manager.block_at(
                    b.position[0].floor() as i32,
                    feet_y,
                    b.position[2].floor() as i32,
                );
                let surface_slip = standing_block.slipperiness();

                b.apply_input(
                    [desired_world.x, 0.0, desired_world.z],
                    speed,
                    surface_slip,
                    dt,
                );

                if bindings.is_action_just_pressed(GameAction::Jump, &input) {
                    b.try_jump();
                }

                // Sample wind at the player's position and time, feed
                // into physics params. Result: a slowly-rotating lateral
                // breeze pushes the player around in open terrain. The
                // effect is subtle — a few % deviation at typical magnitudes
                // — but creates a sense of weather presence.
                //
                // Use SIMULATION time (TickId × period) not wall clock:
                // for rollback determinism, same tick = same wind. Wall
                // clock differs across machines / runs and would desync.
                let sim_t = chunk_manager.current_tick()
                    .to_sim_seconds(chunk_manager.tick_period());
                let (wind_x, wind_z, _) = unendlich_engine::wind::wind_at(
                    b.position[0] as f64,
                    b.position[1] as f64,
                    b.position[2] as f64,
                    sim_t,
                );
                let mut params = PhysicsParams::player_default();
                params.wind_force = [wind_x, wind_z];

                physics::step_body(
                    b,
                    dt,
                    params,
                    &chunk_manager,
                );

                // Camera follows the body, with eye at top of AABB.
                // Use render_y() (not position[1]) so step-up animation
                // smoothly lerps the camera up rather than teleporting.
                local_player.view.position = glm::vec3(
                    b.position[0],
                    b.render_y() + physics::constants::PLAYER_EYE_OFFSET,
                    b.position[2],
                );
            }
        }

        // Drive the chunk manager with the player's world position.
        // Multi-interest-point ready: today we pass a single-element
        // slice; future split-screen will pass one per local player.
        let pos = local_player.view.position;
        let t_mgr = Instant::now();
        chunk_manager.update_with_interest_points(&[
            (pos.x as i32, pos.y as i32, pos.z as i32),
        ]);
        t_manager_update_us += t_mgr.elapsed().as_micros() as u64;

        // Advance simulation ticks (plant lifecycle, future leaf decay,
        // fluid flow, etc.) by this frame's real elapsed time. Internally
        // the scheduler accumulates and only fires handlers when a full
        // tick period (default 0.5s) has passed.
        let t_tick = Instant::now();
        _render_probe.set(unendlich_engine::thread_probe::Activity::TickSim);
        chunk_manager.tick_simulation(dt);
        _render_probe.set(unendlich_engine::thread_probe::Activity::Working);
        t_tick_sim_us += t_tick.elapsed().as_micros() as u64;

        // ---- Ambient ecosystem: stream chunks in/out + tick roamers ----
        {
            use unendlich_engine::constants::CHUNK_SIZE;
            use unendlich_engine::item::CreatureKind;
            use unendlich_engine::sim::cognition::wander::Wander;
            use unendlich_engine::world::ecosystem::populate_region;

            let pp = local_player.view.position;
            let pcx = (pp.x.floor() as i32).div_euclid(CHUNK_SIZE);
            let pcz = (pp.z.floor() as i32).div_euclid(CHUNK_SIZE);

            // Only re-stream when the player crosses into a new chunk.
            if last_stream_chunk != Some((pcx, pcz)) {
                last_stream_chunk = Some((pcx, pcz));
                let delta = spawn_streamer.update((pcx, pcz), 2);
                // Despawn chunks left behind — but keep persistent (dragon) roamers.
                for c in &delta.to_despawn {
                    roamers.retain(|r| r.home != *c || r.persistent);
                }
                // Populate newly entered chunks (deterministic per cell + seed).
                let cpu = chunk_manager.generator().cpu();
                for c in &delta.to_populate {
                    if roamers.len() >= ROAMER_CAP {
                        break;
                    }
                    let bx = c.0 * CHUNK_SIZE;
                    let bz = c.1 * CHUNK_SIZE;
                    let spawns = populate_region(
                        bx,
                        bz,
                        CHUNK_SIZE,
                        CHUNK_SIZE,
                        WORLD_SEED_DEFAULT as u64,
                        STREAM_RARITY,
                        |x, z| {
                            let sy = cpu.surface_y_at(x, z, probe_y_min, probe_y_max).unwrap_or(72);
                            cpu.biome_at_3d(x, sy, z).biome
                        },
                        |x, z| cpu.surface_y_at(x, z, probe_y_min, probe_y_max).unwrap_or(64),
                    );
                    for s in &spawns {
                        if roamers.len() >= ROAMER_CAP {
                            break;
                        }
                        // Observation: UNENDLICH_SPAWN_ARCHE turns every spawn into
                        // a picker body plan (e.g. the super-tall builds), bridged
                        // into a world creature; otherwise the ecology kind.
                        let wseed = (WORLD_SEED_DEFAULT as u64)
                            ^ ((s.pos[0] as i32 as u32 as u64) << 20)
                            ^ (s.pos[2] as i32 as u32 as u64);
                        let (kind, mut cr) = if let Some(arche) = spawn_arche {
                            let bp = unendlich_engine::creature_design::EggSpec::chosen(arche, wseed).hatch();
                            (CreatureKind::Dragon, unendlich_engine::spawn::from_blueprint(&bp, s.pos))
                        } else {
                            (s.kind, unendlich_engine::spawn::spawn(s.kind, s.pos))
                        };
                        // s.pos.y is the surface TOP (surface block + 1); rest the
                        // feet there rather than a block under it.
                        cr.body.position[1] = cr.grounded_y(s.pos[1]);
                        // One genome per individual drives both its speed and its
                        // senses (eyes + ears from species × genome).
                        let genome = unendlich_engine::genetics::Genome::from_seed(wseed);
                        let ph = genome.express(wseed, 0);
                        let speed = 0.8 + ph.speed * 1.4; // ~0.8 .. 2.2 blk/s
                        // Turn rate from the gene: ~1.5 .. 5.5 rad/s (~85 .. 315 °/s).
                        let turn_rate = 1.5 + ph.turn_rate * 4.0;
                        let senses = unendlich_engine::sim::senses::senses_for(kind, &genome, wseed, 0);
                        let turn_profile = unendlich_engine::sim::cognition::steering::TurnProfile::from_seed(wseed);
                        let persistent = matches!(kind, CreatureKind::Dragon);
                        roamers.push(Roamer {
                            creature: cr,
                            wander: Wander::new(wseed, speed),
                            home: *c,
                            persistent,
                            senses,
                            turn_rate,
                            turn_profile,
                            vitality: unendlich_engine::sim::feeding::Vitality::full(),
                            sex: unendlich_engine::genetics::sex_from_seed(wseed),
                            kind,
                            genome,
                            forage_memory: unendlich_engine::sim::forage_memory::ForageMemory::new(16),
                            leap: unendlich_engine::sim::leap::Leap::from_genes(ph.speed, ph.speed),
                            airborne: None,
                            posture: unendlich_engine::sim::posture::Posture::new(0.0),
                            stumble: unendlich_engine::sim::stumble::Stumble::new(),
                        });
                    }
                }
                log::info!(
                    ">>> ECOSYSTEM STREAM: chunk ({}, {}) | +{} -{} | {} roamers",
                    pcx,
                    pcz,
                    delta.to_populate.len(),
                    delta.to_despawn.len(),
                    roamers.len()
                );
            }

            // Tick every roamer: PERCEIVE the player through its genome-given
            // senses (sight + hearing), flee only what it actually notices, steer
            // with sight around cliffs, and re-ground on the surface top.
            let cpu = chunk_manager.generator().cpu();
            use unendlich_engine::sim::fall::sighted_step;
            const ROAMER_HP: i32 = 20;

            // How fast (and thus how loudly) the player is moving — what creatures
            // hear. ~0.2 blk/tick walk ≈ 200 speed_milli at ~20 tps → blk/s × 50.
            let player_xz = (pp.x, pp.z);
            let player_speed = match last_player_xz {
                Some((lx, lz)) if dt > 1e-4 => ((pp.x - lx).powi(2) + (pp.z - lz).powi(2)).sqrt() / dt,
                _ => 0.0,
            };
            last_player_xz = Some(player_xz);
            let player_speed_milli = (player_speed * 50.0) as u32;
            let player_footstep = unendlich_engine::sim::sound::footstep_loudness(70, player_speed_milli, 20);
            let player_pos3 = [pp.x, pp.y, pp.z];

            roamer_tick = roamer_tick.wrapping_add(1);
            for r in roamers.iter_mut() {
                let Roamer {
                    creature: cr,
                    wander: w,
                    persistent: persist,
                    senses,
                    turn_rate,
                    turn_profile,
                    vitality,
                    forage_memory,
                    leap,
                    airborne,
                    posture,
                    stumble,
                    ..
                } = r;
                let self_xz = (cr.body.position[0], cr.body.position[2]);
                let cy = cr.body.position[1] as i32;
                let here_biome = cpu.biome_at_3d(self_xz.0.floor() as i32, cy, self_xz.1.floor() as i32).biome;

                // --- Eat by habitat ---
                // Grassy biomes graze faster than the metabolic burn (net gain);
                // barren ground (desert, rock, ocean, caves) gives nothing, so a
                // creature there slowly starves. Habitat decides who thrives.
                let net = unendlich_engine::sim::feeding::graze_rate(here_biome)
                    - unendlich_engine::sim::feeding::STARVE_DECAY_PER_SEC;
                vitality.energy = (vitality.energy + net * dt).clamp(0.0, 1.0);

                // Surface probe (terrain steering, grounding, posture, leap).
                let surface_at = |x: f32, z: f32| {
                    cpu.surface_y_at(x.floor() as i32, z.floor() as i32, cy - 48, cy + 16)
                };

                // --- Airborne: integrate a leap arc, then everything else waits ---
                if let Some((v, aloft0)) = *airborne {
                    cr.body.position[0] += v[0] * dt;
                    cr.body.position[1] += v[1] * dt;
                    cr.body.position[2] += v[2] * dt;
                    let nvy = v[1] - unendlich_engine::sim::leap::GRAVITY * dt;
                    let aloft = aloft0.saturating_add(1);
                    *airborne = Some(([v[0], nvy, v[2]], aloft));
                    cr.gait_phase = 0.0;
                    let on_ground = nvy < 0.0
                        && surface_at(cr.body.position[0], cr.body.position[2])
                            .map(|t| cr.body.position[1] <= cr.grounded_y((t + 1) as f32))
                            .unwrap_or(false);
                    if on_ground || aloft > 80 {
                        if let Some(t) = surface_at(cr.body.position[0], cr.body.position[2]) {
                            cr.body.position[1] = cr.grounded_y((t + 1) as f32);
                        }
                        *airborne = None;
                    }
                    continue; // skip normal movement while in the air
                }

                // --- Posture: stable lean (clamped delta + deadzone + upright bias) ---
                let roughness;
                {
                    let (fc, fs) = (cr.yaw.cos(), cr.yaw.sin());
                    let span = 1.2_f32;
                    let h = cr.body.position[1];
                    let sample = |dx: f32, dz: f32| {
                        surface_at(self_xz.0 + dx, self_xz.1 + dz).map(|t| (t + 1) as f32).unwrap_or(h)
                    };
                    let front = sample(fc * span, fs * span);
                    let back = sample(-fc * span, -fs * span);
                    let left = sample(-fs * span, fc * span);
                    let right = sample(fs * span, -fc * span);
                    let lim = unendlich_engine::sim::posture::LeanLimits::default();
                    // Larger bodies absorb small terrain steps: scale the lean by
                    // body height so a tall dragon shrugs off a 1-block lip that a
                    // deer would lean on (baseline half-height ~0.5).
                    let body_size = (cr.body.half_extents[1] / 0.5).max(1.0);
                    let (tp, tr) = unendlich_engine::sim::posture::terrain_lean_sized(
                        front, back, left, right, span * 2.0, &lim, body_size,
                    );
                    posture.settle_to_terrain(tp, tr, 4.0, dt);
                    cr.pitch = posture.pitch;
                    cr.roll = posture.roll;
                    roughness = unendlich_engine::sim::posture::terrain_roughness(front, back, left, right);
                }

                // --- Foraging: hungry on barren ground → head for remembered grass ---
                let here_lush = unendlich_engine::sim::feeding::graze_rate(here_biome)
                    > unendlich_engine::sim::feeding::STARVE_DECAY_PER_SEC;
                if here_lush {
                    forage_memory.remember((self_xz.0.floor() as i32, self_xz.1.floor() as i32), roamer_tick);
                }
                const FORAGE_HORIZON: u32 = 4000;
                let foraging_goal: Option<(f32, f32)> = if vitality.is_hungry() && !here_lush {
                    forage_memory
                        .nearest(self_xz, roamer_tick, FORAGE_HORIZON)
                        .map(|c| (c.0 as f32 + 0.5, c.1 as f32 + 0.5))
                } else {
                    None
                };

                // --- Perceive the player ---
                // Sight: from the creature's eye, looking along its heading.
                let eye = [self_xz.0, cr.body.position[1] + cr.body.half_extents[1], self_xz.1];
                let facing = [cr.yaw.cos(), 0.0, cr.yaw.sin()];
                let seen = unendlich_engine::sim::eyes::sees_in_fog(senses.eyes, frame_fog_sigma, eye, facing, player_pos3, 0.4, player_speed)
                    .is_detected();
                let perceived: Option<(f32, f32)> = if seen {
                    Some(player_xz) // sight pins the true position
                } else {
                    // Hearing: the player's footstep loudness perceived here
                    // (occlusion not yet modelled), gated by this creature's ears
                    // and the biome's ambient floor; then localized to a NOISY
                    // estimate it flees from.
                    let dist = ((player_xz.0 - self_xz.0).powi(2) + (player_xz.1 - self_xz.1).powi(2)).sqrt();
                    let perc = unendlich_engine::sim::sound::perceived_loudness(player_footstep, dist as i32, 0);
                    let floor = unendlich_engine::sim::sound::biome_ambient_loudness(here_biome);
                    if senses.notices(perc, floor) {
                        let jseed = ((self_xz.0.floor() as i32 as u32 as u64) << 32)
                            | (self_xz.1.floor() as i32 as u32 as u64);
                        unendlich_engine::sim::localize::localize(
                            [self_xz.0, cr.body.position[1], self_xz.1],
                            &[([player_xz.0, pp.y, player_xz.1], perc)],
                            senses.hearing_threshold,
                            jseed,
                        )
                        .map(|e| (e.pos[0], e.pos[2]))
                    } else {
                        None
                    }
                };
                // Flight-initiation distance: a creature may *notice* the player far
                // off (and we could surface that as wariness later), but it only
                // BOLTS once the threat is within a flight distance. Without this,
                // fauna flee on first sight from ~48 blocks and you never see them
                // roam. Dragons hold their ground (no flight).
                const FLIGHT_DIST: f32 = 16.0;
                let threat: Option<(f32, f32)> = if *persist {
                    None // persistent (dragons): don't flee the player
                } else {
                    perceived.filter(|&(tx, tz)| {
                        (tx - self_xz.0).powi(2) + (tz - self_xz.1).powi(2) < FLIGHT_DIST * FLIGHT_DIST
                    })
                };

                // Flee the perceived threat (perception + flight distance already
                // gated, so a large radius: if it's a threat at all, it runs).
                let (mut rdx, mut rdz) = w.step_aware(dt, self_xz, threat, 1000.0, 4.0);
                // If not fleeing but hungry with a remembered patch, steer to it
                // (goal pathing — turns crisply, unlike idle wander).
                let mut pathing = false;
                if threat.is_none() {
                    if let Some((gx, gz)) = foraging_goal {
                        let (gdx, gdz) = (gx - self_xz.0, gz - self_xz.1);
                        let gd = (gdx * gdx + gdz * gdz).sqrt();
                        if gd > 0.5 {
                            let spd = w.speed * dt;
                            rdx = gdx / gd * spd;
                            rdz = gdz / gd * spd;
                            w.heading = gdz.atan2(gdx);
                            pathing = true;
                        }
                    }
                }
                // Stumble recovery slows the step (and the trip below may start
                // one). A creature mid-stumble is hobbled until it recovers.
                let speed_mult = stumble.tick();
                rdx *= speed_mult;
                rdz *= speed_mult;
                let step = (rdx * rdx + rdz * rdz).sqrt();
                let desired = rdz.atan2(rdx);
                let desperation = if threat.is_some() { 0.8 } else { 0.0 };
                let (dx, dz) = sighted_step(self_xz, desired, step, ROAMER_HP, desperation, &surface_at);
                if dx != 0.0 || dz != 0.0 {
                    cr.body.position[0] += dx;
                    cr.body.position[2] += dz;
                    cr.gait_phase = (cr.gait_phase + (dx * dx + dz * dz).sqrt() * 2.5) % std::f32::consts::TAU;
                    if let Some(t) = surface_at(cr.body.position[0], cr.body.position[2]) {
                        // Smooth the vertical: ease toward the surface top rather than
                        // snapping a whole block each frame (that read as jittery
                        // "stepping up and down"). Big corrections (spawn/teleport)
                        // still snap.
                        let target_y = cr.grounded_y((t + 1) as f32);
                        let gap = target_y - cr.body.position[1];
                        if gap.abs() > 2.0 {
                            cr.body.position[1] = target_y;
                        } else {
                            cr.body.position[1] += gap * (dt * 12.0).min(1.0);
                        }
                    }
                    // It wants to face where it stepped. Fleeing OR goal-pathing
                    // turns crisply; idle wandering uses the detent + nonlinear map
                    // so it doesn't vacillate or look robotic.
                    w.heading = dz.atan2(dx);
                    let max_step = *turn_rate * dt;
                    cr.yaw = if threat.is_some() || pathing {
                        unendlich_engine::sim::awareness::turn_toward(cr.yaw, w.heading, max_step)
                    } else {
                        unendlich_engine::sim::cognition::steering::wander_turn(cr.yaw, w.heading, max_step, *turn_profile)
                    };
                } else {
                    // Hemmed in: a normal step won't go. If it's agile (a fast
                    // leaper), try PARKOUR — launch a ballistic hop toward a ledge
                    // or across a gap it can't simply walk, if the arc reaches it.
                    let mut leapt = false;
                    if leap.forward > 5.0 {
                        let (fc, fs) = (w.heading.cos(), w.heading.sin());
                        let here_y = cr.body.position[1];
                        for d in [2.0_f32, 3.0, 4.0] {
                            if let Some(t) = surface_at(self_xz.0 + fc * d, self_xz.1 + fs * d) {
                                let rise = cr.grounded_y((t + 1) as f32) - here_y;
                                if rise < 8.0
                                    && leap.clears_gap(d, rise, unendlich_engine::sim::leap::GRAVITY)
                                {
                                    *airborne = Some(([fc * leap.forward, leap.up, fs * leap.forward], 0));
                                    leapt = true;
                                    break;
                                }
                            }
                        }
                    }
                    if leapt {
                        continue;
                    }
                    // Otherwise SURVEY: face the direction that reveals the most open
                    // ground (an information-seeking turn — get your bearings / face
                    // where a predator would come from). Hold position, re-grounded.
                    use unendlich_engine::sim::awareness::{best_facing, clear_sightline, LookCandidate};
                    use unendlich_engine::sim::fall::STEP_UP_BLOCKS;
                    let base_y = cr.body.position[1] as i32;
                    let max_view = 24.0;
                    let cands: Vec<LookCandidate> = (0..8)
                        .map(|i| {
                            let heading = i as f32 * std::f32::consts::FRAC_PI_4;
                            LookCandidate {
                                heading,
                                clear_dist: clear_sightline(self_xz, heading, max_view, base_y, STEP_UP_BLOCKS, &surface_at),
                                familiarity: 0.5, // no landscape memory yet; novelty neutral
                            }
                        })
                        .collect();
                    if let Some(h) = best_facing(w.heading, &cands, max_view) {
                        w.heading = h;
                    } else {
                        w.heading += std::f32::consts::FRAC_PI_2;
                    }
                    // Surveying is idle behaviour — sweep with the detent +
                    // nonlinear turn map so it pans smoothly, not robotically.
                    cr.yaw = unendlich_engine::sim::cognition::steering::wander_turn(
                        cr.yaw, w.heading, *turn_rate * dt, *turn_profile,
                    );
                    if let Some(f) = surface_at(self_xz.0, self_xz.1) {
                        let target_y = cr.grounded_y((f + 1) as f32);
                        let gap = target_y - cr.body.position[1];
                        if gap.abs() > 2.0 {
                            cr.body.position[1] = target_y;
                        } else {
                            cr.body.position[1] += gap * (dt * 12.0).min(1.0);
                        }
                    }
                }

                // --- Stumble: bolting across broken ground can trip ---
                // Risk rises with the speed it actually moved × how uneven the
                // ground is; a trip slows it (recovery) and lurches it toward the
                // leg that caught. Calm/flat movement never trips.
                if !stumble.is_stumbling() {
                    let moved = (dx * dx + dz * dz).sqrt();
                    let speed_now = if dt > 1e-4 { moved / dt } else { 0.0 };
                    const SUREFOOT: f32 = 0.3;
                    let risk = unendlich_engine::sim::stumble::stumble_risk(speed_now, roughness, SUREFOOT);
                    let tseed = (roamer_tick as u64)
                        ^ ((self_xz.0.floor() as i32 as u32 as u64) << 20)
                        ^ (self_xz.1.floor() as i32 as u32 as u64);
                    if unendlich_engine::sim::stumble::trips(risk, tseed) {
                        stumble.trigger(unendlich_engine::sim::stumble::STUMBLE_RECOVER_TICKS);
                        let (yk, pk) =
                            unendlich_engine::sim::stumble::stumble_kick(risk, (tseed & 1) == 0);
                        cr.yaw += yk;
                        cr.pitch += pk;
                    }
                }
            }

            // ---- Starvation: the unfed die ----
            // Persistent creatures (dragons) are exempt; anyone else that burned
            // through its reserve in barren country is removed.
            roamers.retain(|r| r.persistent || r.vitality.energy > 0.0);

            // ---- Mating ----
            // Nearby, well-fed, opposite-sex pairs of the same species breed. The
            // child inherits a Mendelian cross of the parents' genomes and spawns
            // at a parent's spot with a modest reserve. O(n^2) over the cap (96),
            // but the per-pair checks are cheap and try_mate only fires on close,
            // fed, opposite-sex pairs.
            {
                const MATE_DIST2: f32 = 9.0; // within 3 blocks
                let mut offspring: Vec<Roamer> = Vec::new();
                let n = roamers.len();
                'pairs: for i in 0..n {
                    for j in (i + 1)..n {
                        if roamers.len() + offspring.len() >= ROAMER_CAP {
                            break 'pairs;
                        }
                        if roamers[i].kind != roamers[j].kind {
                            continue;
                        }
                        let pi = roamers[i].creature.body.position;
                        let pj = roamers[j].creature.body.position;
                        let d2 = (pi[0] - pj[0]).powi(2) + (pi[2] - pj[2]).powi(2);
                        if d2 > MATE_DIST2 {
                            continue;
                        }
                        let mseed = (pi[0].to_bits() as u64)
                            ^ ((pj[2].to_bits() as u64) << 21)
                            ^ ((i as u64) << 8)
                            ^ ((j as u64) << 40);
                        let (left, right) = roamers.split_at_mut(j);
                        let a = &mut left[i];
                        let b = &mut right[0];
                        if let Some(child) = unendlich_engine::sim::feeding::try_mate(
                            a.sex, b.sex, &mut a.vitality, &mut b.vitality, &a.genome, &b.genome, mseed,
                        ) {
                            let kind = a.kind;
                            let pos = a.creature.body.position;
                            let mut cr = unendlich_engine::spawn::spawn(kind, [pos[0], pos[1], pos[2]]);
                            cr.body.position[1] = cr.grounded_y(pos[1]);
                            let cseed = mseed ^ 0x5DEE_CE15;
                            let ph = child.express(cseed, 0);
                            offspring.push(Roamer {
                                creature: cr,
                                wander: Wander::new(cseed, 0.8 + ph.speed * 1.4),
                                home: a.home,
                                persistent: a.persistent,
                                senses: unendlich_engine::sim::senses::senses_for(kind, &child, cseed, 0),
                                turn_rate: 1.5 + ph.turn_rate * 4.0,
                                turn_profile: unendlich_engine::sim::cognition::steering::TurnProfile::from_seed(cseed),
                                vitality: unendlich_engine::sim::feeding::Vitality::new(0.4),
                                sex: unendlich_engine::genetics::sex_from_seed(cseed),
                                kind,
                                genome: child,
                                forage_memory: unendlich_engine::sim::forage_memory::ForageMemory::new(16),
                                leap: unendlich_engine::sim::leap::Leap::from_genes(ph.speed, ph.speed),
                                airborne: None,
                                posture: unendlich_engine::sim::posture::Posture::new(0.0),
                                stumble: unendlich_engine::sim::stumble::Stumble::new(),
                            });
                        }
                    }
                }
                roamers.extend(offspring);
            }
        }

        // ---- Rollback input capture ----
        //
        // Each render frame, OR the currently-held action bits into the
        // pending_frame and accumulate mouse deltas. When the chunk
        // manager's tick number advances (a tick boundary fired), flush
        // the accumulated frame into the ring buffer with the OLD tick
        // number, and start a fresh accumulator for the new tick.
        //
        // Held actions: OR — anything held during the tick is
        //   considered "held for this tick."
        // Just-pressed: OR — any one-shot key transition during the
        //   tick fires once for that tick.
        // Look deltas: SUM — total camera rotation for the tick.
        //
        // This is passive infrastructure today: simulation still reads
        // input directly from `input` and `bindings`. The buffer is
        // populated for future replay use.
        for action in [
            unendlich_engine::keybindings::GameAction::MoveForward,
            unendlich_engine::keybindings::GameAction::MoveBackward,
            unendlich_engine::keybindings::GameAction::MoveLeft,
            unendlich_engine::keybindings::GameAction::MoveRight,
            unendlich_engine::keybindings::GameAction::Jump,
            unendlich_engine::keybindings::GameAction::Sneak,
            unendlich_engine::keybindings::GameAction::Sprint,
            unendlich_engine::keybindings::GameAction::FlyUp,
            unendlich_engine::keybindings::GameAction::FlyDown,
            unendlich_engine::keybindings::GameAction::ToggleFlight,
        ] {
            let bit = ActionBits::from_action(action);
            if bit.is_empty() { continue; }
            if bindings.is_action_pressed(action, &input) {
                pending_frame.held.insert(bit);
            }
            if bindings.is_action_just_pressed(action, &input) {
                pending_frame.just_pressed.insert(bit);
            }
        }
        // Mouse deltas accumulate.
        pending_frame.look_yaw_delta += input.cursor_delta.0 as f32;
        pending_frame.look_pitch_delta += input.cursor_delta.1 as f32;

        // Tick-boundary flush.
        let now_tick = chunk_manager.current_tick();
        if now_tick != last_captured_tick {
            // Stamp the pending frame with the tick that JUST COMPLETED
            // (i.e. last_captured_tick + 1, which equals now_tick), then
            // push and reset.
            pending_frame.tick_id = now_tick;
            input_buffer.push(pending_frame);
            pending_frame = InputFrame::new(now_tick.next());
            last_captured_tick = now_tick;
        }

        // ---- Debug overlay toggle + populate ----
        //
        // F4 toggles the in-world debug overlay. When enabled, the
        // scene is repopulated each frame with markers / arrows
        // showing real-time engine state: lifecycle stages above
        // plants, wind direction at player, physics velocity arrow,
        // tick-cull boundary AABB. The mesh build happens here too,
        // but actual rendering is deferred to a future renderer
        // integration (the synthetic-chunk pipeline).
        if bindings.is_action_just_pressed(unendlich_engine::keybindings::GameAction::ToggleDebugOverlay, &input) {
            debug_scene.enabled = !debug_scene.enabled;
            // Loud stderr print (not log::info) so the user sees it
            // regardless of RUST_LOG setting. F4 toggle visibility
            // matters because if the overlay turns on but no
            // primitives are in view the user can't tell if F4 fired.
            eprintln!(
                "[F4] Debug overlay: {} — markers/arrows/AABBs draw in world space.\n      Look for: yellow AABB around your body, cyan wind arrow above you,\n      green/orange velocity arrow at your feet, magenta forward marker\n      ~3 blocks in front of camera, color-coded plant lifecycle markers,\n      purple tick-cull boundary at edge of simulating chunks.",
                if debug_scene.enabled { "ON" } else { "OFF" }
            );
        }

        // ---- Tick speed slider (F6 = slower, F7 = faster) ----
        //
        // Geometric step: each press multiplies/divides by sqrt(10), so
        // 6 presses = 1000x. Clamped to [0.01, 1000.0]. Mutates the
        // chunk manager's tick_speed_multiplier, which scales real_dt
        // before it reaches the scheduler. Slow simulation features
        // (plant lifecycle stages, weathering) become visible in
        // seconds at 100x+ — useful for showcasing or debugging.
        if bindings.is_action_just_pressed(unendlich_engine::keybindings::GameAction::TickSpeedSlower, &input) {
            chunk_manager.tick_speed_multiplier =
                (chunk_manager.tick_speed_multiplier / 10.0_f32.sqrt()).max(0.01);
            eprintln!("[F6] Tick speed: {:.3}x — simulation runs slower (use F7 to speed up)",
                chunk_manager.tick_speed_multiplier);
        }
        if bindings.is_action_just_pressed(unendlich_engine::keybindings::GameAction::TickSpeedFaster, &input) {
            chunk_manager.tick_speed_multiplier =
                (chunk_manager.tick_speed_multiplier * 10.0_f32.sqrt()).min(10000.0);
            eprintln!("[F7] Tick speed: {:.3}x — simulation runs faster (use F6 to slow down)",
                chunk_manager.tick_speed_multiplier);
        }
        if bindings.is_action_just_pressed(unendlich_engine::keybindings::GameAction::LightFire, &input) {
            if !local_player.permissions.can_light_fire {
                eprintln!("[F9] Permission denied: can_light_fire is false");
            } else {
                let pos = local_player.view.position;
                let foot_y = pos.y - unendlich_engine::physics::constants::PLAYER_EYE_OFFSET;
                // F9 toggles: if there's a fire within 3 blocks, extinguish it
                // (closest first). Otherwise plant a new fire at the player's
                // feet. This lets one key both light AND extinguish — no need
                // for a separate keybind, and the action is intuitive (press
                // F9 standing on/near a fire to put it out).
                const TOGGLE_RADIUS: f32 = 3.0;
                if let Some(removed) = heat_sources.remove_nearest_within(
                    pos.x, foot_y, pos.z, TOGGLE_RADIUS)
                {
                    eprintln!("[F9] Extinguished fire at ({:.1}, {:.1}, {:.1}) — total sources: {}",
                        removed.position[0], removed.position[1], removed.position[2],
                        heat_sources.len());
                } else {
                    // Drop a fire at the player's feet. Intensity 800°C, radius 5 —
                    // visible warming bubble extending a few blocks. The temperature
                    // → convection coupling produces an updraft that the F4 wind
                    // arrow shows as a yellow-tilted-up arrow.
                    let src = unendlich_engine::heat_source::HeatSource::new(
                        [pos.x, foot_y, pos.z], 800.0, 5.0);
                    let idx = heat_sources.add(src);
                    eprintln!("[F9] Lit fire #{} at ({:.1}, {:.1}, {:.1}) — total sources: {}",
                        idx, pos.x, foot_y, pos.z, heat_sources.len());
                }
            }
        }

        debug_scene.clear();
        if debug_scene.enabled {
            unendlich_engine::debug_scene::populate_debug_scene(
                &mut debug_scene,
                &chunk_manager,
                body.as_ref(),
                &local_player.view.position,
                &local_player.forward(),
                &heat_sources,
                sky_state.day_phase(),
            );
        }
        // Build mesh from current scene and upload at the sentinel
        // coord. The world renderer skips frustum culling for this
        // coord so primitives outside any single chunk still render.
        // When the scene is disabled, build_debug_mesh returns empty,
        // and upload_prebuilt_mesh evicts any prior mesh — clean.
        let debug_mesh = unendlich_engine::debug_overlay::build_debug_mesh(&debug_scene);
        let debug_coord = unendlich_engine::world::chunk::ChunkCoord {
            x: i32::MIN, y: i32::MIN, z: i32::MIN,
        };
        if let Err(e) = renderer.upload_prebuilt_mesh(&ctx, debug_coord, debug_mesh, 0) {
            log::warn!("debug overlay upload failed: {}", e);
        }

        // ---- Block palette selection (hotbar 1..9 pick the place block) ----
        {
            use unendlich_engine::keybindings::GameAction;
            for (i, action) in [
                GameAction::Hotbar1, GameAction::Hotbar2, GameAction::Hotbar3,
                GameAction::Hotbar4, GameAction::Hotbar5, GameAction::Hotbar6,
                GameAction::Hotbar7, GameAction::Hotbar8, GameAction::Hotbar9,
            ]
            .iter()
            .enumerate()
            {
                if bindings.is_action_just_pressed(*action, &input) {
                    selected_block = PLACE_PALETTE[i];
                    log::info!("Selected place block: {:?}", selected_block);
                }
            }
        }

        // ---- Block edit input ----
        //
        // Left mouse: break the block we're aiming at (set to Air).
        // Right mouse: place a block adjacent to the face we're aiming at.
        //
        // Reuses the eye position + camera forward as the ray origin/direction.
        // Uses an OpaqueOnlyFilter for breaking (you can break leaves through
        // grass — questionable but matches Minecraft) and AnySolidFilter for
        // placement (you can place against any solid, including leaves).
        //
        // Cooldown prevents rapid-fire when the button is held. Just-pressed
        // edge detection alone would only do one edit per click — fine for
        // building, frustrating for breaking through walls. Cooldown is the
        // middle ground.
        let mouse_left_held = input.is_mouse_button_pressed(MouseButton::Button1);
        let mouse_right_held = input.is_mouse_button_pressed(MouseButton::Button2);
        let cooldown_ok = last_block_action.elapsed().as_millis() >= BLOCK_ACTION_COOLDOWN_MS;
        // Permission gate: a player without can_break_block / can_place_block
        // shouldn't even raycast (saves cycles). Single-player default
        // permissions grant both, so behavior is unchanged. Multiplayer
        // server pushes restrictive permissions to e.g. spectators.
        let break_allowed = mouse_left_held && local_player.permissions.can_break_block;
        let place_allowed = mouse_right_held && local_player.permissions.can_place_block;
        if mouse_captured && cooldown_ok && (break_allowed || place_allowed) {
            let cam_forward = local_player.forward();
            // v0.24.23 propagation: route the player block-selection
            // raycast through the octree-backed hierarchical raycast
            // on ChunkManager. Same semantics as the legacy
            // `raycast::raycast` over BlockSampler, but with the
            // skip-pass property: rays through homogeneous air visit
            // one node per region instead of one cell per cell.
            //
            // Predicate: `is_solid` matches the legacy `AnySolidFilter`
            // — player targets any solid block including leaves /
            // glass / crystal (same behaviour for break and place).
            let hit_opt = chunk_manager.raycast_octree(
                [pos.x, pos.y, pos.z],
                [cam_forward.x, cam_forward.y, cam_forward.z],
                6.0,
                |b| b.is_solid(),
            );
            if let Some(world_hit) = hit_opt {
                let hit_cell = [world_hit.world_cell.0, world_hit.world_cell.1, world_hit.world_cell.2];
                let hit_block_id = world_hit.block;
                let hit_face = match raycast::BlockFace::from_normal(world_hit.normal) {
                    Some(f) => f,
                    None => {
                        // Degenerate normal — shouldn't happen for
                        // axis-aligned voxel raycasts but log if it
                        // does. Skip this frame's action.
                        log::warn!("octree raycast returned non-axis normal {:?}", world_hit.normal);
                        // Synthesize PosY as a fallback so the rest of
                        // the code path still runs.
                        raycast::BlockFace::PosY
                    }
                };
                if break_allowed {
                    // Break: replace hit block with air.
                    let [x, y, z] = hit_cell;
                    if chunk_manager.set_block(x, y, z, BlockId::Air) {
                        log::info!("Broke {:?} at ({},{},{})", hit_block_id, x, y, z);
                        last_block_action = Instant::now();
                    }
                } else {
                    // Place: put the selected palette block adjacent to the face we hit.
                    let target = hit_face.neighbor(hit_cell);
                    let [x, y, z] = target;
                    // Don't place inside the player's AABB — would suffocate.
                    if let Some(b) = body.as_ref() {
                        let body_min = b.aabb_min();
                        let body_max = b.aabb_max();
                        let cell_overlaps_body =
                            (x as f32) < body_max[0] && (x as f32 + 1.0) > body_min[0] &&
                            (y as f32) < body_max[1] && (y as f32 + 1.0) > body_min[1] &&
                            (z as f32) < body_max[2] && (z as f32 + 1.0) > body_min[2];
                        if cell_overlaps_body {
                            // Skip silently to avoid log spam when player
                            // accidentally aims at their own feet.
                            // No-op.
                        } else if chunk_manager.set_block(x, y, z, selected_block) {
                            log::info!("Placed {:?} at ({},{},{}) on {:?} face of {:?}",
                                selected_block, x, y, z, hit_face, hit_block_id);
                            last_block_action = Instant::now();
                        }
                    } else if chunk_manager.set_block(x, y, z, selected_block) {
                        log::info!("Placed {:?} at ({},{},{}) on {:?} face", selected_block, x, y, z, hit_face);
                        last_block_action = Instant::now();
                    }
                }

                // The chunk's version got bumped by set_block. The
                // mesh-streaming loop below detects stale meshes
                // (version-in-renderer != version-in-chunk) and
                // resubmits them to the meshing pool.
                //
                // CRITICAL: do NOT evict the chunk here. Evicting would
                // create a visible flash — the chunk renders empty
                // until the new mesh arrives (which happens 1-N frames
                // later via the off-thread meshing pool). Instead, we
                // leave the OLD mesh visible and atomically swap to
                // the new one when ready (via upload_prebuilt_mesh's
                // insert-then-defer-destroy path).
                //
                // We DO need to clear meshing_in_flight so the
                // resubmission logic doesn't think a job is already
                // pending. (If a meshing job for the OLD version is
                // still in flight, the stale-mesh detector will
                // discard it on completion.)
                let edit_chunk = ChunkCoord::from_block_pos(
                    hit_cell[0], hit_cell[1], hit_cell[2],
                );
                meshing_in_flight.remove(&edit_chunk);
                // Boundary case: if the edit is at the chunk's edge, the
                // neighbor chunk's boundary faces might also need redrawing.
                // For each neighbor we ALSO bump that chunk's version
                // (so the resubmission loop will re-mesh it) by performing
                // a no-op edit. Cleaner: just remove from in_flight; if
                // nothing changed about the neighbor, the new mesh is
                // identical. If something did change (boundary face),
                // it gets the right faces.
                //
                // Actually, the chunk version of neighbors does NOT bump
                // when set_block edits a different chunk. So we can't
                // detect them as stale. A cleaner fix is needed in
                // chunk_manager.set_block, but for now: drop the
                // neighbor from in_flight and rely on the user not
                // typically caring about subtle cross-chunk seam updates.
                for (dx, dy, dz) in &[(1,0,0),(-1,0,0),(0,1,0),(0,-1,0),(0,0,1),(0,0,-1)] {
                    let nx = hit_cell[0] + dx;
                    let ny = hit_cell[1] + dy;
                    let nz = hit_cell[2] + dz;
                    let nbr_chunk = ChunkCoord::from_block_pos(nx, ny, nz);
                    if nbr_chunk != edit_chunk {
                        meshing_in_flight.remove(&nbr_chunk);
                    }
                }
            }
        }

        // ---- Cloud animation remesh trigger ----
        //
        // Periodically bump the version of every loaded chunk that
        // intersects the cloud band. Reasons cloud chunks need this
        // (and other chunks don't):
        //   - Cloud cubes are sampled from a time-shifted noise field
        //     in `cloud_density_at_time`. Without re-meshing, they're
        //     frozen at the time they were last meshed.
        //   - Terrain doesn't change with time → no need to re-mesh.
        //
        // Bumping the version triggers the existing version-aware
        // streaming loop to resubmit the chunks. Old mesh stays
        // visible until new one arrives (no flash, thanks to the
        // earlier remesh-fix work).
        if last_cloud_remesh.elapsed().as_secs_f32() > CLOUD_REMESH_INTERVAL {
            let t0 = Instant::now();
            last_cloud_remesh = Instant::now();
            // Find every loaded chunk whose Y range crosses the cloud
            // band, bump its version. Use the chunk_manager's chunks
            // arc directly to avoid copying coords.
            //
            // CRITICAL CORRECTNESS: collect the coord list FIRST, then
            // drop the iterator, THEN issue bumps.
            //
            // Why: `chunks_arc.iter()` holds a per-shard READ lock while
            // iterating each shard. `bump_chunk_version` calls
            // `chunks.get_mut(coord)` which takes a WRITE lock on coord's
            // shard. parking_lot RwLock is NOT reentrant — if `coord`'s
            // shard happens to be the same shard the iter is currently
            // on, the write lock deadlocks waiting for the read we
            // ourselves are holding via the iter.
            //
            // This was the user-visible "input freezes" bug: the main
            // thread deadlocks here mid-iteration, the frame loop stops
            // advancing, the OS event pump stops being called, but
            // Vulkan keeps presenting the LAST drawn frame so the
            // window LOOKS alive. Symptom: keys stop responding, mouse
            // stops rotating camera, but the game appears at full FPS
            // (because no new frames are being rendered — what's on
            // screen is the last completed swap).
            //
            // Fix: materialize the coord list (releasing the iter,
            // releasing all shard read locks), then iterate the Vec
            // and call bump. Each bump now acquires its own clean
            // write lock with no overlapping reader.
            let chunks_arc = chunk_manager.chunks_arc();
            let cb_bottom_chunk_y =
                unendlich_engine::cloud::CLOUD_BAND_BOTTOM.div_euclid(unendlich_engine::constants::CHUNK_SIZE);
            let cb_top_chunk_y =
                unendlich_engine::cloud::CLOUD_BAND_TOP.div_euclid(unendlich_engine::constants::CHUNK_SIZE);
            let cloud_coords: Vec<ChunkCoord> = chunks_arc.iter()
                .map(|entry| *entry.key())
                .filter(|c| c.y >= cb_bottom_chunk_y && c.y <= cb_top_chunk_y)
                .collect();
            // Iter dropped here — all shard read locks released.
            for coord in cloud_coords {
                chunk_manager.bump_chunk_version(coord);
            }
            t_cloud_remesh_us += t0.elapsed().as_micros() as u64;
        }

        // ---- Lava → HeatSourceMap periodic sync ----
        //
        // Walk the loaded chunks. For each that we haven't scanned yet,
        // scan it for surface lava cells and register them with the
        // HeatSourceMap. For chunks we'd previously registered that are
        // NO LONGER in the chunk manager (evicted), tear down their
        // heat sources via the chunk's world-space bounding box.
        //
        // The sync runs at LAVA_SYNC_INTERVAL (currently 1Hz) so per-
        // chunk scan cost (32K cell visit) is amortized over time.
        // A typical world has ~3% lava density in deep stone chunks, so
        // each scan adds ~100-1000 heat sources per lava-rich chunk.
        // The HeatSourceMap's contribution evaluation is short-range
        // (Gaussian falloff with 4-block radius), so distant lava
        // sources don't cost the temperature query much.
        if last_lava_sync.elapsed().as_secs_f32() > LAVA_SYNC_INTERVAL {
            let t_lava_start = Instant::now();
            last_lava_sync = Instant::now();
            let chunks_arc = chunk_manager.chunks_arc();
            // Step 1: register newly-loaded chunks. Iterate the manager's
            // chunks; for any not in our registered set, scan and
            // register, then mark.
            // Cap per-tick registration so a fresh world's first sync
            // doesn't all-at-once spawn 100K heat sources. With cap=8
            // and an interval of 1Hz, the lava field stabilizes over
            // ~10s of a 100-chunk-deep world load. Player won't notice;
            // far lava chunks are out of view anyway.
            //
            // PERF/SAFETY: collect target coords FIRST without holding
            // the DashMap iter, THEN scan each. Holding `chunks_arc.iter()`
            // for the duration of the scan keeps shard read locks alive,
            // blocking worker-thread inserts of newly-completed chunks.
            // With 8 chunks × 32K cells per scan that's milliseconds of
            // blocked inserts on the worker side — small individually,
            // but compounding when worker delivery is bursty.
            //
            // Two-step: (1) snapshot coord-list with a brief iter,
            // (2) scan each by `get(&coord)` — each get is one shard
            // read, released before next scan starts. Workers can
            // insert between scans.
            const MAX_LAVA_REGISTRATIONS_PER_TICK: usize = 8;
            let lava_max_y = (unendlich_engine::world::world_model::anchors::SEA_LEVEL_BASELINE as i32) - 30;
            // Step 1a: snapshot the list of unregistered coords (cheap walk).
            let candidate_coords: Vec<ChunkCoord> = {
                let mut out: Vec<ChunkCoord> = Vec::new();
                for entry in chunks_arc.iter() {
                    let coord = *entry.key();
                    if lava_registered.contains(&coord) { continue; }
                    out.push(coord);
                }
                out
            }; // iter dropped here, all shard locks released.
            let mut registered_this_tick = 0;
            // Step 1b: process up to N candidates. Each scan briefly
            // acquires one shard read lock via get, releases on drop.
            for coord in candidate_coords {
                if registered_this_tick >= MAX_LAVA_REGISTRATIONS_PER_TICK { break; }
                if coord.y * unendlich_engine::constants::CHUNK_SIZE >= lava_max_y {
                    lava_registered.insert(coord);
                    continue;
                }
                if let Some(entry) = chunks_arc.get(&coord) {
                    let chunk = entry.value();
                    let added = register_lava_heat_sources(chunk, &heat_sources);
                    if added > 0 {
                        log::debug!("Registered {} lava heat sources from chunk {:?}", added, coord);
                    }
                } // entry dropped here, shard lock released.
                lava_registered.insert(coord);
                registered_this_tick += 1;
            }
            // Step 2: tear down sources for evicted chunks. For any
            // coord in our set NOT in chunks_arc, dereg by bbox.
            let mut to_unregister: Vec<ChunkCoord> = Vec::new();
            for coord in &lava_registered {
                if !chunks_arc.contains_key(coord) {
                    to_unregister.push(*coord);
                }
            }
            for coord in to_unregister {
                let (ox, oy, oz) = coord.block_origin();
                let cs = unendlich_engine::constants::CHUNK_SIZE as f32;
                let min = [ox as f32, oy as f32, oz as f32];
                let max = [ox as f32 + cs, oy as f32 + cs, oz as f32 + cs];
                let removed = heat_sources.remove_within_bbox(min, max);
                if removed > 0 {
                    log::debug!("Unregistered {} lava heat sources from evicted chunk {:?}",
                        removed, coord);
                }
                lava_registered.remove(&coord);
            }
            // Log lava-sync wall time. If this fires for >50ms, the main
            // thread was blocked on chunk-iter + per-chunk scan + heat-
            // source RwLock for that long.
            let lava_ms = t_lava_start.elapsed().as_secs_f64() * 1000.0;
            if lava_ms > 5.0 {
                log::info!("Lava sync: scanned {} chunks, {:.1}ms wall time",
                    registered_this_tick, lava_ms);
            }
        }

        // ---- Sync renderer mesh set with chunk manager ----
        // Compute desired chunk coords in 3D render distance.
        let player_chunk = ChunkCoord::from_block_pos(
            pos.x.floor() as i32,
            pos.y.floor() as i32,
            pos.z.floor() as i32,
        );
        let player_chunk_changed = player_chunk != last_player_chunk;
        last_player_chunk = player_chunk;
        let _ = player_chunk_changed;

        // v0.24.66-biased-walk: per-frame forward direction in world space.
        // Only used when the biased shell walk is enabled; computed
        // unconditionally because it's a few trig ops (~10ns).
        let player_forward = local_player.forward();

        let rd = runtime_render_distance();
        let rd_sq = rd * rd;

        // Inline membership check (was: build a full `desired: HashSet`
        // of every chunk within the sphere, ~1.15M entries at RD=64).
        // The HashSet was used for two things: eviction membership (a
        // single contains() per loaded chunk) and the submit candidate
        // filter (which iterated the full set every frame). Building it
        // every frame at RD=64 was cost ~1.15M inserts; iterating it
        // for submit was another ~1.15M lookups; both were dominant
        // costs in the frame loop.
        //
        // Replacement: inline closure for eviction membership (O(1)
        // per loaded chunk); shell-iterator for submit (walks outward
        // from the player in Chebyshev order and short-circuits).
        let in_render_distance = |c: ChunkCoord| -> bool {
            let dx = c.x - player_chunk.x;
            let dy = c.y - player_chunk.y;
            let dz = c.z - player_chunk.z;
            dx*dx + dy*dy + dz*dz <= rd_sq
        };

        // Evict any meshes that are no longer desired. v0.24.67: only when the
        // player crossed a chunk boundary. Evictions become necessary only when
        // the render-distance sphere moves — a stationary player strands no
        // loaded chunk out of range, and chunks meshed while stationary are
        // always submitted in-range, so there is nothing to evict on an
        // unchanged frame. This skips a per-frame ~3.5k-coord Vec allocation +
        // scan (loaded_chunk_coords) that ran every frame regardless. Eviction
        // on a real move is unbudgeted and completes in that frame (and is now
        // O(4)/chunk after the evict_chunk fix), so no multi-frame drain is lost.
        let t0 = Instant::now();
        if player_chunk_changed {
            let loaded: Vec<ChunkCoord> = renderer.loaded_chunk_coords().collect();
            // Collect the out-of-range coords, then evict them in one batch so
            // the shared octree pool lock is taken ONCE per tick (not per coord),
            // cutting contention with the mesher. Same coords evicted as before.
            let to_evict: Vec<ChunkCoord> =
                loaded.into_iter().filter(|c| !in_render_distance(*c)).collect();
            renderer.evict_chunks(&to_evict);
        }
        t_eviction_us += t0.elapsed().as_micros() as u64;

        // ---- Submit nearby chunks to the meshing pool ----
        let t_submit_start = Instant::now();
        // Any chunk that's "desired" (in render distance) AND available from
        // the chunk manager AND (NOT in the renderer OR has a stale mesh
        // version) AND not already being meshed gets a mesh request.
        //
        // The "stale mesh" check is critical for block-edit responsiveness:
        // when a block is broken, the chunk's version bumps but the OLD
        // mesh stays loaded. Without this check, we'd never resubmit and
        // the old mesh would persist forever. With it, we resubmit while
        // the old mesh remains visible, then atomically swap on upload —
        // no flash.
        //
        // PERF (v0.23.4 rewrite): the previous code iterated the full
        // `desired` HashSet (~1.15M coords at RD=64) every frame, doing
        // DashMap reads for each one. With a 64-slot mesh queue and
        // ~95% of the sphere not yet generated, that was ~1.1M wasted
        // DashMap reads per frame burning ~600ms.
        //
        // F3-STARVATION FIX (v0.23.5): the v0.23.4 shell-walk-with-budget
        // approach short-circuited on a global candidate budget. In
        // steady state, the player's immediate vicinity (shells 0..7)
        // *always* has stale-or-missing Near chunks, so the global
        // budget filled with Near every frame and the walk never
        // reached shell 9+ where Mid/Far/VeryFar candidates live.
        // Result: F3 dispatch was provably correct but never received
        // non-Near requests, so non-Near tiers had "no chunks meshed"
        // for the entire 5-minute v0.23.4 playtest.
        //
        // Fix: per-tier candidate budgets. Each LOD tier (Near, Mid,
        // Far, VeryFar) gets its own quota of submit slots. The shell
        // walk classifies each candidate by tier as it goes and only
        // accepts candidates for tiers that haven't filled their quota
        // yet. The walk continues outward until either every tier has
        // filled OR we've exhausted the sphere. This guarantees outer
        // tiers make progress regardless of how many Near candidates
        // are pending.
        //
        // Holding the Arc through to submit is safe — Arc is ref-counted,
        // so even if the chunk is evicted between filter and submit,
        // we have a valid (possibly-stale) reference. Stale chunks will
        // produce a mesh result whose version mismatches the manager's,
        // and the upload phase already discards those.
        let manager_empty = chunk_manager.loaded_count() == 0;

        // v0.24.64-submit-skip: cheap pre-check before the shell walk.
        // Three signals, all O(1) reads (atomic + atomic + struct copy):
        let cur_world_edit_tick = chunk_manager.world_edit_tick();
        let cur_loaded_count = chunk_manager.loaded_count();
        let cur_player_chunk = player_chunk;
        let should_skip_submit = submit_skip_enabled
            && submit_skip_was_idle
            && cur_world_edit_tick == submit_skip_last_world_edit_tick
            && cur_loaded_count == submit_skip_last_loaded_count
            && cur_player_chunk == submit_skip_last_player_chunk;
        if should_skip_submit {
            submit_skip_frames_skipped += 1;
            // Periodic log to confirm the optimization is firing. Quiet
            // by default: log only at 64, 256, 1024, ... (power-of-2)
            // cumulative skips so it doesn't spam.
            if submit_skip_frames_skipped >= submit_skip_last_log_frames * 2
                && submit_skip_frames_skipped >= 64
            {
                log::info!(
                    "submit_skip: skipped {} cumulative frames (world stationary)",
                    submit_skip_frames_skipped
                );
                submit_skip_last_log_frames = submit_skip_frames_skipped;
            }
        }
        if !should_skip_submit && !manager_empty {
            // Per-tier budgets. Sum is 4× MESH_QUEUE_CAPACITY = 256,
            // matching the v0.23.4 global cap, but now reserved by tier.
            // Numbers are equal across tiers as a starting point; tunable
            // later. The 64-each shape means even if Near saturates,
            // the other 192 slots are reserved for outer tiers and the
            // walk *must* reach shell 9+ to fill them.
            const NEAR_BUDGET: usize = MESH_QUEUE_CAPACITY;
            const MID_BUDGET: usize = MESH_QUEUE_CAPACITY;
            const FAR_BUDGET: usize = MESH_QUEUE_CAPACITY;
            const VFAR_BUDGET: usize = MESH_QUEUE_CAPACITY;

            // One Vec per tier so within-tier order is preserved
            // (Chebyshev shells emit roughly nearest-first within tier).
            // T7: reuse the buffers hoisted above the frame loop (clear retains
            // capacity); they were fully drained on the previous use.
            near_cands.clear();
            mid_cands.clear();
            far_cands.clear();
            vfar_cands.clear();

            'shells: for shell in 0..=rd {
                // Early-exit: if every tier has hit its budget, stop.
                // Avoids walking outer shells when we have nothing to
                // accumulate in them.
                if near_cands.len() >= NEAR_BUDGET
                    && mid_cands.len() >= MID_BUDGET
                    && far_cands.len() >= FAR_BUDGET
                    && vfar_cands.len() >= VFAR_BUDGET
                {
                    break 'shells;
                }

                // Enumerate shell surface via face-round-robin ShellWalker
                // (v0.24.0a.2). The prior triply-nested `dz/dy/dx`
                // walk emitted -z-face coords first, which caused
                // directional-slice loading under pipeline saturation.
                // ShellWalker interleaves the 6 cube faces, so when
                // a tier's budget fills mid-shell, every direction
                // has received roughly equal coords.
                //
                // v0.24.66-biased-walk: when UNENDLICH_BIASED_SHELL_WALK=1,
                // collect the shell's coords and re-order them by forward
                // dot product (chunks ahead of the player first). Per-tier
                // budget logic then fills with ahead-of-player coords
                // first, leaving back-coords for next frame's quota. When
                // the flag is off, this collapses to a direct walker
                // iteration via an intermediate Vec — adding ~1µs of
                // allocation overhead per shell (re-used across shells).
                use unendlich_engine::world::lod::ShellWalker;
                let mut shell_coords: Vec<(i32, i32, i32)> = ShellWalker::new(shell).collect();
                if biased_shell_walk_enabled {
                    shell_coords.sort_by(|a, b| {
                        let da = (a.0 as f32) * player_forward.x
                               + (a.1 as f32) * player_forward.y
                               + (a.2 as f32) * player_forward.z;
                        let db = (b.0 as f32) * player_forward.x
                               + (b.1 as f32) * player_forward.y
                               + (b.2 as f32) * player_forward.z;
                        db.partial_cmp(&da).unwrap_or(std::cmp::Ordering::Equal)
                    });
                }
                for (dx, dy, dz) in shell_coords {
                    // Sphere cull (Euclidean). Drops cube corners that
                    // exceed RD even though their Chebyshev distance
                    // is ≤ RD.
                    if dx*dx + dy*dy + dz*dz > rd_sq { continue; }

                    // Classify by LOD FIRST, before any DashMap work.
                    // If this tier is already full we can skip the
                    // candidate entirely — the cheapest possible filter.
                    let coord = ChunkCoord::new(
                        player_chunk.x + dx,
                        player_chunk.y + dy,
                        player_chunk.z + dz,
                    );
                    let lod = unendlich_engine::world::lod::desired_lod_for(coord, player_chunk, rd);
                    let tier_full = match lod {
                        LodLevel::Near    => near_cands.len() >= NEAR_BUDGET,
                        LodLevel::Mid     => mid_cands.len()  >= MID_BUDGET,
                        LodLevel::Far     => far_cands.len()  >= FAR_BUDGET,
                        LodLevel::VeryFar => vfar_cands.len() >= VFAR_BUDGET,
                    };
                    if tier_full { continue; }

                    if meshing_in_flight.contains(&coord) { continue; }

                    // Single DashMap read captures both the chunk Arc
                    // and its version.
                    let chunk = match chunk_manager.get_chunk(coord) {
                        Some(c) => c,
                        None => continue, // not yet generated
                    };
                    let chunk_ver = chunk.version;
                    // v0.24.72: use mesh_or_empty_version_for_lod so
                    // chunks that meshed to empty AT THIS LOD don't
                    // get re-enqueued every dispatch round. Before
                    // this fix, all-air chunks (sky / deep underground)
                    // re-meshed endlessly because empty meshes aren't
                    // stored in `meshes`, so `mesh_version_for` always
                    // returned None.
                    //
                    // LOD-precise: a chunk meshed empty at Near isn't
                    // necessarily empty at Far (different downsample
                    // grids see different content) — so we key the
                    // check on `(coord, lod)`, not just `coord`.
                    let renderer_ver = renderer.mesh_or_empty_version_for_lod(coord, lod);
                    let needs_submit = match renderer_ver {
                        None => true,
                        Some(r) => r != chunk_ver,
                    };
                    if !needs_submit { continue; }

                    // Append to the right tier bucket.
                    match lod {
                        LodLevel::Near    => near_cands.push((coord, chunk, chunk_ver)),
                        LodLevel::Mid     => mid_cands.push((coord, chunk, chunk_ver)),
                        LodLevel::Far     => far_cands.push((coord, chunk, chunk_ver)),
                        LodLevel::VeryFar => vfar_cands.push((coord, chunk, chunk_ver)),
                    }
                }
            }

            // v0.25-prefetch (mesh-shell-extension): also enqueue motion-
            // predicted chunks that are already LOADED but may still be BEYOND
            // the render-distance shell, so the expensive near-mesh runs ahead
            // of visibility instead of firing at shell-entry (the ~30ms
            // frame-head stall). Same per-coord filters as the shell walk
            // above; tier budgets still cap, meshing_in_flight dedups in-flight,
            // and an explicit candidate-membership check skips coords the shell
            // already queued this round (overlap when a predicted coord falls
            // inside rd).
            for coord in chunk_manager.predicted_mesh_ahead() {
                let lod = unendlich_engine::world::lod::desired_lod_for(coord, player_chunk, rd);
                let tier_full = match lod {
                    LodLevel::Near    => near_cands.len() >= NEAR_BUDGET,
                    LodLevel::Mid     => mid_cands.len()  >= MID_BUDGET,
                    LodLevel::Far     => far_cands.len()  >= FAR_BUDGET,
                    LodLevel::VeryFar => vfar_cands.len() >= VFAR_BUDGET,
                };
                if tier_full { continue; }
                if meshing_in_flight.contains(&coord) { continue; }
                let already = match lod {
                    LodLevel::Near    => near_cands.iter().any(|(c, _, _)| *c == coord),
                    LodLevel::Mid     => mid_cands.iter().any(|(c, _, _)| *c == coord),
                    LodLevel::Far     => far_cands.iter().any(|(c, _, _)| *c == coord),
                    LodLevel::VeryFar => vfar_cands.iter().any(|(c, _, _)| *c == coord),
                };
                if already { continue; }
                let chunk = match chunk_manager.get_chunk(coord) {
                    Some(c) => c,
                    None => continue,
                };
                let chunk_ver = chunk.version;
                let needs_submit = match renderer.mesh_or_empty_version_for_lod(coord, lod) {
                    None => true,
                    Some(r) => r != chunk_ver,
                };
                if !needs_submit { continue; }
                match lod {
                    LodLevel::Near    => near_cands.push((coord, chunk, chunk_ver)),
                    LodLevel::Mid     => mid_cands.push((coord, chunk, chunk_ver)),
                    LodLevel::Far     => far_cands.push((coord, chunk, chunk_ver)),
                    LodLevel::VeryFar => vfar_cands.push((coord, chunk, chunk_ver)),
                }
            }

            // Submit phase. Interleave tiers round-robin so the mesh
            // pool sees a mix of LODs even before any single tier
            // exhausts its budget. This matters for the worker pool:
            // if we submitted all-Near-then-all-Mid, workers would
            // process Near first (slowest tier, ~6ms each) and the
            // user would never see far chunks until Near drained.
            // Round-robin gives every tier roughly fair throughput.
            let world_model = chunk_manager.model_arc();
            let now_tick = chunk_manager.current_tick().raw();

            // v0.24.64-submit-skip: total candidates discovered. If
            // zero, the world is stationary from the meshing pool's
            // perspective; mark idle so next frame can skip the walk.
            let total_candidates = near_cands.len()
                + mid_cands.len()
                + far_cands.len()
                + vfar_cands.len();

            // Convert each tier vec into an iterator so we can advance
            // them independently. T7: drain (not into_iter) so the hoisted
            // buffers keep their capacity for next frame; drain still yields
            // the owned (coord, Arc, version) tuples, moving the Arc into
            // MeshRequest exactly as before.
            let mut near_iter = near_cands.drain(..);
            let mut mid_iter  = mid_cands.drain(..);
            let mut far_iter  = far_cands.drain(..);
            let mut vfar_iter = vfar_cands.drain(..);

            let mut submitted = 0usize;
            'outer: loop {
                let mut any_progress = false;
                // Order: Near first to keep the player's immediate
                // surroundings responsive, but every other position
                // gets a chance every round.
                for slot in 0..4 {
                    let next = match slot {
                        0 => near_iter.next(),
                        1 => mid_iter.next(),
                        2 => far_iter.next(),
                        _ => vfar_iter.next(),
                    };
                    if let Some((coord, chunk, version)) = next {
                        any_progress = true;
                        let lod = unendlich_engine::world::lod::desired_lod_for(coord, player_chunk, rd);
                        // v0.23.7 Stage 2b: attach a generator Arc to Far/VeryFar
                        // requests so the worker can take the reverse-regen path
                        // instead of meshing the materialised block grid.
                        // Near/Mid don't need it; carrying None avoids the clone.
                        let generator = if matches!(lod, LodLevel::Far | LodLevel::VeryFar) {
                            Some(Arc::clone(chunk_manager.generator().cpu()))
                        } else {
                            None
                        };
                        let req = MeshRequest {
                            coord,
                            chunk,
                            version_at_submit: version,
                            sky: sky_state,
                            world_model: Some(Arc::clone(&world_model)),
                            current_tick: Some(now_tick),
                            lod,
                            generator,
                            // v0.24.27-octree-mesher: ChunkManager
                            // carries the octree alongside the dense
                            // Chunk. Cheap Arc clone. If octree mode
                            // (UNENDLICH_GPU_MESHER=2) is on, the
                            // worker uses this; in other modes the
                            // field is ignored.
                            octree: chunk_manager.octree(coord),
                        };

                        // v0.24.32-pool-prewarm: in mode 2, ensure
                        // this chunk AND its 6 neighbors are resident
                        // in the shared octree pool BEFORE the mesher
                        // dispatches. The v0.24.30 cross-chunk shader
                        // reads neighbor octrees at chunk boundaries;
                        // without pre-warm, only meshed chunks were
                        // in the pool, so freshly-loaded chunks saw
                        // "no neighbor" sentinels and emitted seams
                        // anyway.
                        //
                        // upload_chunk_octree_versioned is a no-op
                        // when the chunk is already resident at the
                        // same version (just one HashMap lookup).
                        // Cost at steady state: 7 lookups per
                        // submission × 30 submissions/sec ≈ ~10 µs.
                        if gpu_mesher_mode == 2 {
                            for dc in [(0_i32, 0_i32, 0_i32),
                                       (1, 0, 0), (-1, 0, 0),
                                       (0, 1, 0), (0, -1, 0),
                                       (0, 0, 1), (0, 0, -1)] {
                                let nc = unendlich_engine::world::chunk::ChunkCoord::new(
                                    coord.x + dc.0, coord.y + dc.1, coord.z + dc.2,
                                );
                                if let Some(n_octree) = chunk_manager.octree(nc) {
                                    // The neighbor's content version
                                    // is taken from the corresponding
                                    // chunk's version. If the chunk
                                    // isn't in chunk_manager yet, we
                                    // skip — it'll get warmed when
                                    // it's submitted next.
                                    let n_ver = chunk_manager
                                        .get_chunk(nc)
                                        .map(|c| c.version)
                                        .unwrap_or(0);
                                    let _ = renderer.upload_chunk_octree_versioned(
                                        nc, &n_octree, n_ver,
                                    );
                                }
                            }
                        }

                        if mesh_pool.submit(req) {
                            meshing_in_flight.insert(coord);
                            submitted += 1;
                        } else {
                            // Pool full — drop this candidate, try again
                            // next frame. (We can't push back; the
                            // iterator already consumed it. The version
                            // check next frame will re-pick it up.)
                            break 'outer;
                        }
                    }
                }
                if !any_progress { break 'outer; }
            }
            let _ = submitted; // available for future debug logging

            // v0.24.64-submit-skip: refresh the tracking state. Mark
            // idle iff the shell walk produced ZERO candidates across
            // all tiers — that's the strictest no-op signal. If we
            // had candidates but couldn't submit (pool full), don't
            // mark idle: we MUST re-walk next frame to retry.
            submit_skip_was_idle = total_candidates == 0;
            submit_skip_last_world_edit_tick = cur_world_edit_tick;
            submit_skip_last_loaded_count = cur_loaded_count;
            submit_skip_last_player_chunk = cur_player_chunk;
        }
        t_submit_us += t_submit_start.elapsed().as_micros() as u64;

        // ---- Pull completed meshes and upload (time-budgeted) ----
        //
        // BUG NOTE (fixed v0.11.2): the previous version had a leak.
        // `drain_up_to(max_uploads_per_frame)` extracts up to N results
        // from the channel into a Vec; the for-loop calls
        // `meshing_in_flight.remove(&result.coord)` per iteration. If
        // the time-budget `break` fires, the unprocessed remainder of
        // the Vec is dropped on scope exit — those chunks were taken
        // out of the result channel but never had their `in_flight`
        // entries cleared. The submit filter
        // (`.filter(|c| !meshing_in_flight.contains(c))`) then refused
        // to re-submit them, so they sat permanently in "loaded but
        // not rendering" purgatory until a block edit cleared them.
        //
        // Symptoms: `loaded: 1396 / rendering: 422` while the player
        // walked through chunks that visibly weren't there (you could
        // pass through the air where blocks should be); breaking a
        // block on the missing chunk caused it to suddenly appear,
        // because the edit-handler removed the in_flight entry.
        //
        // Fix: drain results into a buffer, clear in_flight for all of
        // them up front, then upload up to the time budget. Any
        // unuploaded results are dropped — those chunks are now
        // unblocked and will re-submit next frame, rebuilding the
        // mesh. We waste some CPU on the redundant rebuild, but the
        // system stays live (no more permanent gaps). 16 results per
        // frame at 60 fps × ~5KB-ish is fine to drop occasionally.
        let upload_start = Instant::now();
        let mut uploaded_this_frame = 0usize;

        // v0.24.65-slack-scheduler: compute the effective upload budget
        // for THIS frame based on remaining slack to the frame target.
        // When the flag is off, this falls through to the static
        // upload_budget_ms (existing behavior).
        let effective_upload_budget_ms: f64 = if slack_scheduler_enabled {
            let frame_elapsed_ms = last_frame.elapsed().as_secs_f64() * 1000.0;
            let slack_ms = (frame_target_ms - frame_elapsed_ms).max(0.0);
            // Slack-bounded, with a minimum floor (no total starvation)
            // and a hard cap at the static upload_budget_ms (so the
            // slack scheduler can only TIGHTEN the budget, never loosen
            // it beyond the explicit operator-set cap).
            upload_budget_ms.min(slack_ms.max(upload_min_ms))
        } else {
            upload_budget_ms
        };

        // v0.24.3 upload-pressure investigation: track per-frame upload
        // BYTES alongside elapsed ms. The actual memcpy into
        // HOST_COHERENT mapped memory is the cache-coherency cost the
        // GPU pays for on its next read.
        //
        // v0.24.20-vertex-pack: vertex = 8 bytes (was 24).
        // PackedVertex format = A2B10G10R10_UNORM_PACK32 (pos) +
        // R8G8B8A8_UNORM (color). 3× reduction vs the FP32 path.
        let mut upload_bytes_this_frame: usize = 0;
        const VERTEX_BYTES: usize = 8;
        const INDEX_BYTES:  usize = 4;
        // v0.24.63-adaptive-uploads: when enabled, pull the wait_fences
        // measurement from the previous frame and let the controller
        // decide this frame's cap. When disabled, fall back to the
        // static `max_uploads_per_frame` from env / default. The
        // controller's floor / ceiling are derived from the same
        // static value, so a user who tuned MAX_UPLOADS_PER_FRAME up
        // or down sees a proportional adjustment range.
        let upload_cap = if let Some(ctrl) = adaptive_uploads.as_mut() {
            let wait = renderer.last_wait_fences_ms();
            let prev_cap = ctrl.current_cap();
            let moved = ctrl.observe(wait);
            if moved {
                log::info!(
                    "adaptive_uploads: cap {} -> {} (wait_ms_ema={:.1}, last_wait_ms={:.1}, adjustments={})",
                    prev_cap,
                    ctrl.current_cap(),
                    ctrl.rolling_wait_ms(),
                    wait,
                    ctrl.adjustment_count(),
                );
            }
            ctrl.current_cap()
        } else {
            max_uploads_per_frame
        };
        let mut drained = mesh_pool.drain_up_to(upload_cap);

        // v0.24.67-upload-priority: sort the drained results by
        // (LOD, chebyshev distance to player). Near chunks at small
        // distance land first; VeryFar at the back. Default off.
        if upload_priority_enabled {
            drained.sort_by(|a, b| {
                let a_dist = (a.coord.x - player_chunk.x).abs()
                    .max((a.coord.y - player_chunk.y).abs())
                    .max((a.coord.z - player_chunk.z).abs());
                let b_dist = (b.coord.x - player_chunk.x).abs()
                    .max((b.coord.y - player_chunk.y).abs())
                    .max((b.coord.z - player_chunk.z).abs());
                a.lod.cmp(&b.lod).then(a_dist.cmp(&b_dist))
            });
        }

        // Clear in_flight for ALL drained results before any uploads,
        // so a budget-triggered early break can't leave stragglers.
        for result in &drained {
            meshing_in_flight.remove(&result.coord);
        }
        for result in drained {
            // Skip if the chunk has been edited since the mesh was generated.
            // The chunk_manager will produce a fresh chunk; we'll re-mesh
            // next frame.
            if let Some(current) = chunk_manager.get_chunk(result.coord) {
                if current.version != result.version {
                    log::debug!("Mesh result for {:?} is stale (v{} vs current v{})",
                        result.coord, result.version, current.version);
                    continue;
                }
            } else {
                // Chunk was evicted while we were meshing it; drop the result.
                continue;
            }

            // v0.24.3: compute the bytes BEFORE the upload call (which
            // consumes result.mesh by value).
            let this_mesh_bytes =
                result.mesh.vertex_count() as usize * VERTEX_BYTES
                + result.mesh.index_count() as usize * INDEX_BYTES;

            // v0.24.37-mesh-cmp: gated GPU↔CPU equivalence check.
            // Runs CPU reference (with the same cross-chunk neighbors
            // the GPU saw via the pool) and compares vertex count +
            // index count + AABB. Logs mismatches but doesn't block
            // the upload — diagnostic-only.
            //
            // Cost: one CPU mesh per Near submission (~100 µs).
            // Gated by env var, so free when not opted-in.
            if octree_mesh_gpu_cmp
                && gpu_mesher_mode == 2
                && result.lod == LodLevel::Near
            {
                use unendlich_engine::render::gpu_meshing_octree_cpu_ref::{
                    compare_meshes, mesh_chunk_octree_cpu_reference_with_neighbors,
                    Neighbors,
                };
                if let Some(own_octree) = chunk_manager.octree(result.coord) {
                    // Resolve neighbor octrees — these MUST match what
                    // the GPU saw via the pool's pre-warm. Since pool
                    // pre-warm uses the same chunk_manager.octree call,
                    // these are the same Arc<OctreeArray>s.
                    let lookup_n = |dx: i32, dy: i32, dz: i32| {
                        let nc = unendlich_engine::world::chunk::ChunkCoord::new(
                            result.coord.x + dx, result.coord.y + dy, result.coord.z + dz,
                        );
                        chunk_manager.octree(nc)
                    };
                    let n_px = lookup_n(1, 0, 0);
                    let n_nx = lookup_n(-1, 0, 0);
                    let n_py = lookup_n(0, 1, 0);
                    let n_ny = lookup_n(0, -1, 0);
                    let n_pz = lookup_n(0, 0, 1);
                    let n_nz = lookup_n(0, 0, -1);
                    let neighbors = Neighbors {
                        px: n_px.as_deref(),
                        nx: n_nx.as_deref(),
                        py: n_py.as_deref(),
                        ny: n_ny.as_deref(),
                        pz: n_pz.as_deref(),
                        nz: n_nz.as_deref(),
                    };
                    let cpu_mesh = mesh_chunk_octree_cpu_reference_with_neighbors(
                        result.coord, &own_octree, &neighbors,
                    );
                    let cmp = compare_meshes(&result.mesh, &cpu_mesh);
                    if !cmp.matches_loosely(0.5) {
                        log::warn!(
                            "MESH_CMP_MISMATCH coord={:?} \
                             gpu_v={} cpu_v={} gpu_i={} cpu_i={} \
                             gpu_aabb={:?} cpu_aabb={:?}",
                            result.coord,
                            cmp.vertex_count_gpu, cmp.vertex_count_cpu,
                            cmp.index_count_gpu, cmp.index_count_cpu,
                            cmp.aabb_gpu, cmp.aabb_cpu,
                        );
                    }
                }
            }

            // v0.25-prefetch (T05-R27): split the upload by visibility. A chunk
            // the camera will draw this frame MUST flush immediately (the draw
            // binds every resident mesh — a deferred visible slot draws stale
            // memory). A prefetched, not-yet-visible chunk was meshed ahead during
            // slack; its copy can ride the budgeted deferrable lane so the burst
            // doesn't stall the frame head. `chunk_visible` uses the same frustum
            // as the cull with a conservative chunk AABB, so it only ever defers a
            // chunk that is definitely off-screen.
            let visible_now = !prefetch_upload_enabled
                || renderer.chunk_visible(result.coord, &local_player.view);

            if !visible_now {
                // Ahead/prefetched. In GPU-mesher mode 2 the compressed/regen GPU
                // paths exist, so consult `upload_repr::classify`: an untouched,
                // far-enough chunk is Procedural — the GPU regenerates it from
                // seed, needing no resident bytes, so skip the mesh upload
                // entirely (the strongest prefetch win). It self-heals if the
                // chunk later comes near (re-meshed Octree, uploaded immediately).
                if gpu_mesher_mode == 2 {
                    if let Some(chunk) = chunk_manager.get_chunk(result.coord) {
                        use unendlich_engine::world::surface_run::SurfaceRunChunk;
                        use unendlich_engine::world::upload_repr::{
                            classify, ChunkStats, UploadPolicy, UploadRepr,
                        };
                        use std::cell::RefCell;
                        // Reuse a per-thread scratch SurfaceRunChunk across the
                        // prefetch burst: clear+refill instead of allocating two
                        // fresh Vecs per prefetched chunk. Result is identical.
                        thread_local! {
                            static SRUN_SCRATCH: RefCell<SurfaceRunChunk> =
                                RefCell::new(SurfaceRunChunk::empty());
                        }
                        let stats = SRUN_SCRATCH.with(|cell| {
                            let mut srun = cell.borrow_mut();
                            srun.from_chunk_into(&chunk);
                            ChunkStats::from_surface_run(&*srun, result.lod, chunk.dirty)
                        });
                        if classify(&stats, &UploadPolicy::default()) == UploadRepr::Procedural {
                            log::debug!(
                                "prefetch: skip resident upload for {:?} (Procedural — GPU regenerates)",
                                result.coord
                            );
                            continue;
                        }
                    }
                }
                if let Err(e) = renderer.upload_prebuilt_mesh_at_lod_deferrable(
                    &ctx, result.coord, result.lod, result.mesh, result.version,
                ) {
                    log::warn!("Deferrable upload failed for {:?} at {:?}: {}", result.coord, result.lod, e);
                    continue;
                }
            } else if let Err(e) = renderer.upload_prebuilt_mesh_at_lod(&ctx, result.coord, result.lod, result.mesh, result.version) {
                log::warn!("Upload failed for {:?} at {:?}: {}", result.coord, result.lod, e);
                continue;
            }
            uploaded_this_frame += 1;
            // v0.25-prefetch (T05-R27): only IMMEDIATE (visible) uploads spend
            // the per-frame upload-bytes budget that gates the loop below and
            // the adaptive controller. Deferrable copies are spread by the mesh
            // pool's own `UNENDLICH_MESH_DEFER_BUDGET_BYTES` lane, so counting
            // them here would let an ahead-of-camera burst starve visible
            // uploads for the rest of the frame.
            if visible_now {
                upload_bytes_this_frame += this_mesh_bytes;
            }

            // v0.24.26-octree-on-gpu: after a successful mesh upload,
            // mirror the chunk's octree onto the GPU. No production
            // consumer reads this yet — the substrate has to exist
            // before consumers migrate. Failures are non-fatal: log
            // and continue. ChunkManager carries the octree alongside
            // the dense Chunk (v0.24.23-octree-propagation); if it's
            // missing here, this is a streaming race and the next
            // mesh result for the same chunk will retry the octree.
            //
            // v0.24.30-shared-octree: when running in mode 2, the
            // `OctreeGpuMesher` already uploaded the chunk's octree
            // to the shared pool during dispatch (it has to, in
            // order to read it). Re-uploading here is redundant and
            // costs allocator churn. Skip when mode 2 is active.
            if gpu_mesher_mode != 2 {
                if let Some(octree) = chunk_manager.octree(result.coord) {
                    if let Err(e) = renderer.upload_chunk_octree(result.coord, &octree) {
                        log::warn!("Octree upload failed for {:?}: {}", result.coord, e);
                    }
                }
            }

            // Stop if we've exhausted EITHER the time budget OR the
            // bytes budget. Remaining drained-but-unuploaded results
            // are dropped — their in_flight entries already cleared
            // above, so they'll re-submit next frame.
            //
            // v0.24.65-slack-scheduler: `effective_upload_budget_ms`
            // is the slack-bounded budget when SLACK_SCHEDULER=1,
            // else equals the static `upload_budget_ms`.
            let elapsed_ms = upload_start.elapsed().as_secs_f64() * 1000.0;
            if elapsed_ms > effective_upload_budget_ms { break; }
            if upload_bytes_this_frame >= upload_bytes_budget { break; }
        }
        let _ = uploaded_this_frame;
        t_upload_us += upload_start.elapsed().as_micros() as u64;

        // v0.24.3 upload-pressure event: when per-frame upload bytes
        // exceeds the threshold, emit an event so we can correlate
        // high-upload frames with the GPU spikes from chug-debug-2.
        //
        // The hypothesis: heavy CPU memcpy into the persistent-mapped
        // pool buffer triggers cache-coherency cost the GPU pays on
        // its next read. Both shadow and main read the same buffer, so
        // the cost shows up as correlated spikes (matching chug-debug-2's
        // shadow+main-together pattern).
        //
        // Frame_count is the monotonic wall-clock frame counter; events
        // can be ordered chronologically in events.jsonl and clustering
        // with gpu_spike/wait_fences_spike events identifies whether
        // upload pressure is the chug cause.
        if upload_bytes_this_frame >= UPLOAD_PRESSURE_EVENT_THRESHOLD_BYTES {
            let elapsed_ms = upload_start.elapsed().as_secs_f64() * 1000.0;
            unendlich_shared::events::record(
                "upload_pressure",
                serde_json::json!({
                    "frame": frame_count,
                    "upload_bytes": upload_bytes_this_frame,
                    "upload_chunks": uploaded_this_frame,
                    "upload_ms": elapsed_ms,
                }),
            );
        }

        // ---- Draw ----
        // Multiplayer-prep: render through the LocalPlayer's View
        // rather than the renderer's internal camera. The renderer's
        // `self.camera` field is no longer used by this bin (it's
        // kept for backward compat with other bins).
        //
        // Sky-tracking clear color: each frame, update the renderer's
        // clear color from the current sky state. The void beyond
        // loaded chunks fades into the same atmospheric color as the
        // visible horizon — sunset orange, midnight navy, noon cyan.
        // This also softens the "floating green island" perception
        // during chunk load (chunks at render-distance edge fade
        // into the void rather than appearing in a hard background).
        renderer.clear_color = if weather_enabled {
            unendlich_engine::weather::compose(&sky_state, weather_state.params()).sky_color
        } else {
            sky_state.sky_color_rgba()
        };

        // Build per-frame atmosphere uniform from the simulation state.
        // Sun direction & intensity & sky color: animated by the Sky
        // module across the day/night cycle. Climate: sampled at the
        // player's position from the world model. Wind: sampled at the
        // player's position from the wind field.
        //
        // The bin owns this bridging because the renderer doesn't (and
        // shouldn't) reach into world_model / wind / sky directly. It
        // takes parameters; the bin computes them.
        let mut atmos = {
            let pos = local_player.view.position;
            let world_time = sky_state.time_seconds as f32;
            let model = chunk_manager.model_arc();
            let temp  = model.temperature(pos.x as f64, pos.y as f64, pos.z as f64, 0.0) as f32;
            let humid = model.humidity(pos.x as f64, pos.y as f64, pos.z as f64, 0.0) as f32;
            let (wx, wz, wmag) = unendlich_engine::wind::wind_at(
                pos.x as f64, pos.y as f64, pos.z as f64, world_time as f64,
            );
            let sun_dir = sky_state.sun_direction();
            let mut sun_intensity = sky_state.directional_intensity();
            let mut ambient = sky_state.ambient_color();
            let mut sky_rgba = sky_state.sky_color_rgba();
            let mut fog_density = 0.012_f32;
            // Weather (opt-in) reshapes the atmosphere: greyed sky, dimmer sun,
            // cooler ambient, thicker fog — composed from the same Sky so the
            // day/night cycle still drives the base and weather only modulates.
            if weather_enabled {
                let wp = weather_state.params();
                let atmo = unendlich_engine::weather::compose(&sky_state, wp);
                sun_intensity = atmo.sun_intensity;
                ambient = atmo.ambient_color;
                sky_rgba = atmo.sky_color;
                // wp.fog_density() is [0,1]; scale into the shader's small
                // density units (0.045 ≈ a heavy-fog half-fade near ~15 blocks
                // stacked on the humidity base).
                fog_density += wp.fog_density() * 0.045;
            }
            // Load-adaptive fog (opt-in): while the world is still streaming,
            // thicken fog so the render-distance edge — where chunks haven't
            // arrived yet — hides in haze instead of showing holes. Relaxes to
            // exactly 1.0 (no change) once the generate queue drains, so the
            // settled look is unaffected.
            if load_fog_enabled {
                fog_density *= unendlich_engine::visual_fade::load_fog_boost(
                    chunk_manager.pending_count(),
                    16,   // settle: at/below this many pending → no boost
                    512,  // full: at/above this → max boost (teleport/initial load)
                    2.5,  // up to 3.5× fog at peak loading
                );
            }
            // Diagnostic override: kill fog entirely (UNENDLICH_NO_FOG=1).
            if no_fog_enabled {
                fog_density = 0.0;
            }
            // Sun color: warm at low intensity (sunrise/sunset), bright
            // white at noon. Cheap proxy — Sky doesn't expose this yet
            // but should eventually.
            let sun_color = [
                1.0,
                0.85 + 0.15 * sun_intensity,         // greener-white at noon
                0.65 + 0.30 * sun_intensity,         // bluer at noon, oranger at sunset
            ];
            unendlich_engine::render::world_renderer::AtmosphereParams {
                sun_dir,
                sun_intensity,
                sun_color,
                sky_color: [sky_rgba[0], sky_rgba[1], sky_rgba[2]],
                ambient_color: ambient,
                fog_density,
                humidity: humid,
                temperature: temp,
                time_of_day: world_time,
                wind_xz: [wx, wz],
                wind_magnitude: wmag,
            }
        };

        // Apply the selected debug "look" (atmosphere preset) + fog toggle.
        {
            // Looks operate on a decoupled LookParams now (debug_settings no
            // longer references the renderer's AtmosphereParams, so its layout
            // can move freely). Copy the five fields in, apply, copy back.
            let mut lp = unendlich_engine::debug_settings::LookParams {
                fog_density: atmos.fog_density,
                sun_intensity: atmos.sun_intensity,
                sun_color: atmos.sun_color,
                sky_color: atmos.sky_color,
                ambient_color: atmos.ambient_color,
            };
            unendlich_engine::debug_settings::apply_look(&mut lp);
            atmos.fog_density = lp.fog_density;
            atmos.sun_intensity = lp.sun_intensity;
            atmos.sun_color = lp.sun_color;
            atmos.sky_color = lp.sky_color;
            atmos.ambient_color = lp.ambient_color;
        }

        // v0.24.2c.10: late input poll. Drain any glfw events that
        // arrived during the CPU-side state management above (chunk
        // mgmt, mesh updates, physics tick, etc.). Mouse motion is
        // the most latency-sensitive input — applying late-poll
        // mouse delta here means camera rotation reflects the very
        // latest pointer position when we record this frame's draw
        // commands. Result: input-to-photon latency for camera is
        // minimised.
        //
        // Other late events (esc, click, resize) are also drained
        // here but they'll be acted on next frame — that's fine
        // because they're sparse and a single-frame delay isn't
        // perceptible for those.
        glfw.poll_events();
        for (_, event) in glfw::flush_messages(&events) {
            // Late drain: route through handle_event_late so discrete key /
            // button PRESS edges caught here carry into next frame's
            // just-pressed set instead of being wiped by end_frame (the
            // "eaten esc / jump / flight" bug).
            input.handle_event_late(&event);
        }
        if mouse_captured && input.cursor_delta != (0.0, 0.0) {
            local_player.apply_mouse(input.cursor_delta.0, input.cursor_delta.1);
            input.cursor_delta = (0.0, 0.0);
            // local_player.view is rebuilt by apply_mouse, so the
            // draw_frame call below uses the freshest possible
            // camera matrix.
        }

        let t_draw_start = Instant::now();
        // v0.24.18: tag this as WaitFence because in steady state and
        // during chug both, the dominant time inside draw_frame is
        // vkWaitForFences (per cpu_zones output). When the probe is
        // sampled mid-draw it'll almost always be inside that wait.
        // True for the chug investigation we care about; if/when we
        // get sub-call probe resolution we'd split this further.
        _render_probe.set(unendlich_engine::thread_probe::Activity::WaitingForFence);
        #[cfg(feature = "ui-overlay")]
        {
            // Forward the live cursor + button state to egui. Active only in
            // panel mode; in game mode the cursor is captured by GLFW so its
            // position is frozen and useless to egui.
            renderer.ui_pointer_pos = (input.cursor_pos.0 as f32, input.cursor_pos.1 as f32);
            // Button level from InputCache, which BOTH drains update — so a
            // release landing in the late drain still clears it. (The old
            // `ui_mouse_down` was set only in the top drain, leaving egui stuck
            // "pressed" on late releases → clicks became window drags.)
            renderer.ui_pointer_down = input.is_mouse_button_pressed(MouseButtonLeft);
            // Clicks whose press+release collapse into one frame (low fps)
            // produce no level edge; forward them as discrete clicks.
            renderer.ui_click_pending =
                (input.is_mouse_button_just_pressed(MouseButtonLeft)
                    && !input.is_mouse_button_pressed(MouseButtonLeft)) as u32;
            renderer.ui_pointer_active = ui_panel_open || renderer.settings_open;
        }
        // Bake this frame's entities + creatures and upload before drawing.
        // Tick the sentinel flock against the player; log any reveals (throttled).
        {
            use unendlich_engine::sim::fixed::{Fixed, Vec3};
            let pp = local_player.view.position;
            let player = Vec3::new(
                Fixed::from_f64(pp[0] as f64),
                Fixed::from_f64(pp[1] as f64),
                Fixed::from_f64(pp[2] as f64),
            );
            // A still player is near-silent (you can sneak — even behind a bird,
            // since hearing is omnidirectional); moving makes audible footsteps.
            let cur_cell = [pp[0].floor() as i32, pp[1].floor() as i32, pp[2].floor() as i32];
            let moving = last_player_cell.map_or(true, |c| c != cur_cell);
            last_player_cell = Some(cur_cell);
            let loudness = if moving { 80 } else { 10 };
            // Perceive against the real terrain now: a Späher no longer sees the
            // player through a hill (ChunkManager implements WorldQuery).
            let reveals = sentinel_flock.tick(&chunk_manager, &[(player, loudness)], player);
            if !reveals.is_empty() && frame_count.saturating_sub(last_reveal_log_frame) >= 30 {
                last_reveal_log_frame = frame_count;
                for (idx, alarm) in &reveals {
                    log::info!(
                        "Späher #{idx} reveals the player — {:?} via {:?}",
                        alarm.posture,
                        alarm.via
                    );
                }
            }
            // Periodic readout of the flock's sustained state (so alarm holding
            // while you linger — and decaying as you leave — is visible).
            if frame_count % 120 == 0 {
                let postures: Vec<_> =
                    sentinel_flock.members().iter().map(|m| m.agent.sentinel.posture).collect();
                log::info!("Sentinel flock postures: {:?}", postures);
            }
        }
        {
            use unendlich_engine::world::entity::{
                creature_to_vertices, default_color_for_kind, mobs_to_vertices, snapshot_to_vertices,
                EntityRenderSnapshot,
            };
            let snap = EntityRenderSnapshot::capture(&entity_store);
            let (mut everts, mut eindices) = snapshot_to_vertices(&snap, default_color_for_kind);
            // Append each multi-part creature, rebasing its indices onto the
            // shared vertex buffer.
            for c in &test_creatures {
                let (cv, ci) = creature_to_vertices(c);
                let base = everts.len() as u32;
                everts.extend(cv);
                eindices.extend(ci.iter().map(|i| i + base));
            }
            // Append the streamed ambient roamers.
            for r in &roamers {
                let cr = &r.creature;
                let (cv, ci) = creature_to_vertices(cr);
                let base = everts.len() as u32;
                everts.extend(cv);
                eindices.extend(ci.iter().map(|i| i + base));
            }
            // Append sim mobs via the bridge.
            {
                let (mv, mi) = mobs_to_vertices(&test_mobs);
                let base = everts.len() as u32;
                everts.extend(mv);
                eindices.extend(mi.iter().map(|i| i + base));
            }
            renderer.upload_entities(&ctx, &everts, &eindices);
        }
        let draw_result = renderer.draw_frame_with_view_and_atmos(
            &ctx, &local_player.view, &atmos,
        );
        _render_probe.set(unendlich_engine::thread_probe::Activity::Working);
        t_draw_us += t_draw_start.elapsed().as_micros() as u64;
        match draw_result {
            Ok(true) => {
                frame_count += 1;
                if frame_count % 30 == 0 {
                    let pp = local_player.view.position;
                    let humid = chunk_manager
                        .model_arc()
                        .humidity(pp.x as f64, pp.y as f64, pp.z as f64, 0.0) as f32;
                    frame_fog_sigma = if no_fog_enabled {
                        0.0
                    } else {
                        unendlich_engine::sim::fog_model::shader_fog_density(0.012, humid)
                    };
                }
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

        // ---- End of frame ----
        input.end_frame();

        if last_log.elapsed().as_secs() >= 1 {
            // Compute frame-time percentiles. Skip the first frame (cold).
            let mut sorted = frame_times_ms.clone();
            sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let n = sorted.len();
            let p50 = if n > 0 { sorted[n / 2] } else { 0.0 };
            let p99 = if n > 0 { sorted[(n * 99 / 100).min(n - 1)] } else { 0.0 };
            let max = sorted.last().copied().unwrap_or(0.0);

            log::info!(
                "FPS: {:>4}  frame: p50={:.1}ms p99={:.1}ms max={:.1}ms  pos: ({:>6.0},{:>6.0},{:>6.0})  loaded: {}  gen_pending: {}  rendering: {}  upload_fails: {}  mesh_q: {}/{}  fires: {}",
                frame_count,
                p50, p99, max,
                pos.x, pos.y, pos.z,
                chunk_manager.loaded_count(),
                chunk_manager.pending_count(),
                already_in_renderer_count(&renderer),
                renderer.upload_fail_count(),
                meshing_in_flight.len(),
                mesh_pool.pending_results(),
                heat_sources.len(),
            );
            // v0.24.18: push these same values into the thread probe's
            // engine_state pseudo-thread so the sampler's per-100ms
            // rows carry the fps/queue context. Lets the analysis tool
            // slice on chug-vs-not without parsing engine stdout.
            unendlich_engine::thread_probe::engine_state_update(
                (frame_count as u32) * 10, // fps in tenths, matches threshold
                (p50 * 1000.0) as u32,
                0, // wait_fences pulled from renderer telemetry is per-window aggregate;
                   // would require new accessor. Skip for now — chug detector still works
                   // on fps alone.
                meshing_in_flight.len() as u32,
                mesh_pool.pending_results() as u32,
                chunk_manager.pending_count() as u32,
                already_in_renderer_count(&renderer) as u32,
                chunk_manager.loaded_count() as u32,
            );
            // v0.24.2c.7: snapshot loaded/rendering counts for the
            // run-summary block. One sample per FPS-log window (= ~1s).
            loaded_samples.push(chunk_manager.loaded_count() as u64);
            rendering_samples.push(already_in_renderer_count(&renderer) as u64);
            tick_apply_us_samples.push(chunk_manager.last_apply_us());
            tick_apply_chunks_samples.push(chunk_manager.last_apply_chunks() as u64);

            // v0.24.2c.8: structured event for retrospective analysis.
            // One record per FPS-log window with frame-time percentiles
            // and workload counters. Machine-parseable counterpart of
            // the human-readable FPS line above.
            //
            // c.12-e: tagged `mode: "steady"` to distinguish from the
            // per-frame `mode: "boot"` events emitted during warmup.
            unendlich_shared::events::record("frame_window", serde_json::json!({
                "mode": "steady",
                "window_secs": last_log.elapsed().as_secs_f32(),
                "frames": frame_count,
                "frame_p50_ms": p50,
                "frame_p99_ms": p99,
                "frame_max_ms": max,
                "loaded": chunk_manager.loaded_count(),
                "rendering": already_in_renderer_count(&renderer),
                "gen_pending": chunk_manager.pending_count(),
                "mesh_q": mesh_pool.pending_results(),
                "upload_fails": renderer.upload_fail_count(),
            }));
            // CPU streaming-loop timing for the just-elapsed 1s window.
            log::info!(
                "  cpu: cloud_remesh={:.1}ms evict={:.1}ms submit={:.1}ms upload={:.1}ms mgr_update={:.1}ms tick_sim={:.1}ms draw={:.1}ms",
                t_cloud_remesh_us as f64 / 1000.0,
                t_eviction_us as f64 / 1000.0,
                t_submit_us as f64 / 1000.0,
                t_upload_us as f64 / 1000.0,
                t_manager_update_us as f64 / 1000.0,
                t_tick_sim_us as f64 / 1000.0,
                t_draw_us as f64 / 1000.0,
            );
            // v0.23.4: mgr_update sub-zone breakdown. Splits the
            // mgr_update line above into the three phases inside
            // chunk_manager.update(). At RD=64 the playtest showed
            // mgr_update ≈ 400-600ms; this attribution tells us
            // whether request_visible (O(rd³) sphere walk),
            // evict_distant (O(loaded) scan), or drain_completed
            // (O(generated_this_frame)) is the hot spot.
            {
                let drain_us = unendlich_engine::world::chunk_manager::MGR_DRAIN_US
                    .swap(0, std::sync::atomic::Ordering::Relaxed);
                let evict_us = unendlich_engine::world::chunk_manager::MGR_EVICT_US
                    .swap(0, std::sync::atomic::Ordering::Relaxed);
                let request_us = unendlich_engine::world::chunk_manager::MGR_REQUEST_US
                    .swap(0, std::sync::atomic::Ordering::Relaxed);
                log::info!(
                    "  mgr_update breakdown: drain={:.1}ms evict={:.1}ms request_visible={:.1}ms",
                    drain_us as f64 / 1000.0,
                    evict_us as f64 / 1000.0,
                    request_us as f64 / 1000.0,
                );
            }
            // v0.21.0: per-pass GPU time from Vulkan timestamp queries.
            // `draw=` above is wall-clock CPU time INSIDE draw_frame
            // (includes vsync wait, queue submit, etc.); `gpu:` here is
            // actual GPU time per pass. If `gpu: total` ≈ `cpu: draw`
            // we're GPU-bound; if much less, we're vsync-blocked.
            log::info!("  {}", renderer.flush_gpu_timer_log());
            // v0.23.0: CPU per-zone breakdown for the render thread.
            // Surfaces wall-clock time spent in wait_fences, acquire,
            // record, submit, present. Catches the gap between
            // `cpu: draw` (which is the whole draw_frame call) and
            // `gpu: total` (just GPU work) — when those diverge,
            // this line tells us which CPU zone is eating the time.
            log::info!("{}", renderer.flush_cpu_timer_log());
            // v0.23.1: per-LOD mesher worker timing for this 1s window.
            // Tests the v0.23.0 hypothesis that "meshing is slow" by
            // attributing CPU time to the worker dispatch by LOD tier.
            // High mean × high count = sustained pressure; high mean ×
            // low count = single chunk costing a lot.
            log::info!("{}", mesh_pool.flush_mesher_log());
            // v0.24.19: mesh-cache hit/miss telemetry (empty when
            // UNENDLICH_MESH_CACHE_SIZE=0; appears as `cache: hits=N
            // misses=M size=S/cap=C` otherwise).
            {
                let cache_log = renderer.mesh_cache_log();
                if !cache_log.is_empty() {
                    log::info!("  {}", cache_log);
                }
            }
            // Per-handler tick breakdown for the most recent tick batch
            // (single-batch totals, not a 1s window).
            {
                let mut breakdown = String::new();
                for (name, us) in chunk_manager.last_handler_us() {
                    if !breakdown.is_empty() { breakdown.push(' '); }
                    breakdown.push_str(&format!("{}={:.1}ms",
                        name, *us as f64 / 1000.0));
                }
                let apply_us = chunk_manager.last_apply_us();
                let apply_chunks = chunk_manager.last_apply_chunks();
                log::info!(
                    "  tick: {}  apply={:.1}ms (chunks={})",
                    breakdown,
                    apply_us as f64 / 1000.0,
                    apply_chunks,
                );
            }
            // Input + climate: emit only when something changed.
            // Heartbeat ("system alive") is the FPS line above; this
            // line is only diagnostic when input is happening or
            // climate has shifted. Suppressing duplicates cuts the
            // log volume on idle sessions roughly in half.
            {
                let model = chunk_manager.model_arc();
                let temp = model.temperature(pos.x as f64, pos.y as f64, pos.z as f64, 0.0) as f32;
                let humid = model.humidity(pos.x as f64, pos.y as f64, pos.z as f64, 0.0) as f32;
                let focus_now = window.is_focused();
                let cap_now = mouse_captured;

                let any_input = events_received > 0;
                let climate_shifted = (temp - last_log_temp).abs() > 0.005
                                   || (humid - last_log_humid).abs() > 0.005;
                let mode_shifted = focus_now != last_log_focus
                                || cap_now != last_log_capture;

                if any_input || climate_shifted || mode_shifted {
                    log::info!(
                        "  input: events={} (k={} m={} c={}) focus={} cap={}  climate: T={:.2} H={:.2}",
                        events_received, key_events, mouse_events, cursor_events,
                        focus_now, cap_now,
                        temp, humid,
                    );
                    last_log_temp = temp;
                    last_log_humid = humid;
                    last_log_focus = focus_now;
                    last_log_capture = cap_now;
                }
            }
            events_received = 0;
            key_events = 0;
            mouse_events = 0;
            cursor_events = 0;
            t_cloud_remesh_us = 0;
            t_eviction_us = 0;
            t_submit_us = 0;
            t_upload_us = 0;
            t_manager_update_us = 0;
            t_tick_sim_us = 0;
            t_draw_us = 0;
            frame_count = 0;
            frame_times_ms.clear();
            last_log = Instant::now();
        }
    }

    let elapsed = start.elapsed();
    let saved = chunk_manager.save_all();
    if saved > 0 {
        log::info!("Saved {} edited chunks before shutdown", saved);
    }
    log::info!("Shutting down after {:.1}s", elapsed.as_secs_f32());

    // v0.21.3: dump the lifetime per-zone GPU timer stats. One line
    // per zone with count/mean/stddev/min/max. Useful for offline
    // comparison across runs (e.g., before vs after a perf change).
    // v0.24.2c.2: now also includes percentiles, box plots, and
    // over-budget reporting.
    let gpu_summary = renderer.flush_gpu_timer_summary();
    log::info!("{}", gpu_summary);
    // v0.23.0: CPU timer lifetime summary.
    let cpu_summary = renderer.flush_cpu_timer_summary();
    log::info!("{}", cpu_summary);
    // v0.23.1: lifetime per-LOD mesher worker stats.
    let mesher_summary = mesh_pool.flush_mesher_summary();
    log::info!("{}", mesher_summary);
    // v0.23.3: env-var flag values in effect.
    let flags_summary = unendlich_engine::runtime_flags::flags_summary("unendlich-world");
    log::info!("{}", flags_summary);

    // v0.24.2c.7: workload-shape summary. Sampled once per FPS-log
    // window during the run. Helps distinguish perf differences
    // caused by workload (more chunks loaded → more rendering work)
    // from perf differences caused by code changes — particularly
    // important when comparing chained runs at the same env-vars.
    let workload_summary = unendlich_engine::debug_scene::format_workload_summary(&loaded_samples, &rendering_samples);    log::info!("{}", workload_summary);

    // v0.24.NEXT+12-tick-telemetry: per-tick chunk-apply timing. The
    // loading-wave hitch (a single tick applying hundreds of chunks at
    // 100+ms) is the dominant source of frame-time spikes once memory is
    // off the critical path. Surfaced here so it's trackable in the
    // dashboard radar instead of only the transient console line.
    let tick_apply_summary = {
        let mut us = tick_apply_us_samples.clone();
        us.sort_unstable();
        let mut ch = tick_apply_chunks_samples.clone();
        ch.sort_unstable();
        let pct = |s: &[u64], p: usize| -> u64 {
            if s.is_empty() { 0 } else { s[((p * (s.len() - 1)) / 100).min(s.len() - 1)] }
        };
        format!(
            "tick_apply: n={} p50_ms={:.1} p90_ms={:.1} p99_ms={:.1} max_ms={:.1} max_chunks={}",
            us.len(),
            pct(&us, 50) as f64 / 1000.0,
            pct(&us, 90) as f64 / 1000.0,
            pct(&us, 99) as f64 / 1000.0,
            us.last().copied().unwrap_or(0) as f64 / 1000.0,
            ch.last().copied().unwrap_or(0),
        )
    };
    log::info!("{}", tick_apply_summary);

    log::info!("remesh_gate skipped={} [UNENDLICH_REMESH_GATE]", chunk_manager.remesh_skipped());

    // v0.24.NEXT+14-mesh-diag: empty-mesh counts by LOD. A chunk that meshed
    // to nothing lands here; legitimate for sky (all-air) but a large count at
    // near/mid (visible solid terrain) flags the "mesh not showing on GPU"
    // failure. near is CPU-meshed, mid/far/vfar are GPU-meshed under
    // UNENDLICH_GPU_MESHER=2 — so a lopsided GPU-bucket count points the
    // finger at the GPU mesher dropping geometry vs the CPU path.
    let empty_mesh_summary = {
        let c = renderer.empty_mesh_counts_by_lod();
        // v0.30: the old "(near=CPU…)" note was a static label, not a measurement —
        // under GPU_MESHER=2 Near forwards to the GPU when it has an octree. Report
        // the real octree-absent CPU fallback count instead (the `near` spike source).
        format!("empty_meshes: near={} mid={} far={} vfar={} | near_cpu_fallback={} (Near→GPU when octree present; this counts octree-absent CPU greedy meshes under GPU_MESHER=2)",
            c[0], c[1], c[2], c[3],
            unendlich_engine::render::meshing_pool::near_cpu_fallback_count())
    };
    log::info!("{}", empty_mesh_summary);

    // v0.24.NEXT+16-warn-snipe: list which WARN/ERROR sites fired this run
    // (UNENDLICH_WARN_SNIPE=1). Empty string when disabled, so this is a
    // silent no-op in the default config.
    let warn_summary = unendlich_engine::warn_snipe::summary();
    if !warn_summary.is_empty() {
        log::info!("{}", warn_summary);
    }

    // v0.24.26-octree-on-gpu: optional GPU/CPU octree equivalence
    // check. When `UNENDLICH_OCTREE_VERIFY=1`, dispatch a compute
    // probe that samples every loaded chunk's GPU-resident octree
    // at deterministic positions and asserts every result equals
    // the CPU `OctreeArray::sample`. This is the falsification
    // gate for `octree_buffer.rs` + `octree_sample.glsl` + the
    // upload path. Run failures are logged loudly so CI can
    // detect them; a non-zero `mismatched` count means the wire
    // format has drifted and downstream consumer migrations would
    // silently corrupt world reads.
    let octree_verify = std::env::var("UNENDLICH_OCTREE_VERIFY")
        .ok().and_then(|v| v.parse::<u32>().ok()).unwrap_or(0) > 0;
    if octree_verify {
        log::info!("UNENDLICH_OCTREE_VERIFY=1: running GPU↔CPU octree probe…");
        match renderer.run_octree_verify(&ctx, &chunk_manager, 24) {
            Ok(report) => {
                log::info!(
                    "octree verify: queries={} matched={} mismatched={} chunks_examined={} chunks_skipped={}",
                    report.queries, report.matched, report.mismatched,
                    report.chunks_examined, report.chunks_skipped,
                );
                if let Some((coord, pos, gpu, cpu)) = report.first_mismatch {
                    log::error!(
                        "octree verify FAIL: first mismatch at chunk={:?} pos=({},{},{}) gpu={} cpu={}",
                        coord, pos[0], pos[1], pos[2], gpu, cpu,
                    );
                }
                if report.mismatched > 0 {
                    log::error!("octree verify FAILED with {} mismatched samples", report.mismatched);
                }
            }
            Err(e) => log::error!("octree verify dispatch failed: {}", e),
        }
    }

    // v0.24.2c.2: append the run summary block to run_summaries.txt
    // in the project root (cwd). Every run accumulates one block
    // there so we can A/B perf changes across runs without scraping
    // terminal scrollback. Failures are logged but don't block
    // shutdown — the in-terminal log already has the same content.
    let cmdline = std::env::args().collect::<Vec<_>>().join(" ");
    // v0.24.NEXT-mem-telemetry: a memory-pressure line in the run summary so the
    // dashboard (and a plain `grep`) can see GPU octree-pool utilization and how
    // many chunks fell back to the CPU mesher under OOM this run. A non-zero
    // gpu_oom_fallback is the fingerprint of the stripe (docs/MEMORY-PRESSURE.md).
    let lc = renderer.lod_chunk_counts();
    let mem_summary = format!(
        "memory: octree_bytes={} octree_capacity={} octree_chunks={} indirect_slots_cap={} gpu_oom_fallback={} lod_near={} lod_mid={} lod_far={} lod_vfar={}",
        renderer.octree_allocated_bytes(),
        renderer.octree_pool_capacity(),
        renderer.octree_live_chunks(),
        renderer.indirect_capacity(),
        unendlich_engine::render::meshing_pool::gpu_oom_fallback_count(),
        lc[0], lc[1], lc[2], lc[3],
    );
    log::info!("{}", mem_summary);
    // SurfaceRun compression stats — measures how much memory the live
    // Vec<BlockId> storage would shrink under SurfaceRun encoding. Numbers
    // here decide whether the live-storage swap is worth the architectural
    // lift; see docs/FOLLOWUPS.md for the rest of the compression track.
    let cs = chunk_manager.compression_stats();
    let comp_summary = format!(
        "compression: chunks={} raw_bytes={} surface_run_bytes={} ratio={:.2} worst_col_runs={}",
        cs.chunks, cs.raw_bytes, cs.surface_run_bytes, cs.ratio(), cs.worst_col_runs,
    );
    log::info!("{}", comp_summary);
    // Per-chunk averages tell us what the GPU-side encoding choice costs vs
    // what SurfaceRun would cost. Octree currently bloats surface chunks
    // 5-10x (sparse-air optimization fails on a surface layer); if the same
    // chunks were SurfaceRun-encoded GPU-side we'd fit ~30x more in the same
    // pool. Use these to size UNENDLICH_OCTREE_POOL_MB.
    let octree_used = renderer.octree_allocated_bytes();
    let octree_cap = renderer.octree_pool_capacity();
    let octree_chunks = renderer.octree_live_chunks() as u64;
    let octree_largest = renderer.octree_largest_free_span();
    let octree_per_chunk = if octree_chunks > 0 { octree_used / octree_chunks } else { 0 };
    let srun_per_chunk = if cs.chunks > 0 { cs.surface_run_bytes / cs.chunks } else { 0 };
    let gpu_mem_summary = format!(
        "gpu_mem: octree_used_mb={} octree_cap_mb={} octree_largest_free_kb={} \
         octree_per_chunk_kb={} srun_per_chunk_kb={} ratio_octree_over_srun={:.1} \
         mesh_vert_used_mb={} mesh_vert_cap_mb={} mesh_idx_used_mb={} mesh_idx_cap_mb={}",
        octree_used / (1024 * 1024),
        octree_cap / (1024 * 1024),
        octree_largest / 1024,
        octree_per_chunk / 1024,
        srun_per_chunk / 1024,
        if srun_per_chunk > 0 { octree_per_chunk as f64 / srun_per_chunk as f64 } else { 0.0 },
        renderer.mesh_vert_allocated() / (1024 * 1024),
        renderer.mesh_vert_capacity() / (1024 * 1024),
        renderer.mesh_idx_allocated() / (1024 * 1024),
        renderer.mesh_idx_capacity() / (1024 * 1024),
    );
    log::info!("{}", gpu_mem_summary);
    let body = format!("{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n",
        gpu_summary, cpu_summary, mesher_summary, workload_summary,
        mem_summary, comp_summary, gpu_mem_summary, tick_apply_summary,
        empty_mesh_summary,
    );
    // v0.24.2c.8: shutdown event records duration + the "did we hit
    // the max-duration cap?" signal. Emitted BEFORE the RunBlock so
    // events.jsonl and run_summaries.txt see the same shutdown timing.
    let hit_cap = max_duration.map(|d| elapsed >= d).unwrap_or(false);
    unendlich_shared::events::record("shutdown", serde_json::json!({
        "duration_secs": elapsed.as_secs_f32(),
        "reason": if hit_cap { "max_duration" } else { "window_close_or_esc" },
    }));

    let run_block = unendlich_shared::run_summary::RunBlock {
        version: env!("CARGO_PKG_VERSION"),
        uuid: &run_uuid,
        duration_secs: elapsed.as_secs_f32(),
        cmdline: &cmdline,
        env_flags: &flags_summary,
        body: &body,
    };
    // v0.24.NEXT-mem-telemetry: drop sub-2s runs so transient startup/crash
    // runs don't spam run_summaries.txt with throwaway run-ids (user request).
    if elapsed.as_secs_f32() < 2.0 {
        log::info!(
            "Run only {:.2}s (<2s); skipping run-summary append to avoid run-id spam",
            elapsed.as_secs_f32(),
        );
    } else {
        match run_block.append(unendlich_shared::run_summary::DEFAULT_PATH) {
            Ok(()) => log::info!(
                "Appended run summary to {}",
                unendlich_shared::run_summary::DEFAULT_PATH,
            ),
            Err(e) => log::warn!(
                "Could not write run summary to {}: {}",
                unendlich_shared::run_summary::DEFAULT_PATH, e,
            ),
        }
    }
    // v0.24.2c.9: in spec-driven mode, emit the .unendresult.json
    // alongside everything else, evaluate assertions, and stash the
    // exit code for the final process::exit at the end of main. We
    // do this BEFORE renderer.destroy so the timer percentile
    // accessors still work.
    let mut spec_exit_code: i32 = 0;
    if let Some(ref ls) = loaded_spec {
        let result_path = ls.result_path();

        // Build the stats map. Layout:
        //   {
        //     "gpu": { "shadow": {n,min_ms,...}, "main": {...}, ... },
        //     "cpu": { "wait_fences": {...}, ... },
        //     "workload": { "loaded": {min,p50,p90,max}, "rendering": {...} }
        //   }
        //
        // Plus a flat-suffix-sugar view for simpler assertions:
        //   "shadow_p50_ms": <number>, "shadow_p99_ms": <number>, ...
        //   "main_p50_ms": <number>, ...
        //   "rendering_max": <number>, "rendering_p50": <number>, ...
        //   "loaded_max": <number>, ...
        let mut stats = serde_json::Map::new();
        let gpu_pct = renderer.gpu_zone_percentiles();
        let cpu_pct = renderer.cpu_zone_percentiles();

        let mut gpu_map = serde_json::Map::new();
        for (zone, p) in &gpu_pct {
            gpu_map.insert(zone.to_string(), serde_json::to_value(p)
                .unwrap_or(serde_json::Value::Null));
            // Flat keys: <zone>_p50_ms, <zone>_p99_ms, <zone>_max_ms.
            stats.insert(format!("{}_p50_ms", zone), serde_json::json!(p.p50_ms));
            stats.insert(format!("{}_p90_ms", zone), serde_json::json!(p.p90_ms));
            stats.insert(format!("{}_p99_ms", zone), serde_json::json!(p.p99_ms));
            stats.insert(format!("{}_max_ms", zone), serde_json::json!(p.max_ms));
            stats.insert(format!("{}_min_ms", zone), serde_json::json!(p.min_ms));
        }
        stats.insert("gpu".to_string(), serde_json::Value::Object(gpu_map));

        let mut cpu_map = serde_json::Map::new();
        for (zone, p) in &cpu_pct {
            cpu_map.insert(zone.to_string(), serde_json::to_value(p)
                .unwrap_or(serde_json::Value::Null));
            // Flat keys with "cpu_" prefix to disambiguate from GPU zones.
            stats.insert(format!("cpu_{}_p50_ms", zone), serde_json::json!(p.p50_ms));
            stats.insert(format!("cpu_{}_p99_ms", zone), serde_json::json!(p.p99_ms));
            stats.insert(format!("cpu_{}_max_ms", zone), serde_json::json!(p.max_ms));
        }
        stats.insert("cpu".to_string(), serde_json::Value::Object(cpu_map));

        // Workload counters: loaded/rendering as full distributions
        // plus flat suffix keys for assertion ergonomics.
        fn dist(samples: &[u64]) -> serde_json::Value {
            if samples.is_empty() {
                return serde_json::json!({"n":0});
            }
            let mut s = samples.to_vec();
            s.sort_unstable();
            let n = s.len();
            let pct = |q: f64| -> u64 {
                let idx = ((q * (n as f64 - 1.0)) as usize).min(n - 1);
                s[idx]
            };
            serde_json::json!({
                "n": n,
                "min": s[0],
                "p50": pct(0.50),
                "p90": pct(0.90),
                "max": s[n - 1],
            })
        }
        let loaded_dist = dist(&loaded_samples);
        let rendering_dist = dist(&rendering_samples);
        // Flat suffix keys.
        for field in ["min", "p50", "p90", "max"] {
            if let Some(v) = loaded_dist.get(field) {
                stats.insert(format!("loaded_{}", field), v.clone());
            }
            if let Some(v) = rendering_dist.get(field) {
                stats.insert(format!("rendering_{}", field), v.clone());
            }
        }
        let mut workload = serde_json::Map::new();
        workload.insert("loaded".to_string(), loaded_dist);
        workload.insert("rendering".to_string(), rendering_dist);

        // c.13-instrumentation: world-state snapshots for
        // cross-referencing perf data with workload size. The start
        // snapshot is captured at the warmup→steady transition (or
        // at the boot→steady transition if no warmup was set). The
        // end snapshot is captured at shutdown. The diff tells us
        // how much the world grew (or shrank, if eviction was active)
        // during the measurement window.
        //
        // Today this is just (loaded_chunks, non_air_blocks). Future
        // ships will extend with plant_count, creature_count, etc.
        // when working in those subsystems — kept thin until needed.
        let end_snapshot = chunk_manager.world_state_snapshot();
        log::info!(
            "End-of-run world state: loaded_chunks={} non_air_blocks={}",
            end_snapshot.0, end_snapshot.1,
        );
        let mut snapshot_map = serde_json::Map::new();
        if let Some((start_loaded, start_blocks)) = start_snapshot {
            snapshot_map.insert("start".to_string(), serde_json::json!({
                "loaded_chunks": start_loaded,
                "non_air_blocks": start_blocks,
            }));
        } else {
            // Boot may not have completed yet on very short runs.
            // Record `null` rather than synthesizing fake numbers.
            snapshot_map.insert("start".to_string(), serde_json::Value::Null);
        }
        snapshot_map.insert("end".to_string(), serde_json::json!({
            "loaded_chunks": end_snapshot.0,
            "non_air_blocks": end_snapshot.1,
        }));
        // Also include a `delta` for convenience. Assertions can use
        // these flat keys (e.g. `workload_blocks_delta < 1000` to check
        // the world didn't churn during measurement).
        if let Some((s_l, s_b)) = start_snapshot {
            let dl: i64 = end_snapshot.0 as i64 - s_l as i64;
            let db: i64 = end_snapshot.1 as i64 - s_b as i64;
            snapshot_map.insert("delta".to_string(), serde_json::json!({
                "loaded_chunks": dl,
                "non_air_blocks": db,
            }));
            // Flat sugar keys.
            stats.insert("workload_chunks_delta".to_string(), serde_json::json!(dl));
            stats.insert("workload_blocks_delta".to_string(), serde_json::json!(db));
        }
        workload.insert("snapshot".to_string(), serde_json::Value::Object(snapshot_map));

        stats.insert("workload".to_string(), serde_json::Value::Object(workload));

        // Resolved env-flag values (env-source vs default).
        let mut env_map = std::collections::BTreeMap::new();
        for f in unendlich_engine::runtime_flags::flags() {
            let applies = f.binaries.is_empty()
                || f.binaries.contains(&"unendlich-world");
            if !applies { continue; }
            let (value, source) = match std::env::var(f.env) {
                Ok(v) => (v, "env".to_string()),
                Err(_) => (f.default.to_string(), "default".to_string()),
            };
            env_map.insert(f.env.to_string(),
                unendlich_shared::test_spec::ResolvedFlag { value, source });
        }

        // Evaluate assertions + build the result via the
        // pure-function helper so the binary's wiring stays thin.
        let shutdown_reason = if hit_cap { "max_duration" } else { "window_close_or_esc" };
        let result = unendlich_shared::test_spec::build_result(
            unendlich_shared::test_spec::BuildResultInputs {
                spec: ls,
                binary_version: env!("CARGO_PKG_VERSION"),
                binary: "unendlich-world",
                run_uuid: &run_uuid,
                ran_at_utc: &unendlich_shared::run_summary::iso8601_utc_now(),
                duration_secs: elapsed.as_secs_f32(),
                shutdown_reason,
                env: env_map,
                stats,
                gpu_info: Some(gpu_info.clone()),
            },
        );
        if !result.passed { spec_exit_code = 1; }

        match serde_json::to_string_pretty(&result) {
            Ok(json) => match std::fs::write(&result_path, json) {
                Ok(()) => log::info!(
                    "Wrote .unendresult to {} (passed={})",
                    result_path.display(), result.passed,
                ),
                Err(e) => {
                    log::error!("Failed to write {}: {}", result_path.display(), e);
                    spec_exit_code = 2;
                }
            },
            Err(e) => {
                log::error!("Failed to serialise result: {}", e);
                spec_exit_code = 2;
            }
        }

        // Per-assertion summary to terminal log too.
        for ar in &result.assertions {
            if ar.passed {
                log::info!("  PASS  {}  (actual={})", ar.name, ar.actual);
            } else {
                log::warn!("  FAIL  {}  expected {}  got {}  ({})",
                    ar.name, ar.expected, ar.actual,
                    ar.reason.as_deref().unwrap_or(""));
            }
        }
    }

    // v0.24.2c.8: flush the events file so the last records aren't
    // lost if the process is killed during shutdown.
    unendlich_shared::events::flush();

    // Hide the window FIRST, before we touch any Vulkan resources. The
    // teardown of swapchain / device / etc. can hang on
    // device_wait_idle() if the GPU is in a bad state (we've seen this
    // on Apple M2 Max via Asahi Vulkan during normal Esc-Esc shutdown).
    // Hiding the window first means the user sees the app close
    // immediately even if the process needs another second to fully
    // exit (or gets killed by Ctrl-C / OOM-style cleanup later).
    log::info!("Hiding window...");
    window.hide();

    log::info!("Destroying renderer...");
    renderer.destroy(&ctx);

    // v0.24.21-gpu-mesher-prod: drop mesh_pool (and the GPU mesher
    // Arc, if any) BEFORE dropping ctx.
    //
    // Reason: when GPU meshing is enabled, mesh_pool's dispatcher
    // thread holds an Arc clone of SparseGpuMesher; the
    // `_gpu_mesher_for_cleanup` binding in `main` holds another.
    // The mesher's Drop calls `device.destroy_*(...)` to release
    // Vulkan resources, which requires `ctx.device` to still be
    // alive.
    //
    // Order:
    //   1. drop(mesh_pool) — joins workers and dispatcher; the
    //      dispatcher's Arc clone is released as part of its exit.
    //   2. drop(_gpu_mesher_for_cleanup) — last Arc clone dies,
    //      mesher's Drop runs against the live device.
    //   3. drop(ctx) — device + instance teardown.
    //
    // For v0.24.20 and earlier (CPU-only mesher), workers held no
    // Vulkan resources, so the original "ctx-before-mesh_pool"
    // order was harmless.
    log::info!("Pre-exit: dropping mesh_pool (may join worker threads + GPU dispatcher)...");
    drop(mesh_pool);
    log::info!("Pre-exit: dropped mesh_pool");
    log::info!("Pre-exit: dropping GPU mesher (if active)...");
    drop(_gpu_mesher_for_cleanup);
    // v0.24.27-octree-mesher: same drop discipline for the octree
    // mesher Arc. At most one of `_gpu_mesher_for_cleanup` and
    // `_octree_gpu_mesher_for_cleanup` is Some(_) at runtime; the
    // other's drop is a no-op.
    drop(_octree_gpu_mesher_for_cleanup);
    log::info!("Pre-exit: dropped GPU mesher");

    log::info!("Dropping Vulkan context...");
    drop(ctx);
    log::info!("Shutdown complete.");

    // v0.24.2c.4: explicit-drop diagnostics + force-exit.
    // v0.24.2c.5: mesh_pool drop fixed (was the culprit; bounded
    // result-channel was full and workers blocked in res_tx.send,
    // never noticing request channel had closed). Other drops below
    // haven't been validated yet — they MAY also hang, in which case
    // the diagnostic logs will name the next culprit.
    //
    // v0.24.21: mesh_pool drop moved up (before ctx) — see the block
    // above. The pre-exit section is now only the GLFW window/glfw
    // teardown.
    //
    // We keep the explicit-drop block + `process::exit(0)` belt-and-
    // suspenders so chained shell invocations work reliably even if
    // a future Drop bug appears. Important Drop side effects (saved
    // chunks, flushed run-summary file, Vulkan teardown) all happen
    // explicitly ABOVE this block, so exit-bypass is safe.
    //
    // If a future playtest shows all "dropped X" lines for the
    // remaining drops, we can switch to natural drop and remove the
    // exit call — but until that's empirically true, the exit stays.
    log::info!("Pre-exit: dropping chunk_manager (may join chunk-loader workers)...");
    drop(chunk_manager);
    log::info!("Pre-exit: dropped chunk_manager");
    log::info!("Pre-exit: dropping window...");
    drop(window);
    log::info!("Pre-exit: dropped window");
    log::info!("Pre-exit: dropping glfw...");
    drop(glfw);
    log::info!("Pre-exit: dropped glfw");
    log::info!("Pre-exit: calling std::process::exit({})", spec_exit_code);
    std::process::exit(spec_exit_code);
}

// Suppress an "unused" warning for MouseButton if the const isn't dragged in.
#[allow(dead_code)]
fn _silence_unused(_b: MouseButton) {}

/// Scan a chunk for SURFACE lava cells and add a heat source for
/// each. "Surface lava" means a Lava block with at least one Air
/// neighbor — interior lava (surrounded by stone) is heat-trapped
/// and shouldn't drive convection at the open-air surface above.
///
/// This is the runtime side of worldgen lava: the cpu_generator
/// places lava cells in deep stone, but they don't affect anything
/// until they're registered as heat sources here. Once registered,
/// `temperature_at_with_heat` includes their contribution; the F4
/// wind arrow shows updrafts above lava-rich chunks; ice doesn't
/// form near lava (thermal_mass_factor sees lava neighbors as
/// reservoir contribution).
///
/// Registered intensity: 600°C (less than F9 fires at 800°C — bulk
/// lava is hot but not as concentrated as a flickering flame).
/// Radius: 4 blocks (lava heat is shorter-range than fire because
/// lava is dense; flames radiate further).
///
/// Returns the count of sources added.
fn register_lava_heat_sources(
    chunk: &unendlich_engine::world::chunk::Chunk,
    heat_sources: &unendlich_engine::heat_source::HeatSourceMap,
) -> usize {
    use unendlich_engine::constants::CHUNK_SIZE;
    use unendlich_engine::heat_source::HeatSource;

    let (origin_x, origin_y, origin_z) = chunk.coord.block_origin();
    let mut to_add: Vec<HeatSource> = Vec::new();

    // Helper: check if (lx, ly, lz) is air. Out-of-chunk reads return
    // Air conservatively — at chunk edges this means we MIGHT register
    // a lava cell as "surface" when its actual neighbor across the
    // chunk boundary is stone. Acceptable approximation: at worst we
    // get a slight excess of heat sources at chunk borders, which
    // produces a tiny boost to convection at boundaries but doesn't
    // produce visible artifacts.
    let is_air_at = |lx: i32, ly: i32, lz: i32| -> bool {
        if lx < 0 || lx >= CHUNK_SIZE
            || ly < 0 || ly >= CHUNK_SIZE
            || lz < 0 || lz >= CHUNK_SIZE
        {
            return true;
        }
        chunk.blocks[unendlich_engine::world::chunk::Chunk::block_index(lx, ly, lz)] == BlockId::Air
    };

    for ly in 0..CHUNK_SIZE {
        for lz in 0..CHUNK_SIZE {
            for lx in 0..CHUNK_SIZE {
                let idx = unendlich_engine::world::chunk::Chunk::block_index(lx, ly, lz);
                if chunk.blocks[idx] != BlockId::Lava { continue; }
                // Surface lava: at least one of 6 face neighbors is Air.
                let surface = is_air_at(lx + 1, ly, lz)
                    || is_air_at(lx - 1, ly, lz)
                    || is_air_at(lx, ly + 1, lz)
                    || is_air_at(lx, ly - 1, lz)
                    || is_air_at(lx, ly, lz + 1)
                    || is_air_at(lx, ly, lz - 1);
                if !surface { continue; }
                let wx = (origin_x + lx) as f32 + 0.5;
                let wy = (origin_y + ly) as f32 + 0.5;
                let wz = (origin_z + lz) as f32 + 0.5;
                to_add.push(HeatSource::new([wx, wy, wz], 600.0, 4.0));
            }
        }
    }

    let n = to_add.len();
    if n > 0 {
        heat_sources.add_many(to_add);
    }
    n
}


/// Populate the debug overlay scene with markers/arrows showing real-
/// time engine adaptations. Called every frame when overlay is on.
///
/// What's visualized:
///   - Player physics velocity arrow (green) at player position.
///   - Wind direction arrow (cyan) above the player, scaled by wind
///     magnitude so calm wind shows a tiny arrow, gusts show a big one.
///   - AABB outline (yellow) of the player's collider — useful for
///     debugging collision behavior.
///   - Lifecycle stage markers (color-coded) above each plant in the
///     player's chunk that's NOT in the default Mature stage. Capped
///     to a few hundred markers per frame for perf.

/// Quick helper for the per-second log line. Counts how many chunks the
/// renderer is currently displaying. Avoids re-collecting a HashSet
/// just for the count.
fn already_in_renderer_count(renderer: &unendlich_engine::render::WorldRenderer) -> usize {
    renderer.loaded_chunk_coords().count()
}

