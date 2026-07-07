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
// r328: native input vocabulary for InputCache queries; glfw types above
// are kept only for raw WindowEvent matching + glfw API calls.
use unendlich_engine::input_codes::{Keycode, MouseBtn};
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
const UPLOAD_BUDGET_MS_DEFAULT: f64 = 8.0; // r482: ↑ from 6.0 (FIF=2 headroom)

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
const MAX_UPLOADS_PER_FRAME_DEFAULT: usize = 96; // r482: ↑ from 64 (adaptive ctrl manages range [12,192])

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
const UPLOAD_BYTES_BUDGET_DEFAULT: usize = 12 * 1024 * 1024; // r482: ↑ from 8MB

/// v0.24.3 upload-pressure investigation: emit an `upload_pressure`
/// event when per-frame upload bytes exceeds this threshold. Hunting
/// frames that pay the memory-contention cost; lower threshold catches
/// more, higher reduces noise. 2 MB picked to catch most chug-correlated
/// frames without flooding events.jsonl during normal play.
const UPLOAD_PRESSURE_EVENT_THRESHOLD_BYTES: usize = 2 * 1024 * 1024;

/// Number of mesh-worker threads. CPU-bound, so leave 2 cores for the OS
/// + main thread. M2 Max has 12 perf+eco cores, so 8 is reasonable.
const MESH_WORKERS: usize = 6; // r482: ↑ from 4 (more CPU-mesher throughput; big win on Ultra)

/// Mesh request queue capacity. ~4× per-frame upload cap so workers stay
/// busy across player movement bursts.
const MESH_QUEUE_CAPACITY: usize = 64;

/// A live, streamed creature roaming the world, with everything its behaviour
/// needs: body + wander state, the chunk it belongs to, whether it's persistent
/// (dragons never despawn/flee), its genome-derived senses/turn character, and
/// its ecology — energy reserve, sex, species, and genome (for grazing,
/// starvation, and mating).
struct Roamer {
    /// r143: taming state (sim::bond_tame) — Feed/Gift raise trust, Harm
    /// tanks it; Tamed creatures follow the player.
    bond: unendlich_engine::sim::bond_tame::Bond,
    trust: f32,
    /// r127: hit points — punching wires sim::combat::resolve; at 0 the
    /// creature dies and drops meat. (r129: field placement fixed — the r127
    /// edit anchored on the wrong first field and never landed.)
    hp: i32,
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
    /// r462: the catalogued wild species this creature is (from sim::fauna_roster), picked by biome
    /// at spawn for generic Fauna. None for special kinds (Dragon/Lauscher/etc.) and egg-spawns.
    species: Option<unendlich_engine::sim::fauna_roster::Species>,
    genome: unendlich_engine::genetics::Genome,
    forage_memory: unendlich_engine::sim::forage_memory::ForageMemory,
    leap: unendlich_engine::sim::leap::Leap,
    /// In-air ballistic state during a leap: (world velocity, ticks aloft).
    airborne: Option<([f32; 3], u16)>,
    posture: unendlich_engine::sim::posture::Posture,
    stumble: unendlich_engine::sim::stumble::Stumble,
    /// r119 dragon soaring: true while in the flight phase of the cycle.
    flying: bool,
    /// r119: per-individual flight clock (seeded so dragons desynchronize).
    flight_phase: f32,
    /// r265: frame number when this roamer last ran a full brain tick. Used by
    /// the mob_bands gate to compute ticks_since_last so a Simulation-band mob
    /// can integrate the whole skipped interval at once (catch-up), instead of
    /// silently dropping the frames. 0 = never ticked yet.
    last_tick_frame: u64,
    /// r268: current aggression toward the player in [0,1]. Spikes when harmed
    /// (capped by aggression_level from health/numbers), decays over time +
    /// distance (aggro_decay). High aggro makes a non-persistent creature
    /// APPROACH rather than flee (gated behind UNENDLICH_AGGRO). 0 = placid.
    aggro: f32,
    /// r269: last behavior the arbiter chose, encoded (0=none, 1=Rest, 2=Forage,
    /// 3=Patrol, 4=Fight, 5=Flee). Feeds hysteresis so the choice doesn't
    /// flip-flop on near-ties.
    prev_behavior: u8,
    /// r269: BEHAVIORAL MEMORY / habit in [0,1] — a learned combat tendency that
    /// strengthens each time the creature fights and decays slowly otherwise.
    /// It biases effective aggression UPWARD, so a battle-hardened individual
    /// stays ready to fight even when its genome/temperament would say flee.
    /// This is a FAST per-individual path-dependent channel that runs against the
    /// SLOW population-genetic trend (see sim_vectors doc). 0 = naive.
    combat_habit: f32,
    /// r300: cached expressed disposition. Disposition is a pure function of the
    /// genome, which is fixed for life, so `express_disposition` need only run
    /// once per individual instead of every tick (the loop re-derived it each
    /// frame — see the comment that flagged "caching it on the roamer is the perf
    /// refinement"). Filled lazily on first need; identical value, so behaviour
    /// is unchanged. None = not yet expressed.
    disposition_cache: Option<unendlich_engine::sim::disposition::Disposition>,
    /// r300: GRAZE MEMORY — the chosen food block this herbivore is walking to.
    /// The 30-cell ring scan is the loop's single largest `block_at` source; with
    /// this, the scan runs only to *acquire* a target (on a cell change or every
    /// GRAZE_SCAN_INTERVAL ticks), and the creature pursues the remembered target
    /// in between instead of re-scanning every frame. None = no current target.
    graze_target: Option<(i32, i32, i32)>,
    /// r300: cell at the last graze scan (motion gate — a stationary grazer's
    /// larder doesn't change). i32::MIN sentinel forces a scan on first tick.
    graze_cell: (i32, i32),
    /// r300: `roamer_tick` value at the last graze scan (cadence gate).
    graze_scan_tick: u32,
}

/// r666: build a wild, persistent [`Roamer`] of `kind` at `pos`, seeded by
/// `wseed`. Factored out of the H-key spawn so the tutorial arena can place
/// "trainer" creatures through the exact same construction (genome-derived
/// senses / turn character / ecology all identical).
fn build_roamer(kind: unendlich_engine::item::CreatureKind, pos: [f32; 3], wseed: u64) -> Roamer {
    let mut cr = unendlich_engine::spawn::spawn(kind, pos);
    cr.body.position[1] = cr.grounded_y(pos[1]);
    let genome = unendlich_engine::genetics::Genome::from_seed(wseed);
    let ph = genome.express(wseed, 0);
    let speed = 0.8 + ph.speed * 1.4;
    let turn_rate = 1.5 + ph.turn_rate * 4.0;
    let senses = unendlich_engine::sim::senses::senses_for(kind, &genome, wseed, 0);
    let turn_profile = unendlich_engine::sim::cognition::steering::TurnProfile::from_seed(wseed);
    let home = (
        (pos[0].floor() as i32).div_euclid(unendlich_engine::constants::CHUNK_SIZE),
        (pos[2].floor() as i32).div_euclid(unendlich_engine::constants::CHUNK_SIZE),
    );
    Roamer {
        bond: unendlich_engine::sim::bond_tame::Bond::Wild,
        trust: 0.0,
        hp: 40,
        creature: cr,
        wander: unendlich_engine::sim::cognition::wander::Wander::new(wseed, speed),
        home,
        persistent: true, // never despawned by streaming
        senses,
        turn_rate,
        turn_profile,
        vitality: unendlich_engine::sim::feeding::Vitality::full(),
        sex: unendlich_engine::genetics::sex_from_seed(wseed),
        kind,
        species: None,
        genome,
        forage_memory: unendlich_engine::sim::forage_memory::ForageMemory::new(16),
        leap: unendlich_engine::sim::leap::Leap::from_genes(ph.speed, ph.speed),
        airborne: None,
        posture: unendlich_engine::sim::posture::Posture::new(0.0),
        stumble: unendlich_engine::sim::stumble::Stumble::new(),
        flying: false,
        flight_phase: (wseed % 30) as f32,
        last_tick_frame: 0,
        aggro: 0.0,
        prev_behavior: 0,
        combat_habit: 0.0,
        disposition_cache: None,
        graze_target: None,
        graze_cell: (i32::MIN, i32::MIN),
        graze_scan_tick: 0,
    }
}


// r124: survival-mode hand-item semantics at the use (right-mouse) event.
// Returns Some(block) if a place should proceed (consuming happened), None if
// the event was consumed by eating or denied for lack of items.
const MEAT_ITEM: u32 = 2000; // r127: dropped by slain creatures; hearty edible
const COOKED_MEAT_ITEM: u32 = 2001; // r510: raw meat cooked at a fire — more nourishing
const APPLE_ITEM: u32 = 2002; // r579: fruit foraged from leafy trees; a light, refreshing snack

fn survival_use_item(
    app: &mut unendlich_engine::app::App,
    selected_block: unendlich_engine::world::blocks::BlockId,
    hand_item: Option<unendlich_engine::items::ItemId>,
) -> Option<unendlich_engine::world::blocks::BlockId> {
    use unendlich_engine::items::ItemId;
    use unendlich_engine::world::blocks::BlockId;
    // r127/r510: non-block hand items resolve first. Cooked meat is the hearty meal;
    // raw meat barely sustains you (cook it at a fire — see the cook tick in main).
    if let Some(h) = hand_item {
        if h.0 == COOKED_MEAT_ITEM {
            if app.inventory_mut().remove_item(h, 1) == 0 {
                app.vitality_mut().feed(0.55);
                log::info!("[survival] ate cooked meat — vitality {:.0}%", app.vitality().energy * 100.0);
            }
            return None;
        }
        if h.0 == MEAT_ITEM {
            if app.inventory_mut().remove_item(h, 1) == 0 {
                app.vitality_mut().feed(0.18); // raw: barely nourishing — cook it for the full meal
                log::info!("[survival] ate RAW meat (cook it at a fire for more) — vitality {:.0}%", app.vitality().energy * 100.0);
            }
            return None;
        }
        if h.0 == APPLE_ITEM {
            // r579: a foraged fruit — light but refreshing.
            if app.inventory_mut().remove_item(h, 1) == 0 {
                app.vitality_mut().feed(0.22);
                log::info!("[survival] ate an apple — vitality {:.0}%", app.vitality().energy * 100.0);
            }
            return None;
        }
    }
    let id = ItemId(selected_block as u32);
    // Edible hand items: right-mouse EATS instead of placing. r512: vegetables are food too —
    // but the bin's place path plants them when you aim at soil, so eating only happens off-soil.
    let edible = matches!(
        selected_block,
        BlockId::Mushroom | BlockId::Carrot | BlockId::Potato | BlockId::Cabbage
    );
    if edible {
        if app.inventory_mut().remove_item(id, 1) == 0 {
            let feed = match selected_block {
                BlockId::Carrot | BlockId::Cabbage => 0.28,
                BlockId::Potato => 0.34, // starchy — more filling
                _ => 0.25,
            };
            app.vitality_mut().feed(feed);
            log::info!("[survival] ate {:?} — vitality {:.0}%", selected_block, app.vitality().energy * 100.0);
        } else {
            log::info!("[survival] no {:?} in inventory to eat", selected_block);
        }
        return None;
    }
    // Placing consumes one from the inventory; deny if absent.
    if app.inventory_mut().remove_item(id, 1) == 0 {
        Some(selected_block)
    } else {
        log::info!("[survival] no {:?} in inventory — mine some first", selected_block);
        None
    }
}


// r126: world catalog persistence (bin-owned IO; sim stays pure).
// One world per line: name|seed|mode|created|last_played
fn catalog_file_path() -> std::path::PathBuf {
    // r203 BUGFIX: was env!("CARGO_MANIFEST_DIR") — a COMPILE-TIME path to
    // the BUILD machine's source tree. On the shipped binary that
    // directory doesn't exist, so the catalog silently read/wrote nowhere
    // and "Load World" was always empty (operator: "still can't see
    // previous worlds"). Use a runtime path: $UNENDLICH_SAVES if set, else
    // the platform config dir (matches the settings.json location), else a
    // cwd-relative fallback. create_dir_all in save_catalog handles
    // first-run.
    if let Ok(p) = std::env::var("UNENDLICH_SAVES") {
        return std::path::PathBuf::from(p).join("catalog.txt");
    }
    std::env::var("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|_| std::env::var("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))
        .map(|p| p.join("unendlich/saves/catalog.txt"))
        .unwrap_or_else(|_| std::path::PathBuf::from("unendlich-saves/catalog.txt"))
}


// r203: a single rollback restore point parsed from its .point marker.
struct RollbackPoint { ts: u64, reason: String, chunks: u32 }

/// List a world's rollback points, newest first, by reading the .point
/// markers written during play. Pure IO; missing dir -> empty.
fn list_rollback_points(world_name: &str) -> Vec<RollbackPoint> {
    let dir = format!(
        "worlds/{}/rollbacks",
        world_name.replace(['/', '\\', ' '], "_")
    );
    let Ok(rd) = std::fs::read_dir(&dir) else { return Vec::new() };
    let mut pts: Vec<RollbackPoint> = Vec::new();
    for entry in rd.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("point") {
            continue;
        }
        let ts = path
            .file_stem()
            .and_then(|s| s.to_str())
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);
        let body = std::fs::read_to_string(&path).unwrap_or_default();
        let mut reason = String::from("point");
        let mut chunks = 0u32;
        for line in body.lines() {
            if let Some(r) = line.strip_prefix("reason=") { reason = r.to_string(); }
            if let Some(c) = line.strip_prefix("chunks=") { chunks = c.trim().parse().unwrap_or(0); }
        }
        pts.push(RollbackPoint { ts, reason, chunks });
    }
    pts.sort_by(|a, b| b.ts.cmp(&a.ts)); // newest first
    pts
}

fn load_catalog_entries() -> Vec<unendlich_engine::sim::world_catalog::WorldEntry> {
    use unendlich_engine::sim::world_catalog::{GameMode, WorldEntry};
    let Ok(text) = std::fs::read_to_string(catalog_file_path()) else { return Vec::new() };
    let mut all: Vec<WorldEntry> = text
        .lines()
        .filter_map(|line| {
            let mut p = line.split('|');
            Some(WorldEntry {
                name: p.next()?.to_string(),
                seed: p.next()?.parse().ok()?,
                mode: if p.next()? == "survival" { GameMode::Survival } else { GameMode::Creative },
                created_tick: p.next()?.parse().ok()?,
                last_played_tick: p.next()?.parse().ok()?,
            })
        })
        .collect();
    // r577: DEDUP BY NAME — the world's identity is its name (matching
    // WorldCatalog::add and the player's mental model). A catalog file that
    // accumulated same-named rows (e.g. a world re-created with a fresh seed) was
    // surfacing DUPLICATE worlds in the menu, since the menu list is built straight
    // from this loader. Keep the most-recently-played row for each name; the next
    // save then rewrites the file clean. Stable for ties (preserves file order).
    all.sort_by(|a, b| b.last_played_tick.cmp(&a.last_played_tick));
    let mut seen = std::collections::HashSet::new();
    all.retain(|e| seen.insert(e.name.clone()));
    all
}

fn save_catalog(entries: &[unendlich_engine::sim::world_catalog::WorldEntry]) {
    use unendlich_engine::sim::world_catalog::GameMode;
    let path = catalog_file_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let body: String = entries
        .iter()
        .map(|e| {
            format!(
                "{}|{}|{}|{}|{}\n",
                e.name.replace('|', "_"),
                e.seed,
                if e.mode == GameMode::Survival { "survival" } else { "creative" },
                e.created_tick,
                e.last_played_tick
            )
        })
        .collect();
    if let Err(e) = std::fs::write(&path, body) {
        log::warn!("[saves] couldn't write catalog: {e}");
    }
}

// r130: pitch proxy for bending directional — forward().y in [-1,1].
fn cam_pitch_for_bend(lp: &unendlich_engine::local_player::LocalPlayer) -> f32 {
    lp.forward().y
}

/// r537: CAMERA PINCH. March from the eye toward the desired chase-camera position
/// and pull the camera in so terrain never sits between it and the player in tight
/// spaces. Returns the camera distance to actually use. Always pinches in a small
/// baseline (5%); each step that hits a solid block clamps the distance to just
/// short of it, weighted by how near the obstruction is (closer hit ⇒ tighter pull).
fn pinch_camera_distance(
    eye: nalgebra_glm::Vec3,
    dir: nalgebra_glm::Vec3,
    desired: f32,
    solid: impl Fn(i32, i32, i32) -> bool,
) -> f32 {
    if desired <= 0.05 {
        return desired;
    }
    // 5% baseline pinch so the camera never rides flush against a surface.
    let mut limit = desired * 0.95;
    let margin = 0.35_f32; // keep the near plane off the blocking face
    let step = 0.15_f32;
    let mut t = 0.25_f32; // skip the eye's own cell
    while t < desired {
        let p = eye + dir * t;
        if solid(
            p.x.floor() as i32,
            p.y.floor() as i32,
            p.z.floor() as i32,
        ) {
            // Distance weight: pull in to just before the hit (closer hit pulls
            // the camera tighter, which is exactly what tight spaces need).
            limit = limit.min((t - margin).max(0.3));
            break;
        }
        t += step;
    }
    limit.max(0.3)
}

fn cz_off(cy: i32) -> i32 { cy - 3 }

/// r526: a [`WorldQuery`](unendlich_engine::sim::physics::WorldQuery) over the
/// live chunk store, so AI mobs navigate and collide against the REAL voxel
/// terrain using the very same `step_character` kernel the player runs.
struct MobWorld<'a> {
    cm: &'a ChunkManager,
}
impl<'a> unendlich_engine::sim::physics::WorldQuery for MobWorld<'a> {
    #[inline]
    fn physics_at(&self, x: i32, y: i32, z: i32) -> unendlich_engine::sim::physics::BlockPhysics {
        unendlich_engine::sim::physics::physics_of_block(self.cm.block_at(x, y, z))
    }
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

    // r466: crash breadcrumbs — fsync'd "last stage reached" trail that survives a hard power-off
    // (Fairphone-5/Alpine GPU bring-up: the device powers off without flushing normal logs).
    unendlich_engine::breadcrumb::reset();
    unendlich_engine::breadcrumb::drop_crumb("main: startup");

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
        .unwrap_or(24); // r482: ↑ from 16 (more chunks/gen dispatch)
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
    // r183: stride knobs — operator log: plant 14.8ms + leaf 9.0ms/tick.
    // Handlers run every Nth tick with dt*N: same rates, fewer teeth.
    let plant_stride: u64 = std::env::var("UNENDLICH_PLANT_TICK_STRIDE")
        .ok().and_then(|v| v.parse().ok()).unwrap_or(4);
    let leaf_stride: u64 = std::env::var("UNENDLICH_LEAF_TICK_STRIDE")
        .ok().and_then(|v| v.parse().ok()).unwrap_or(2);
    // r450: FLORA SPREAD — surface plants (grass/flower/fern/mushroom) occasionally seed nearby
    // suitable ground via the plant_propagation + plant_growth_env cores (sim::flora_spread). Default
    // ON (ambient, terrain-light); UNENDLICH_FLORA_SPREAD=0 to disable. Budgeted + strided so it's cheap.
    let flora_spread_enabled: bool = std::env::var("UNENDLICH_FLORA_SPREAD")
        .map(|v| v != "0").unwrap_or(true);
    let flora_spread_interval: u64 = std::env::var("UNENDLICH_FLORA_SPREAD_INTERVAL")
        .ok().and_then(|v| v.parse().ok()).filter(|&n: &u64| n > 0).unwrap_or(300);
    let flora_spread_radius: i32 = std::env::var("UNENDLICH_FLORA_SPREAD_RADIUS")
        .ok().and_then(|v| v.parse().ok()).unwrap_or(28);
    let flora_spread_budget: u32 = std::env::var("UNENDLICH_FLORA_SPREAD_BUDGET")
        .ok().and_then(|v| v.parse().ok()).unwrap_or(8);
    let flora_spread_scan: u32 = std::env::var("UNENDLICH_FLORA_SPREAD_SCAN")
        .ok().and_then(|v| v.parse().ok()).unwrap_or(48);
    // r452: SAPLING GROWTH — flora occasionally plants an OakLog sapling that matures into a real
    // tree over time, driven by the sim::plant_growth core (env from the live climate model). Shares
    // the flora_spread enable flag. Registry is in-bin + bounded (near-player; not persisted).
    let sapling_grow_interval: u64 = std::env::var("UNENDLICH_SAPLING_GROW_INTERVAL")
        .ok().and_then(|v| v.parse().ok()).filter(|&n: &u64| n > 0).unwrap_or(120);
    let sapling_cap: usize = std::env::var("UNENDLICH_SAPLING_CAP")
        .ok().and_then(|v| v.parse().ok()).unwrap_or(48);
    let sapling_dt: f32 = std::env::var("UNENDLICH_SAPLING_DT")
        .ok().and_then(|v| v.parse().ok()).unwrap_or(0.30);
    let sapling_plant_pct: u64 = std::env::var("UNENDLICH_SAPLING_PLANT_PCT")
        .ok().and_then(|v| v.parse().ok()).unwrap_or(25);
    // r582: AIR-CLEANUP — until the terrain FP32/FP64 surface fix lands (see
    // NEXT_SESSION_GUIDE §4a / handoff/HANDOFF_world_surface.md), placement reads the
    // CPU FP64 surface *prediction* while the GPU fills voxels in FP32, so flora/trees
    // can land on a surface that isn't really there and end up sitting on air. This pass
    // cuts flora + floating tree trunks that have no solid support beneath them. Budgeted
    // random probes near the player; default on. Set UNENDLICH_AIR_CLEANUP=0 to disable.
    let air_cleanup_enabled = std::env::var("UNENDLICH_AIR_CLEANUP").map(|v| v != "0").unwrap_or(true);
    let air_cleanup_interval: u64 = std::env::var("UNENDLICH_AIR_CLEANUP_INTERVAL")
        .ok().and_then(|v| v.parse().ok()).unwrap_or(120);
    let air_cleanup_budget: u32 = std::env::var("UNENDLICH_AIR_CLEANUP_BUDGET")
        .ok().and_then(|v| v.parse().ok()).unwrap_or(12);
    // (pos, growth state) for each living sapling near the player.
    // r464: hot-metal thermodynamics (sim::hot_metal). Metal cells beside lava heat up; they quench
    // (steam) in water and melt to lava at their melting point (~1500°C — above lava, so a hotter
    // source is needed to actually melt). Bounded discovery + cheap evolution; no per-tick remeshing.
    let hot_metal_enabled = std::env::var("UNENDLICH_HOT_METAL").map(|v| v != "0").unwrap_or(true);
    // r263: MOB BANDS (kill switch, default OFF). When enabled, non-persistent
    // roamers are gated by the world::scheduler band of their chunk (the SAME
    // band streaming uses: Render every tick, Simulation every simulation_divisor,
    // CatchUp dormant) via sim::mob_bands, instead of the ai_budget per-entity
    // curfew. Wired but off so it changes nothing until A/B'd on real hardware;
    // UNENDLICH_MOB_BANDS=1 to try it. See sim/mob_bands.rs + world/scheduler.rs.
    let mob_bands_enabled: bool = std::env::var("UNENDLICH_MOB_BANDS")
        .map(|v| v != "0").unwrap_or(true);
    // r266: AWARENESS BLEND (kill switch, default OFF). Combines a roamer's sight
    // + hearing (+ smell, currently 0) into one graded awareness in [0,1] via
    // sim::awareness_blend (probabilistic OR), surfaced as the "wariness" the
    // perception comment anticipated. Wired as a computed+logged signal — it does
    // NOT change the binary flee logic yet, so behaviour is unchanged; it gives
    // the graded value a future wariness/alert state can read. UNENDLICH_AWARENESS=1.
    let awareness_enabled: bool = std::env::var("UNENDLICH_AWARENESS")
        .map(|v| v != "0").unwrap_or(true);
    if awareness_enabled {
        log::info!("[awareness] ENABLED — graded sense-blend wariness computed per roamer (logged; flee logic unchanged)");
    }
    let mob_band_params = unendlich_engine::world::scheduler::TickScheduleParams::default();
    if mob_bands_enabled {
        log::info!(
            "[mob_bands] ENABLED — non-persistent roamers gated by scheduler band (sim_divisor={})",
            mob_band_params.simulation_divisor,
        );
    }
    // r184: strides are LIVE — settings sliders + autotuner write these.
    let plant_stride_atom = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(plant_stride));
    let leaf_stride_atom = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(leaf_stride));
    tick_reg.register(
        "plant_lifecycle",
        unendlich_engine::world::ticks::strided_dyn(
            plant_stride_atom.clone(),
            unendlich_engine::world::plant_lifecycle::lifecycle_handler(),
        ),
    );
    // r192: fluid POIs — proto-reefs from the coral genome + waterfall
    // intensity POI counting. Idempotent, 1/1024-tick throttled.
    tick_reg.register(
        "reef_seeder",
        unendlich_engine::world::reef_seeder::reef_handler(),
    );
    // Leaf decay is intentionally NOT registered as an auto-firing
    // handler. With age-based decay every leaf dies after ~50-100s
    // real time regardless of context — empties forests. The handler
    // is meant for "leaves whose supporting log is broken decay
    // quickly" semantics, but the support relationship isn't wired
    // yet. Until then, leaves are permanent. Re-enable when the
    // log-removal trigger is in place:
    //
    // r160: the orphan gate now exists (FLAG_DECAYING-only) and the break
    // trigger marks leaves near a removed log — safe to register at last.
    tick_reg.register(
        "leaf_decay",
        unendlich_engine::world::ticks::strided_dyn(
            leaf_stride_atom.clone(),
            unendlich_engine::world::leaf_decay::decay_handler(),
        ),
    );
    let tick_registry = std::sync::Arc::new(tick_reg);
    // r514: keep a copy of the world root so the persistent flora store can load/save
    // alongside the chunk edits (the chunk manager takes ownership of `world_root` next).
    let mut flora_root = world_root.clone();
    let mut chunk_manager = ChunkManager::new(generator, num_workers)
        .with_storage(world_root)
        .with_fields(field_registry)
        .with_ticks(tick_registry);
    log::info!("ChunkManager ready ({} workers, render distance {})", num_workers, runtime_render_distance());

    // r447: route the spawn world through the WorldList so the no-concurrent rule + request
    // whitelist are LIVE in the game (not only in the headless server), and every load attempt is
    // tracked. Today the bin loads exactly one world at startup; this is the single gate any future
    // "load/switch world" path must pass, and it confirms multiple worlds can't be loaded at once.
    let mut world_list = unendlich_engine::world::world_list::WorldList::new();
    {
        use unendlich_engine::world::world_list::WorldEntry;
        let wid = WORLD_SEED_DEFAULT as u64;
        world_list.register(WorldEntry {
            id: wid,
            name: "spawn".to_string(),
            seed: WORLD_SEED_DEFAULT as u64,
        });
        match world_list.try_load(wid) {
            Ok(id) => log::info!(
                "[worldlist] loaded world id={id} (rule={:?}); concurrent loads are denied",
                world_list.rule()
            ),
            Err(e) => log::warn!("[worldlist] spawn world load denied: {e:?}"),
        }
    }

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
    window.set_char_polling(true); // r183: text input (WindowEvent::Char)
    window.set_mouse_button_polling(true);
    window.set_cursor_pos_polling(true);
    window.set_scroll_polling(true); // r321: glfw needs per-event opt-in — without this WindowEvent::Scroll never fires (hotbar + UI scroll dead)
    window.set_framebuffer_size_polling(true);
    // Initial cursor mode — visible until the user clicks to capture.
    window.set_cursor_mode(CursorMode::Normal);

    unendlich_engine::breadcrumb::drop_crumb("gpu: creating Vulkan context (instance + physical device + queues)");
    let ctx = VulkanContext::new(&glfw, &window).expect("vulkan context");
    unendlich_engine::breadcrumb::drop_crumb("gpu: Vulkan context ready");

    // Capture GPU + driver identity once at startup. Stays constant for
    // the run; gets folded into the result file at shutdown so the
    // launcher's `compare` command can detect driver-epoch drift between
    // compared runs.
    let gpu_info = ctx.gpu_info();
    // ── crash-safe runtime tuning (r322) ────────────────────────────────────
    // The mesh cache (UNENDLICH_MESH_CACHE_SIZE) can hard-crash some GPU/driver
    // combos — a crash that catch_unwind can't see. Apply it through a trial: arm
    // a marker before use; if a previous run died on trial under THIS driver,
    // revert + blacklist it (cleared automatically when the driver_hash changes).
    let safe_tune = {
        let cfg = std::env::var("XDG_CONFIG_HOME")
            .map(std::path::PathBuf::from)
            .or_else(|_| std::env::var("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))
            .unwrap_or_else(|_| std::path::PathBuf::from("."));
        unendlich_engine::safe_tune::SafeTune::load(&cfg, gpu_info.driver_hash.clone())
    };
    if let Some((s, v)) = safe_tune.reverted() {
        log::warn!("[safe-tune] previous run crashed with {s}={v}; reverted + blacklisted for this driver");
    }
    // r323: perf/graphics knobs now flow through RuntimeConfig (env-seeded, the
    // single source the unified settings UI will edit). First field migrated:
    // the mesh cache, which the crash-safe tuner then gates.
    let runtime_cfg = unendlich_engine::runtime_config::RuntimeConfig::from_env();
    let requested_mesh_cache: usize = runtime_cfg.mesh_cache_size;
    let effective_mesh_cache = if requested_mesh_cache > 0 && !safe_tune.is_blacklisted("mesh_cache") {
        safe_tune.arm("mesh_cache", &requested_mesh_cache.to_string());
        log::info!("[safe-tune] mesh_cache={requested_mesh_cache} on trial (marker armed)");
        requested_mesh_cache
    } else {
        if requested_mesh_cache > 0 {
            log::warn!("[safe-tune] mesh_cache requested but blacklisted on this driver; forcing 0");
        }
        0
    };
    let mut safe_tune_disarmed = false;
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
    unendlich_engine::breadcrumb::drop_crumb("gpu: creating WorldRenderer (Vulkan device/swapchain/pipelines)");
    let mut renderer = WorldRenderer::new(&ctx, (fb_w as u32, fb_h as u32), camera)
        .expect("world renderer");
    unendlich_engine::breadcrumb::drop_crumb("gpu: WorldRenderer ready");
    renderer.set_mesh_cache_capacity(effective_mesh_cache); // r322: crash-safe tuner verdict
    log::info!("Renderer ready ({}x{}). Click to capture mouse.", fb_w, fb_h);

    // ── unendlich-fullgame: title menu before gameplay ──────────────────────
    // The menu panel lives in WorldRenderer's ui-overlay (menu_open → a centered
    // Singleplayer / Host / Join / Quit panel). Open it now, route the cursor to
    // the menu and freeze camera look until the player picks. The world still
    // streams behind the menu as a live backdrop. Title music: sim::music_director.
    renderer.menu_open = true;
    renderer.ui_pointer_active = true;
    // r488: the title menu's "Character" button opens the select/customizer; seed
    // the roster (presets + the saved global default) so it's ready when opened.
    renderer.character_roster =
        Some(unendlich_engine::player_character::CharacterRoster::load());
    window.set_cursor_mode(CursorMode::Normal);
    let mut fg_music = unendlich_engine::sim::music_director::MusicState::new();
    let mut fg_music_clock = std::time::Instant::now();
    // Batch-wired services: every #[distributed_slice(SERVICES)] registration in
    // the engine (central + co-located + sim::service_registry_batch) instantiates
    // here and ticks once per frame below. Adding a service never edits this file.
    let mut fg_services =
        unendlich_engine::sim::service_registry::ServiceHost::from_registry();
    println!("[services] wired {} registered services", fg_services.len());
    // Batch-wired pure kernels: every #[distributed_slice(KERNELS)] registration
    // (generated by tools/bootstrap_registry.py) sweeps a deterministic input
    // phase each frame. Stride 8 = one eighth of the set per frame, full
    // coverage every 8 frames — a perpetual fuzz-lite at negligible cost.
    let mut fg_kernels = unendlich_engine::sim::kernel_registry::KernelHost::new(8);
    println!("[kernels] wired {} registered kernels", fg_kernels.len());
    // r316: the kernel/service sweeps are a perpetual fuzz-lite of pure, black_box'd
    // functions (no gameplay effect). They crash the game on a degenerate generated
    // input when the build is panic=abort (catch_unwind can't catch) or for services
    // (no catch_unwind at all). Opt-in now; off by default.
    let fg_sim_sweep = std::env::var("UNENDLICH_SIM_SWEEP").is_ok();
    log::info!(
        "[sim-sweep] kernel/service fuzz sweep: {}",
        if fg_sim_sweep { "ON" } else { "off (UNENDLICH_SIM_SWEEP=1 to enable)" }
    );
    // r120: UNENDLICH_WINDING=N runtime override (the static default is 2 = CW +
    // cull NONE, operator-confirmed; the initializer is const so env lives here).
    if let Some(wv) = std::env::var("UNENDLICH_WINDING").ok().and_then(|v| v.parse::<u32>().ok()) {
        unendlich_engine::debug_settings::DEBUG
            .winding_variant
            .store(wv.min(5), std::sync::atomic::Ordering::Relaxed);
        log::info!("[render] winding_variant override = {}", wv.min(5));
    }
    // r124: GAME MODE — UNENDLICH_MODE=survival enables the survival ruleset
    // (place consumes inventory; right-mouse with an edible selected EATS it;
    // no creative flight). Default stays creative so existing flows don't change.
    let mut survival_mode = std::env::var("UNENDLICH_MODE")
        .map(|v| v.eq_ignore_ascii_case("survival"))
        .unwrap_or(false);
    if survival_mode {
        log::info!("[mode] SURVIVAL — place consumes items, right-mouse eats edibles, no flight (UNENDLICH_MODE)");
    } else {
        log::info!("[mode] CREATIVE — palette placing, flight allowed (UNENDLICH_MODE=survival to switch)");
    }
    // r487: load the player's global default character (persisted in the config
    // dir, so it carries across every world). Falls back to a preset if none is
    // saved. The character-select menu edits + re-saves this.
    let mut active_character = unendlich_engine::player_character::load_global();
    log::info!(
        "[character] active = '{}' (plan={}, seed={:#x}, tint={:?})",
        active_character.name, active_character.arche, active_character.seed, active_character.tint
    );
    // r116b: feature banner — one line per wired feature so "not wired" vs
    // "wired but invisible" is diagnosable from the log alone.
    log::info!(
        "[features] deep_caves=ON(default; UNENDLICH_MULTI_SCALE_CAVES=0 opts out; CPU+GPU gen both carve) \
weather/wind={} flora=per-streamed-chunk spawn_egg=H(captured) load_fog=ON(default) drops=rendered-cubes music=manifest-anchored winding_default=2",
        if std::env::var("UNENDLICH_WEATHER").map(|v| v == "0").unwrap_or(false) { "OFF" } else { "ON(default)" }
    );
    // New-game wizard (Singleplayer). The world is NOT generated until "Create";
    // the bin owns the wizard, the renderer panel is display-only.
    // Title menu + new-game wizard now run through engine::app: the `App` owns the
    // MenuController (which owns the wizard + world catalog) and composes the launch
    // decision in library code that is unit-tested. The renderer panel is display-only.
    // r131 MENU FIX: restore the saved-worlds list AT CONSTRUCTION. r126
    // added entries after with_defaults(), but MenuController builds its
    // GameFlow (has_save/slot_count) from the catalog at construction time —
    // the post-hoc adds desynced menu navigation from menu contents and
    // broke the menu. with_catalog keeps them consistent from frame 0.
    let mut fg_app = {
        let entries = load_catalog_entries();
        if entries.is_empty() {
            unendlich_engine::app::App::with_defaults()
        } else {
            let n = entries.len();
            log::info!("[saves] restored {} world(s) from {}", n, catalog_file_path().display());
            unendlich_engine::app::App::with_catalog(
                unendlich_engine::sim::world_catalog::WorldCatalog::from_entries(entries),
            )
        }
    };

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
    // r139: population dynamics — sim::popdyn's grazing colony models the
    // land's carrying capacity; breeding is gated on it and overshoot culls.
    let mut fg_colony = unendlich_engine::sim::popdyn::Colony::seeded(24, 0xFA1A);
    // r140: regions already populated this session (natural spawning).
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
    let mut test_mobs: Vec<unendlich_engine::sim::mob::Mob> = {
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
    // r526: mobs MOVE now. Tick their AI brains + step_character at a fixed 20 Hz
    // (frame-rate independent), driving them against the live terrain. `mob_sim_tick`
    // is the deterministic tick counter their wander/seek RNG keys off.
    let mut mob_tick_accum: f32 = 0.0;
    let mut mob_sim_tick: u64 = 0;

    // ── r118 (ported from parallel worker, adapted to real APIs) ──────────────
    // Game state: pause follows the settings panel (Esc) or the inventory view.
    // r525: unified UI state machine. `menu_state` is the single source of truth
    // for which in-world overlay is open (NoMenu/Inventory/Pause/Settings/Debug/Craft);
    // the old renderer bools are PUSHED from it each frame and PULLED back on button
    // closes. `show_inventory` / `ui_panel_open` survive only as per-frame derived
    // locals (computed at the top of the loop) so the many downstream reads keep working.
    use unendlich_engine::sim::ui_state::MenuState;
    let mut menu_state = MenuState::NoMenu;
    let mut world_clock_elapsed: f32 = 0.0; // r121: 1 real sec = 1 game minute
    let fg_item_registry = unendlich_engine::items::UniformRegistry(64);

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
    if survival_mode {
        // r124: survival revokes creative flight (set after construction —
        // the flag itself is read near the feature banner, before this point).
        local_player.permissions.can_toggle_flight = false;
    }
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
    // r129: SMOKESCREEN — a 32x24x32 smoke field (2-block cells, anchored at
    // spawn area) advected by sim::smoke_gpu::step_smoke from live F9 fires.
    // Standing inside dense smoke thickens fog: a usable screen.
    let mut smoke_field = unendlich_engine::sim::smoke_gpu::SmokeField::new(32, 24, 32);
    let smoke_params = unendlich_engine::sim::smoke_gpu::SmokeParams {
        wind: [0.4, 0.0, 0.2],
        rise_base: 1.2,
        rise_density: 0.8,
        decay: 0.35,
        emit: 2.0,
        dmax: 1.5,
    };
    let mut in_smoke = false;

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
    let gpu_mesher_mode: u32 = 0; // r612: FORCED to CPU mesher to isolate the "broken planes" — ignores UNENDLICH_GPU_MESHER (was: env, default 0). If the fans vanish on a FRESH world, the GPU octree/sparse compute mesher is the cause. Revert this line to restore env control.
    let _ = std::env::var("UNENDLICH_GPU_MESHER"); // keep the env read side-effect-free / avoid unused warnings elsewhere

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

    // r476: mesh-worker thread count is now tunable (UNENDLICH_MESH_WORKERS,
    // default MESH_WORKERS=4). More workers raise CPU-mesher throughput on
    // many-core machines (mode 0 / the extract+decorate passes); clamp to a sane
    // 1..=32. The GPU-dispatcher modes are bounded by the single GPU dispatcher,
    // so extra workers there mostly help the CPU-side pre/post passes.
    let mesh_workers: usize = std::env::var("UNENDLICH_MESH_WORKERS")
        .ok().and_then(|s| s.parse::<usize>().ok())
        .map(|n| n.clamp(1, 32))
        .unwrap_or(MESH_WORKERS);
    // r476: mesh queue capacity is now a lever too (UNENDLICH_MESH_QUEUE_CAPACITY,
    // default 64). Larger = workers stay fed during big streaming bursts; smaller
    // = tighter back-pressure so the queue can't balloon to 250+ (as seen on
    // Asahi when the octree mesher falls behind). Clamp 8..=1024.
    let mesh_queue_cap: usize = std::env::var("UNENDLICH_MESH_QUEUE_CAPACITY")
        .ok().and_then(|s| s.parse::<usize>().ok())
        .map(|n| n.clamp(8, 1024))
        .unwrap_or(MESH_QUEUE_CAPACITY);

    // Declared second → dropped FIRST.
    let mesh_pool = if let Some(arc) = &_octree_gpu_mesher_for_cleanup {
        let pool = MeshingPool::with_octree_gpu_mesher(
            mesh_workers,
            mesh_queue_cap,
            Arc::clone(&lod_settings),
            Arc::clone(arc),
        );
        log::info!(
            "MeshingPool ready ({} workers + Octree-GPU dispatcher, capacity {})",
            mesh_workers, mesh_queue_cap
        );
        pool
    } else if let Some(arc) = &_gpu_mesher_for_cleanup {
        let pool = MeshingPool::with_gpu_mesher(
            mesh_workers,
            mesh_queue_cap,
            Arc::clone(&lod_settings),
            Arc::clone(arc),
        );
        log::info!(
            "MeshingPool ready ({} workers + Sparse-GPU dispatcher, capacity {})",
            mesh_workers, mesh_queue_cap
        );
        pool
    } else {
        let pool = MeshingPool::new(mesh_workers, mesh_queue_cap, Arc::clone(&lod_settings));
        log::info!("MeshingPool ready ({} workers, capacity {}; CPU-only; \
                    UNENDLICH_GPU_MESHER=1 for sparse-records GPU mesher, \
                    =2 for octree GPU mesher)",
            mesh_workers, mesh_queue_cap);
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
    // r126: observable day/night — UNENDLICH_DAY_LEN=secs overrides (default
    // 1200 = 20 min/day; engine default was longer, sun barely moved per run).
    {
        let day_len: f32 = std::env::var("UNENDLICH_DAY_LEN")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1200.0);
        sky_state.day_length = day_len;
        sky_state.time_seconds = (day_len as f64) * 0.30; // start mid-morning
        log::info!("[sky] day length {}s (UNENDLICH_DAY_LEN to change)", day_len);
    }

    // Weather layer — DEFAULT ON since r116 (wind/atmosphere modulation was
    // built and proven but nobody flipped the env var); UNENDLICH_WEATHER=0
    // opts out.
    let weather_enabled = std::env::var("UNENDLICH_WEATHER")
        .ok()
        .map(|v| !(v == "0" || v.eq_ignore_ascii_case("false")))
        .unwrap_or(true);
    let weather_seed = std::env::var("UNENDLICH_WEATHER_SEED")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0x5eed_1234_abcd_0001);
    let mut weather_state = unendlich_engine::weather::Weather::new(weather_seed);

    // r495: SEASONS. The natural year is long, so seasons are hard to witness in a
    // session — these levers make them observable. UNENDLICH_YEAR_DAYS sets year
    // length; UNENDLICH_SEASON=spring|summer|autumn|winter (or 0..1) forces a
    // starting season; the O key advances the season ~1/8 year for live testing.
    // season_offset_days is added to the climate day-of-year (drives temp/snow)
    // and the plant-growth season01.
    let season_days_per_year: f32 = std::env::var("UNENDLICH_YEAR_DAYS")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .filter(|d| *d >= 4.0)
        .unwrap_or(360.0);
    let mut season_offset_days: f32 = match std::env::var("UNENDLICH_SEASON")
        .ok()
        .map(|s| s.to_ascii_lowercase())
        .as_deref()
    {
        Some("spring") => 0.0,
        Some("summer") => season_days_per_year * 0.25,
        Some("autumn") | Some("fall") => season_days_per_year * 0.50,
        Some("winter") => season_days_per_year * 0.75,
        Some(other) => other
            .parse::<f32>()
            .ok()
            .map(|p| p.fract() * season_days_per_year)
            .unwrap_or(0.0),
        None => 0.0,
    };
    let mut last_logged_season: Option<unendlich_engine::season::Season> = None;

    // Load-adaptive fog — DEFAULT ON since r120 (hides the unloaded render-
    // distance edge in haze; built+proven, was only dark behind the env var).
    // UNENDLICH_LOAD_FOG=0 opts out.
    let load_fog_enabled = std::env::var("UNENDLICH_LOAD_FOG")
        .ok()
        .map(|v| !(v == "0" || v.eq_ignore_ascii_case("false")))
        .unwrap_or(true);

    // Diagnostic (UNENDLICH_NO_FOG=1): force fog_density to 0 so distant
    // geometry renders at full contrast instead of fading into the sky-
    // colored haze. Lets us tell a *missing* face apart from one that's
    // merely fogged to the background color. Default off.
    let no_fog_enabled = std::env::var("UNENDLICH_NO_FOG")
        .ok()
        .map(|v| v == "1")
        .unwrap_or(false);

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
    // r199: load persisted keybinds; fall back to the QWERTY preset. The
    // hotkey rebinding page (Controls tab) writes this file on every
    // change so bindings survive a restart.
    let keybinds_path: std::path::PathBuf = std::env::var("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|_| std::env::var("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))
        .map(|p| p.join("unendlich/keybinds.json"))
        .unwrap_or_else(|_| std::path::PathBuf::from("unendlich-keybinds.json"));
    let mut bindings = KeyBindings::load_from_file(&keybinds_path)
        .unwrap_or_else(|_| KeyBindings::qwerty_preset());

    let mut input = InputCache::default();
    let mut mouse_captured = false;
    // v0.24.NEXT-input-p-toggle: press `p` to enter "panel mode" — cursor is
    // released, click-to-recapture is suppressed, and pointer events route to
    // egui so the creative inventory / settings panels can be clicked. Press
    // `p` again (or Esc) to exit. `ui_mouse_down` is the live left-button
    // state we forward to the renderer; tracked from MouseButton events.
    let mut p_pressed = false;
    let mut ui_mouse_down = false;

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
    let mut selected_palette_idx: usize = 0; // r122: scroll-wheel hotbar cursor
    // r454: CREATIVE TABS — browse all placeable blocks by category (sim::creative_tabs). G cycles
    // the tab, V cycles the block within it; the picked block becomes the creative placement block.
    let mut creative_tab_idx: usize = 0;
    let mut creative_item_idx: usize = 0;
    let mut hand_item: Option<unendlich_engine::items::ItemId> = None; // r127: raw hand item (survival)
    // r481: debug override — cycle forged weapon models into the hand with J so
    // all eight can be eyeballed first-person without the full loot plumbing.
    let mut debug_weapon_class: Option<usize> = None;
    // r509: TOOL/WEAPON DURABILITY (sim::item_durability). Tracks wear of the currently
    // HELD forged weapon (ids 4000-4007): each mine/attack in survival lowers it, and at 0
    // the weapon breaks (one removed from the inventory). v1 tracks only the equipped
    // weapon — switching weapons re-initialises the tracker; a per-stack instance table is
    // the documented next step (NEXT_SESSION_GUIDE "ITEMS WIRING") for persisting durability.
    let mut equipped_weapon_dura: Option<(unendlich_engine::items::ItemId, i32, i32)> = None; // (id, cur, max)
    const WEAPON_DURA_MAX: i32 = 120;
    const WEAR_PER_ATTACK: i32 = 2;
    const WEAR_PER_MINE: i32 = 1;
    // r490: third-person self-view (F5). The render camera pulls back behind the
    // player and the chosen character is drawn as a Creature avatar at the feet,
    // so the player can actually SEE their character. First-person stays the
    // default; raycast/physics/audio always use the real eye (local_player.view).
    // r490/r515: CAMERA VIEWS. 0 = first-person, 1 = third-person (pulled back behind you),
    // 2 = cinematic over-the-shoulder. F5 toggles first/third; Y toggles the cinematic shoulder
    // cam. `fp_body` renders your own avatar in first-person so you can look down and see your
    // body (the hand viewmodel is hidden then to avoid a second floating arm). Default on.
    let mut camera_mode: u8 = 0;
    // r534: third-person / cinematic camera-distance zoom (+/- keys). 1.0 = default
    // pull-back; clamped so the camera can't clip into the player or fly too far.
    let mut cam_zoom: f32 = 1.0;
    // r540: CAMERA JUICE. `bob_phase` advances with footstep cadence to drive a
    // subtle first-person walk bob; `cam_smoothed` eases the chase camera toward
    // its target so pinch/zoom changes glide instead of snapping. Both are
    // render-only (they never touch the logical eye used for physics/raycast).
    let mut bob_phase: f32 = 0.0;
    let mut prev_bob_phase: f32 = 0.0; // r566: footfall edge detection for ground-shake
    let mut bob_amp_s: f32 = 0.0; // smoothed amplitude (settles to 0 when idle)
    let mut cam_smoothed: Option<glm::Vec3> = None;
    // r542: impulse camera shake — `shake_mag` is kicked up by hard landings (and
    // other impacts) and decays each frame; the offset is multi-frequency so it
    // reads as a jolt, not a wobble. Render-only.
    let mut shake_mag: f32 = 0.0;
    let mut shake_t: f32 = 0.0;
    let view_bob_on = std::env::var("UNENDLICH_VIEW_BOB")
        .map(|v| v != "0" && !v.is_empty()).unwrap_or(true);
    let cam_smooth_on = std::env::var("UNENDLICH_CAM_SMOOTH")
        .map(|v| v != "0" && !v.is_empty()).unwrap_or(true);
    let cam_shake_on = std::env::var("UNENDLICH_CAM_SHAKE")
        .map(|v| v != "0" && !v.is_empty()).unwrap_or(true);
    let mut fp_body: bool = true;
    let mut self_avatar: Option<unendlich_engine::creature::Creature> = None;
    let mut self_avatar_dirty = true; // rebuild from active_character on next use
    let mut self_avatar_prev: Option<[f32; 3]> = None; // for deriving avatar velocity
    // r561: STATURE physical coupling. The active character's height drives the
    // collider, the camera eye height, and locomotion via sim::stature. These are
    // recomputed whenever the character changes (the self_avatar_dirty path) and
    // anchored at the 1.8 person, so a default character is byte-for-byte today's
    // tuning. `player_eye_offset` is the eye height ABOVE the AABB centre (so
    // camera = body.render_y() + player_eye_offset).
    let mut player_stature =
        unendlich_engine::sim::stature::derive(1.8, 1.0, 1.0, 1.0);
    let mut player_eye_offset: f32 =
        player_stature.eye_height - player_stature.half_extents[1];
    let mut player_eye_height: f32 = player_stature.eye_height;
    // r563: incoming-vitality-damage multiplier from the stature's max_health
    // (the player's vitality is normalised [0,1], so "more health" = take a
    // smaller fraction per hit). = 1.0 for a default character.
    let mut player_dmg_resist: f32 =
        unendlich_engine::sim::stature::BASE_HEALTH / player_stature.max_health;
    // r539: hero walk cycle — phase advances over time, amplitude tracks horizontal
    // speed (→0 when idle so the limbs settle), passed to hero_forge::build.
    let mut hero_gait_phase: f32 = 0.0;
    let mut hero_gait_amp: f32 = 0.0;
    // r570: airborne/landing pose state — `hero_airborne01` eases toward 1 while the
    // player is off the ground (fall tuck), `hero_land_crouch` is pulsed to ~1 on a
    // hard touchdown and decays (landing absorb).
    let mut hero_airborne01: f32 = 0.0;
    let mut hero_land_crouch: f32 = 0.0;
    // r572: eased horizontal-flight blend (1 while actively flying & moving).
    let mut hero_flight01: f32 = 0.0;
    // r573: flight banking — eased roll into turns, from the view yaw rate.
    let mut hero_bank: f32 = 0.0;
    let mut prev_view_yaw: f32 = 0.0;
    // r486: held-item swing — a short arc the viewmodel plays each time a block
    // action fires (mine/place), so the hand item visibly swings on use.
    let mut swing_timer: f32 = 0.0;
    const SWING_DURATION: f32 = 0.24;
    let mut prev_fall_vy: f32 = 0.0; // r141: landing speed for fall impact
    // r508: tracks an in-progress jump ascent so releasing Jump early can cut it short
    // (variable jump height — tap = short hop, hold = full jump).
    let mut jump_ascending = false;
    // r265: PARTICLES (kill switch, default OFF). A pooled CPU particle system
    // (sim::particles) stepped each frame; emits a Dust burst at the player's
    // feet on landing. The system TICK (emit/step/retire) is pure CPU and runs
    // when enabled; the GPU upload of render_buffer() is a separate hookup that
    // needs the renderer (left as a marked TODO — not validated without a GPU).
    // UNENDLICH_PARTICLES=1 to enable. See sim/particles.rs.
    let particles_enabled: bool = std::env::var("UNENDLICH_PARTICLES")
        .map(|v| v != "0").unwrap_or(true);
    let mut particle_sys = if particles_enabled {
        log::info!("[particles] ENABLED — CPU particle system active (Dust on landing); GPU upload pending");
        Some(unendlich_engine::sim::particles::ParticleSystem::with_default_cap(0x9165_2C3A))
    } else {
        None
    };
    let mut last_player_speed: f32 = 0.0; // r148: viewmodel bob input
    // r156b: frame-persistent — the weather tick runs LATE in the frame,
    // but the fire-douse and exposure blocks run before it; they read last
    // frame's value (one-frame lag, harmless).
    let mut precip01: f32 = 0.0;
    let mut last_climate_temp_c: f32 = 12.0; // r492: latest column temp, for rain-vs-snow
    // r641: fog extinction at the player's column, refreshed each climate tick and
    // reused by creature vision so mobs can't see through fog the player can't see
    // through. Built from the SAME base + humidity the fragment shader fogs with
    // (see sim::fog_model::shader_fog_density); 0 = clear air.
    let mut frame_fog_sigma: f32 = 0.0;
    let mut current_world_name: Option<String> = None; // r165: for F6 catalog writes
    let mut sleep_hold: f32 = 0.0; // r165: sneak-to-sleep accumulator
    // r510: COOKING — holding raw meat near a fire cooks it over ~COOK_TIME seconds into
    // cooked meat (more nourishing). This accumulates while the conditions hold; it decays
    // back when you step away or stop holding meat.
    let mut cook_progress: f32 = 0.0;
    const COOK_TIME: f32 = 4.0;
    // r512/r514: FARMING — planted crops live in the persistent `session.flora` store
    // (world/flora_store.rs), keyed by cell, round-tripped to disk. (Replaced the old
    // bin-local Vec so crop growth survives save/load.)
    // r522: SEED GENETICS — a per-world seed line per crop (Carrot/Potato/Cabbage). Each planting
    // crosses the line with a fresh genome (sim::plant_tree_genetics) and stores the result on the
    // plant; harvesting a ripe crop breeds its genome back into the line, so a grower who keeps
    // replanting their best harvests improves yield/growth over generations. Genome traits drive
    // growth speed + harvest yield; the per-plant genome persists in the flora store.
    let mut seed_stock: [unendlich_engine::sim::plant_tree_genetics::PlantGenome; 3] =
        [unendlich_engine::sim::plant_tree_genetics::PlantGenome::neutral(); 3];
    let fg_loot_table = unendlich_engine::items::loot::default_loot_table(); // r149
    // r142: body thermal state (sim::survival_exposure) — lags felt temp.
    let mut fg_thermal = unendlich_engine::sim::survival_exposure::Thermal {
        body_c: 36.5,
        comfort: 0.0,
    };
    // r545 (Roll 3 / C2): player hydration. Reuses the dragon_thirst::Hydration
    // reserve (drain/drink/parched). Player drain is slowed vs the dragon constant
    // by scaling dt at the tick site so thirst is a periodic chore, not constant.
    let mut fg_hydration = unendlich_engine::sim::dragon_thirst::Hydration::full();
    // r130: PLAYER BENDING — controller + the bend key edge state.
    let mut player_bending = unendlich_engine::sim::player_bending::PlayerBending::new();
    // r131: the real resonance economy (sim::bending_charge) replaces the
    // controller's local regen stub — build while still, spend per cast.
    let mut bend_resonance = unendlich_engine::sim::bending_charge::Resonance::full();
    // r670: lifetime chi spent on bending — feeds sim::player_rank (Resonance
    // Rank). r671: loaded from / saved to the active character profile so Rank
    // persists across sessions. Each unit of resonance spent counts as 20 chi.
    let mut fg_lifetime_chi: u64 = active_character.lifetime_chi;
    // r267: ABILITY CAST (kill switch, default OFF). Turns a bending DISCHARGE
    // into a typed ability Effect (sim::ability_cast) gated by a cooldown
    // (sim::ability_cooldown) and amplified when chained inside a combo window
    // (sim::ability_combo). The discharge + spent power already exist; this gives
    // them a typed kind/power/radius + cast pacing. UNENDLICH_ABILITY_CAST=1.
    let ability_cast_enabled: bool = std::env::var("UNENDLICH_ABILITY_CAST")
        .map(|v| v != "0").unwrap_or(true);
    let mut ability_last_cast: f32 = -1000.0; // game-seconds of last cast (cooldown ref)
    let mut ability_chain: u8 = 0;            // current combo chain length
    let mut ability_last_chain_t: f32 = -1000.0; // for combo-window expiry
    let ability_elapsed = std::time::Instant::now(); // cast clock
    if ability_cast_enabled {
        log::info!("[ability] ENABLED — bending discharge produces typed ability casts (cooldown + combo)");
    }
    // r268: COMBAT MODIFIERS (kill switch, default OFF). Layers a deterministic
    // crit roll (sim::crit_roll) + natural-armor mitigation (sim::damage_after_
    // armor, armor derived from creature mass) onto the existing punch damage.
    // UNENDLICH_COMBAT_MODS=1.
    let combat_mods_enabled: bool = std::env::var("UNENDLICH_COMBAT_MODS")
        .map(|v| v != "0").unwrap_or(true);
    if combat_mods_enabled {
        log::info!("[combat] MODIFIERS ENABLED — crit rolls + mass-based armor mitigation on melee");
    }
    // rNNN: LIGHTNING ARC (kill switch, default OFF). When on, a melee strike on
    // a creature in a herd chains lightning to nearby foes via the real
    // legendary_weapon swing resolver (r369 resolve_weapon_swing, Storm blade).
    // Purely additive — default melee is unchanged. UNENDLICH_LIGHTNING_ARC=1.
    let lightning_arc_enabled: bool = std::env::var("UNENDLICH_LIGHTNING_ARC")
        .map(|v| v != "0").unwrap_or(false);
    if lightning_arc_enabled {
        log::info!("[lightning] ARC ENABLED — melee strikes chain to nearby foes (lightning weapon)");
    }
    // rNNN: WEATHER SIM readout (kill switch, default OFF). When on, the physical
    // weather model (world::weather_state) is sampled for the player's column on
    // an interval and logged — a live check that the composed sim (cloud / precip
    // / storm / snow) produces sane numbers before it drives any rendering. Pure
    // readout: it touches no gameplay or render state. UNENDLICH_WEATHER_SIM=1.
    let weather_sim_enabled: bool = std::env::var("UNENDLICH_WEATHER_SIM")
        .map(|v| v != "0").unwrap_or(true);
    if weather_sim_enabled {
        log::info!("[weather] SIM ENABLED — physical weather sampled for the player's column (readout only)");
    }
    let mut weather_snow_depth: f32 = 0.0;
    let mut weather_last_sample = std::time::Instant::now();
    // rNNN: WING BUFFET (kill switch, default OFF). A flapping (airborne) dragon's
    // downwash deflects nearby creatures' leap arcs via sim::wing_buffet (r378).
    // Additive + gated; sources are precomputed each frame before the mutable
    // roamer loop. UNENDLICH_WING_BUFFET=1.
    let wing_buffet_enabled: bool = std::env::var("UNENDLICH_WING_BUFFET")
        .map(|v| v != "0").unwrap_or(true);
    if wing_buffet_enabled {
        log::info!("[buffet] WING BUFFET ENABLED — flapping dragons deflect nearby leaping creatures");
    }
    // r268: AGGRO RESPONSE (kill switch, default OFF). A harmed creature builds
    // aggro (capped by aggression_level from its health), which decays over time
    // + distance (aggro_decay). When aggro is high it APPROACHES the threat
    // instead of fleeing — fauna fight back. UNENDLICH_AGGRO=1.
    let aggro_enabled: bool = std::env::var("UNENDLICH_AGGRO")
        .map(|v| v != "0").unwrap_or(true);
    if aggro_enabled {
        log::info!("[aggro] ENABLED — harmed creatures build aggression and approach instead of flee");
    }
    // r269: BEHAVIOR ARBITER (kill switch, default OFF). A utility-AI selector
    // (sim::behavior_arbiter) that scores Flee/Fight/Forage/Rest/Patrol from the
    // roamer's drive scalars (threat/hunger/fatigue/aggression/health) and picks
    // the argmax — replacing the ad-hoc aggro>0.5 fight/flee threshold with a
    // principled choice. UNENDLICH_ARBITER=1 (implies the aggro field is read).
    let arbiter_enabled: bool = std::env::var("UNENDLICH_ARBITER")
        .map(|v| v != "0").unwrap_or(true);
    if arbiter_enabled {
        log::info!("[arbiter] ENABLED — utility-AI behavior selection (flee/fight/forage/rest/patrol)");
    }
    // r284: WITNESSED ATTACKS (kill switch, default OFF). The perception-driven
    // replacement for omniscient herd knowledge: when a creature is harmed it
    // records an attack event; nearby creatures that can SEE it (within witness
    // sight range) REACT via their drives — a bold same-species herdmate's aggro
    // rises (move to help/defend), a timid one flees. Event-driven (only on harm)
    // and perception-gated (local sight), so it's ~free when no combat is
    // happening — unlike the O(n^2) per-frame herd scan, which is really a
    // deep-time/sim-vector population operation, not a real-time one.
    // UNENDLICH_WITNESS=1. The disposition is shaped by aggro/habit now; genetics
    // (a heritable temperament gene) will shape it across generations.
    let witness_enabled: bool = std::env::var("UNENDLICH_WITNESS")
        .map(|v| v != "0").unwrap_or(true);
    if witness_enabled {
        log::info!("[witness] ENABLED — creatures react to attacks they can see (help/defend or flee)");
    }
    // r287: POI SCORING (kill switch, default OFF; depends on UNENDLICH_ARBITER).
    // Routes the arbiter's Drives through sim::poi — the player/threats become Foe
    // POIs scored by the creature's PoiWeights (default until the genome loci land).
    // The unifying near-term decision substrate (food/shelter/friend/foe).
    // UNENDLICH_POI=1.
    let poi_enabled: bool = std::env::var("UNENDLICH_POI")
        .map(|v| v != "0").unwrap_or(false);
    if poi_enabled {
        log::info!("[poi] ENABLED — behaviour drives assembled from scored POIs (food/shelter/friend/foe)");
    }
    // Per-frame buffer of attack events (victim position + kind), populated at the
    // harm points and read by the roamer loop's witness check, then cleared.
    // Usually empty (no combat) → the witness check is a cheap empty-Vec scan.
    let mut attack_events: Vec<([f32; 3], u8)> = Vec::new();
    // r138: per-dragon bending wind-up state (keyed by flight-phase id bits).
    // r139: artifact imbue — the carried ingot (if any) slowly attunes to the
    // local element; its FieldDelta then adds onto the bending fields.
    let mut fg_imbue = unendlich_engine::sim::artifact_imbue::ImbueState {
        counter: 0.0,
        element: 2, // Fire-leaning by default; match01 drives actual progress
        tier: 2,
    };
    // r150: dragon daily life — activity (sleep/forage/alert) + hunt state.
    let mut scroll_accum: f32 = 0.0;
    let mut scroll_accum_x: f32 = 0.0; // r318: horizontal (trackpad) scroll
    // r476: timebend snapshot/scrub camera mode — records pose+sky-time as the
    // player roams; F8 freezes the world and scrolls back through those vantages.
    let mut timebend = unendlich_engine::timebend_scrub::TimebendScrub::new();
    // r476: "detached terrain" — when the player moves over ground that hasn't
    // streamed in yet, suspend their fall (hover) instead of plummeting through
    // the void, then settle them onto the real surface once it spawns. Default
    // on; opt out with UNENDLICH_DETACHED_TERRAIN=0.
    let detached_terrain = std::env::var("UNENDLICH_DETACHED_TERRAIN")
        .map(|v| v != "0" && !v.is_empty()).unwrap_or(true);
    let mut terrain_suspended = false;
    let mut last_jump_tap: Option<Instant> = None; // r319: double-tap Space -> flight

    // -----------------------------------------------------------------------
    // Main loop
    // -----------------------------------------------------------------------
    let start = Instant::now();
    // r412: dynamic in-chunk water sim (experimental, gated UNENDLICH_WATER_SIM=1). Reads
    // each nearby chunk's water as a field, settles it toward its level, and writes back
    // only Air↔Water cells (never touching terrain). OFF by default.
    // r539: the default had drifted to `true`, so the experimental settle ran for everyone —
    // and with rain feeding water into exposed columns each tick (apply_rain below), bodies
    // visibly CLIMB during/after rain ("water rising strangely"). Restored to off; the sim is
    // still backlog-blocked on a persistent per-chunk field, so opt in with UNENDLICH_WATER_SIM=1
    // for testing only.
    let water_sim_enabled =
        std::env::var("UNENDLICH_WATER_SIM").map(|v| v != "0" && !v.is_empty()).unwrap_or(true);
    // r637: DEFAULT ON. Water lateral spread (settle toward level) was gated off since
    // r539 for perf; the operator wants water to flow to the sides, so it's on by default now.
    // It's budgeted/throttled (radius/budget/interval below). Disable with UNENDLICH_WATER_SIM=0
    // if it costs too much on a given machine.
    // Tunable for bulk testing: chunk radius, set_block budget/tick, tick interval (ms).
    let water_sim_radius: i32 = std::env::var("UNENDLICH_WATER_RADIUS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3) // r428: wider window so more of a body flows toward a hole
        .clamp(0, 8);
    let water_sim_budget: usize = std::env::var("UNENDLICH_WATER_BUDGET")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3000) // r428: was 1500 — less throttling while a cave drains
        .clamp(64, 100_000);
    let water_sim_interval_ms: u64 = std::env::var("UNENDLICH_WATER_INTERVAL_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(250)
        .clamp(50, 5000);
    if water_sim_enabled {
        log::info!(
            "[water-sim] dynamic water ON (radius={water_sim_radius} budget={water_sim_budget} interval={water_sim_interval_ms}ms; UNENDLICH_WATER_*)"
        );
    }
    let mut water_sim_last_ms: u64 = 0;
    // opt-1: only simulate when something changed. `water_dirty` is set by edits/loads;
    // `water_active` stays set while water is still settling. Idle settled water = no work.
    let mut water_dirty: bool = water_sim_enabled;
    let mut water_active: bool = false;
    // r436: diagnostics surfaced on the [water] line so WATER_DEBUG alone shows whether the sim
    // is moving water (sideways/down) — changes = water blocks rewritten this tick, flowspd =
    // dominant current in the window, cols = columns simulated.
    let mut dbg_water_changes: usize = 0;
    let mut dbg_flow_speed: f32 = 0.0;
    let mut dbg_water_cols: usize = 0;
    // r417 (phase 1): flow-field producer. The sim accumulates per-column current vectors
    // from the settle; later phases upload this so the surface shader streams waves along
    // the flow (blended with wind). Sized to the (fixed) sim radius. Debug readout:
    // UNENDLICH_WATER_FLOW_DEBUG=1 logs the dominant flow direction/speed near the player.
    let water_flow_dbg =
        std::env::var("UNENDLICH_WATER_FLOW_DEBUG").map(|v| v != "0").unwrap_or(false);
    // r419 (flow-as-shader): when on, feed the persisted flow tiles to the water surface
    // shader so waves stream along the current (blended with wind). Default off → wind-only,
    // rendering identical to before.
    let water_flow_shader =
        std::env::var("UNENDLICH_WATER_FLOW_SHADER").map(|v| v != "0").unwrap_or(true);
    if water_flow_shader {
        log::info!("[water-flow] surface shader flow ON (UNENDLICH_WATER_FLOW_SHADER)");
    }
    // r506: BODY PUSH-BACK / WAVES — a body of some mass moving through water shoves a wake
    // into the flow field (sim::water_react::wave_amplitude → flow_tiles.splash), so the
    // surface visibly parts and foams around a swimmer or a dragon crashing into a lake.
    // Reuses the flow-shader path, so it's inert when UNENDLICH_WATER_FLOW_SHADER=0. Default
    // on; gain/max tune wake strength and cap. UNENDLICH_WATER_PUSHBACK=0 to disable.
    let water_pushback =
        std::env::var("UNENDLICH_WATER_PUSHBACK").map(|v| v != "0").unwrap_or(true);
    let water_pushback_gain: f32 = std::env::var("UNENDLICH_WATER_PUSHBACK_GAIN")
        .ok().and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.05).max(0.0);
    let water_pushback_max: f32 = std::env::var("UNENDLICH_WATER_PUSHBACK_MAX")
        .ok().and_then(|v| v.parse::<f32>().ok()).unwrap_or(4.0).max(0.0);
    if water_pushback {
        log::info!("[water-pushback] ON — moving bodies make wakes/waves (gain={water_pushback_gain}, max={water_pushback_max}; UNENDLICH_WATER_PUSHBACK=0 to disable)");
    }
    // r420: verbose water diagnostics ([water] prefix) + wave-speed knob to chase the
    // "too fast with the wind" report. wave-speed multiplies the wave animation only
    // (not amplitude); 1.0 = unchanged.
    let water_debug =
        std::env::var("UNENDLICH_WATER_DEBUG").map(|v| v != "0").unwrap_or(true);
    // r437: echo/reverb readout at the player (validates sim/voice_spatial live).
    let echo_debug =
        std::env::var("UNENDLICH_ECHO_DEBUG").map(|v| v != "0").unwrap_or(false);
    // Seed the in-game Debug toggles (Settings → Debug) from env so CLI flags still work. Master
    // is OFF unless something asks for logging; it gates everything below it.
    renderer.ui_debug_water = water_debug;
    renderer.ui_debug_flow = water_flow_dbg;
    renderer.ui_debug_echo = echo_debug;
    renderer.ui_debug_master = water_debug
        || water_flow_dbg
        || echo_debug
        || std::env::var("UNENDLICH_DEBUG_LOG").map(|v| v != "0").unwrap_or(false);
    let water_wave_speed: f32 = std::env::var("UNENDLICH_WATER_WAVE_SPEED")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(0.45) // r426: slower default — the old 1.0 read as ~1Hz chop on big lakes
        .clamp(0.0, 4.0);
    let mut water_debug_last_ms: u64 = 0;
    let mut water_dbg_prev_tod: f32 = 0.0;
    // r422: settle tunables. More iterations = faster settling (cheap). The per-neighbour
    // factor MUST stay < 0.25 for stability (0.125 is the checkerboard-optimal value); the
    // clamp enforces that so a bad value can't reintroduce the r420 instability.
    let water_settle_iters: usize = std::env::var("UNENDLICH_WATER_ITERS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(8) // r441: reverted from r428's 12 — fewer iters compounds the checkerboard less
        .clamp(1, 64);
    let water_settle_f: f32 = std::env::var("UNENDLICH_WATER_SETTLE_F")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(0.125) // r441: reverted from r428's 0.15 — r421-proven stable value
        .clamp(0.02, 0.24);
    // r442: per-column per-tick flow cap (blocks). The settle equalizes a small dug hole within
    // a single tick's iterations, so it fills "instantly" (operator: settled a little too fast).
    // Capping how much a column may shed per tick spreads the fill over a few ticks so the flow is
    // visible, while staying volume-conserving (un-shed water waits for next tick). High = ~off.
    let water_max_flow: f32 = std::env::var("UNENDLICH_WATER_MAX_FLOW")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(3.0)
        .max(0.05);
    // r442: CURRENT FORCE — the water flow field pushes a submerged body (operator: "with flow
    // comes force"). Default on + gentle; tune strength/cap or disable via env.
    let water_current_on: bool = std::env::var("UNENDLICH_WATER_CURRENT")
        .ok()
        .map(|v| v != "0")
        .unwrap_or(true);
    let water_current_strength: f32 = std::env::var("UNENDLICH_WATER_CURRENT_STRENGTH")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(1.5)
        .max(0.0);
    let water_current_max: f32 = std::env::var("UNENDLICH_WATER_CURRENT_MAX")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(4.0)
        .max(0.0);
    // r422: temporal smoothing of the flow field fed to the shader (EMA weight of the new
    // sample). Lower = smoother/laggier current; 1.0 = no smoothing.
    let water_flow_smooth: f32 = std::env::var("UNENDLICH_WATER_FLOW_SMOOTH")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(0.35)
        .clamp(0.05, 1.0);
    // r423: evaporation of isolated shallow puddles (off by default). Dries stranded thin
    // water without eroding lakes (a column only evaporates if it AND all neighbours are
    // shallow). rate is depth/tick removed.
    let water_evap = std::env::var("UNENDLICH_WATER_EVAP").map(|v| v != "0").unwrap_or(true);
    let water_evap_rate: f32 = std::env::var("UNENDLICH_WATER_EVAP_RATE")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(0.04)
        .clamp(0.0, 1.0);
    // r424: rain fills exposed columns into shallow puddles (capped, can't flood). Best with
    // evaporation so the puddles dry after the rain — so rain auto-enables it.
    let water_rain = std::env::var("UNENDLICH_WATER_RAIN").map(|v| v != "0").unwrap_or(true);
    let water_rain_rate: f32 = std::env::var("UNENDLICH_WATER_RAIN_RATE")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(0.08)
        .clamp(0.0, 1.0);
    let water_rain_cap: f32 = std::env::var("UNENDLICH_WATER_RAIN_CAP")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(2.0)
        .clamp(1.0, 8.0);
    let water_evap = water_evap || water_rain; // rain needs drying to cycle
    // r425: surface water freezes to Ice in cold climate (and melts back). Uses the climate
    // temperature at the player; hysteresis avoids flicker at the boundary. While frozen the
    // window's water is held static (settle skipped) and the surface cell is Ice.
    let water_ice = std::env::var("UNENDLICH_WATER_ICE").map(|v| v != "0").unwrap_or(true);
    let water_ice_freeze: f32 = std::env::var("UNENDLICH_WATER_ICE_TEMP")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(0.12)
        .clamp(0.0, 0.9);
    let water_ice_melt = water_ice_freeze + 0.08;
    let mut water_ice_frozen: bool = false;
    // r426: how far below the player the sim scans for a floor. Deeper = water drains into
    // caves/holes farther down (at more cost per tick). Water still can't pass through solid
    // rock — only down open holes/gaps.
    let water_drain_depth: i32 = std::env::var("UNENDLICH_WATER_DEPTH")
        .ok()
        .and_then(|v| v.parse::<i32>().ok())
        .unwrap_or(80)
        .clamp(16, 240);
    // r427: underwater screen FX (tint + caustic shimmer when the camera is submerged).
    // On by default — inert unless the eye is in water. Opt out with =0.
    let water_underwater_fx =
        std::env::var("UNENDLICH_WATER_UNDERWATER_FX").map(|v| v != "0").unwrap_or(true);
    // r431: feed real per-column water depth to the surface shader for depth-color (shallow→clear,
    // deep→dark) instead of the view-angle guess. Covers the sim window. Default on with the sim.
    let water_depth_color =
        std::env::var("UNENDLICH_WATER_DEPTH_COLOR").map(|v| v != "0").unwrap_or(true);
    // r433: shoreline erosion — waterline Grass/Dirt washes to Sand over time. Gated + budgeted
    // (per sim tick, while water is active). The one place the water sim edits terrain.
    let water_erosion =
        std::env::var("UNENDLICH_WATER_EROSION").map(|v| v != "0").unwrap_or(true);
    let water_erosion_budget: usize = std::env::var("UNENDLICH_WATER_EROSION_BUDGET")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(2)
        .clamp(1, 256);
    // r428: water that ends up sitting on Lava boils off (with a steam puff) instead of
    // pooling — answers "what happens when water meets the hot underground". A full
    // steam-pressure-pushback model is out of scope; this is the boil-away interaction.
    let water_boil = std::env::var("UNENDLICH_WATER_BOIL").map(|v| v != "0").unwrap_or(true);
    let water_boil_rate: f32 = std::env::var("UNENDLICH_WATER_BOIL_RATE")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(0.75)
        .clamp(0.05, 4.0);
    // r429: boil threshold in °C from the live heat field (sources sum). ~45°C per the
    // operator's hot-underground case; lava sits far above this.
    let water_boil_temp: f32 = std::env::var("UNENDLICH_WATER_BOIL_TEMP")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(45.0)
        .clamp(10.0, 1000.0);
    // r430: max steam back-pressure (in block-heights) a boiling column adds to its effective
    // surface, repelling inflow so hot deep sections reach equilibrium. 0 = pure boil-off.
    let water_steam_pressure: f32 = std::env::var("UNENDLICH_WATER_STEAM_PRESSURE")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(4.0)
        .clamp(0.0, 32.0);
    let mut water_flow = {
        let rh = (water_sim_radius * 8).max(8);
        let side = (2 * rh + 1) as usize;
        unendlich_engine::world::water_flow_field::WaterFlowField::new(side, 0, 0)
    };
    // r444: LIVE FLOW EROSION — fast current over a soft bed (Sand/Gravel/Dirt/Grass) carves it
    // out, lifting the material into a suspended sediment load that the current carries downstream
    // and drops where it slows (channels cut + deltas build). Unlocked by the operator (testing
    // mode, terrain recoverable via timebending). Gated + budgeted; uses the tested cores.
    let water_flow_erosion =
        std::env::var("UNENDLICH_WATER_FLOW_EROSION").map(|v| v != "0").unwrap_or(true);
    let flow_erosion_threshold: f32 = std::env::var("UNENDLICH_WATER_FLOW_EROSION_THRESHOLD")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(0.35)
        .max(0.0);
    let flow_erosion_budget: usize = std::env::var("UNENDLICH_WATER_FLOW_EROSION_BUDGET")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(4)
        .clamp(1, 256);
    let flow_carry_capacity: f32 = std::env::var("UNENDLICH_WATER_SEDIMENT_CAPACITY")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(2.0)
        .max(0.0);
    let mut sediment = unendlich_engine::world::sediment_map::SedimentMap::new();
    // r444: LAVA meeting WATER solidifies — a source/deep lava face → obsidian (water survives),
    // thin flowing lava → stone (water flashes to steam). Gated; uses the tested contact rule.
    let lava_water_react =
        std::env::var("UNENDLICH_LAVA_WATER").map(|v| v != "0").unwrap_or(true);
    // r447: seconds after world load to force the water sim active around spawn so an ocean next
    // to a cave opening starts draining on load (0 disables the warm-up).
    let water_warmup_secs: u64 = std::env::var("UNENDLICH_WATER_WARMUP_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(12);
    let mut water_flow_dbg_last_ms: u64 = 0;
    // r418 (phase 2): persistent flow tiles. The per-tick window flow is ingested into
    // tiles that persist as the player moves (so a whole river keeps its current), with
    // decay for areas the sim stops refreshing and distance/age eviction to stay bounded.
    let mut water_flow_tiles =
        unendlich_engine::world::water_flow_tiles::FlowTileStore::new();
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
    // r293: SHELL-WALK TIME BUDGET. The submit (shell-walk) scan that decides
    // which chunks to mesh has no per-frame time cap — when the player moves into
    // new terrain it runs unbounded and stalls the frame (operator M2: submit
    // 400-868ms/sec → 90-108ms frames while streaming). Cap the wall-time spent
    // scanning per frame; the walk visits shells near→far (and forward-biased), so
    // when the budget is hit the nearest/most-important chunks are already queued
    // and the far ones simply resume next frame — a frame or two of extra pop-in
    // latency in exchange for no stall. Symmetric with the upload budget that
    // already caps the upload stage. UNENDLICH_SUBMIT_BUDGET_MS=F (0 = uncapped).
    let submit_budget_ms: f64 = std::env::var("UNENDLICH_SUBMIT_BUDGET_MS")
        .ok().and_then(|v| v.parse().ok())
        .unwrap_or(5.0);
    if submit_budget_ms > 0.0 {
        log::info!("submit_budget: ENABLED — shell walk capped at {:.1}ms/frame (far chunks resume next frame)", submit_budget_ms);
    }

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

    // r476: DYNAMIC render distance. When streaming overwhelms the frame (the
    // Asahi death-spiral: mesh_q backs up -> 2fps), shrink the effective render
    // radius so there's less to generate/mesh, then expand back once frames
    // recover. Runs on a WALL-CLOCK cadence (not frame-count — at 2fps a
    // 120-frame autotuner is 60s too late) and only ever caps BELOW the UI/env
    // render distance, never above. Gated (default off) so it can't surprise.
    // Levers: shrink when a frame exceeds SHRINK_MS, grow when under GROW_MS.
    let adaptive_rd = std::env::var("UNENDLICH_ADAPTIVE_RENDER_DISTANCE")
        .map(|v| v != "0" && !v.is_empty()).unwrap_or(false);
    let adaptive_rd_shrink_ms: f64 = std::env::var("UNENDLICH_ADAPTIVE_RD_SHRINK_MS")
        .ok().and_then(|s| s.parse().ok()).unwrap_or((frame_target_ms * 3.0).max(25.0));
    let adaptive_rd_grow_ms: f64 = std::env::var("UNENDLICH_ADAPTIVE_RD_GROW_MS")
        .ok().and_then(|s| s.parse().ok()).unwrap_or((frame_target_ms * 1.5).max(12.0));
    let adaptive_rd_min: i32 = std::env::var("UNENDLICH_ADAPTIVE_RD_MIN")
        .ok().and_then(|s| s.parse().ok()).map(|n: i32| n.clamp(2, 32)).unwrap_or(2);
    // r534: GROW SLEW LIMIT. Each +1 to the cap loads a whole new ring of chunks,
    // which at large distances is a heavy one-frame spike. Require N consecutive
    // good-frame checks before growing (so a single spike frame resets the streak
    // and PAUSES growth until the frame rate is genuinely stable), and start the cap
    // LOW so the world ramps up smoothly from spawn instead of slamming to max.
    let adaptive_rd_grow_streak: u32 = std::env::var("UNENDLICH_ADAPTIVE_RD_GROW_STREAK")
        .ok().and_then(|s| s.parse().ok()).map(|n: u32| n.clamp(1, 20)).unwrap_or(3);
    let mut adaptive_rd_good_streak: u32 = 0;
    let mut adaptive_rd_cap: i32 = adaptive_rd_min.max(6).min(32); // ramp up from here
    let mut adaptive_rd_last = std::time::Instant::now();
    if adaptive_rd {
        log::info!("adaptive_render_distance: ENABLED — shrink>{:.1}ms grow<{:.1}ms min={} (UNENDLICH_ADAPTIVE_RENDER_DISTANCE)",
            adaptive_rd_shrink_ms, adaptive_rd_grow_ms, adaptive_rd_min);
    }
    // r476: seed the Performance-page levers from their env vars so the env still
    // works as the initial value and the in-game controls reflect it. After this
    // the UI owns them live (the controller reads renderer.ui_adaptive_rd; the
    // per-frame block below pushes ui_gpu_mesh_yield into the global the mesher
    // dispatcher reads).
    renderer.ui_adaptive_rd = adaptive_rd;
    renderer.ui_gpu_mesh_yield = std::env::var("UNENDLICH_GPU_MESH_YIELD_PCT")
        .ok().and_then(|s| s.parse::<u64>().ok()).unwrap_or(0).min(200);
    unendlich_engine::constants::set_gpu_mesh_yield_pct(renderer.ui_gpu_mesh_yield);
    // r480: seed the new Performance-page toggles from env. octree_tile drives
    // the global atomic the mesh dispatch reads each batch; detached terrain
    // from UNENDLICH_DETACHED_TERRAIN.
    // r557: default the tiled shared-memory mesher path ON — it replaces the
    // per-cell "7 pointer-chasing octree walks" (slow on Asahi) with one
    // cooperative halo load into LDS + zero-descent neighbour reads, and is
    // verified byte-identical to the CPU reference. Set UNENDLICH_OCTREE_TILE=0
    // (or the Performance-page toggle) to fall back to the per-cell path.
    let octree_tile_env = !matches!(std::env::var("UNENDLICH_OCTREE_TILE"), Ok(s) if s == "0");
    unendlich_engine::constants::set_octree_tile(octree_tile_env);
    log::info!(
        "[mesh] octree tiled shared-memory path: {} (UNENDLICH_OCTREE_TILE)",
        if octree_tile_env { "ON" } else { "OFF" }
    );
    renderer.ui_octree_tile = octree_tile_env;
    renderer.ui_detached_terrain = detached_terrain;

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
    // r237: per-zone PERCENTILE profiler over the three frame phases the
    // existing t_*_us sums cover (manager-update / tick-sim / draw). The sums
    // average spikes away; this keeps histograms so p99/max (the stutter tail)
    // are visible. Opt-in via UNENDLICH_FRAME_PROFILE=1 (NOT UNENDLICH_PROFILE,
    // which is already the flag-bundle PRESET selector above — r245 fix for the
    // name collision that suppressed this profiler). Zero cost when off.
    let profile_on = std::env::var("UNENDLICH_FRAME_PROFILE").map(|v| v != "0").unwrap_or(true);
    let mut prof = unendlich_engine::profiler::Profiler::new();
    let mut last_profile_dump = Instant::now();
    // r292: breeding is a SLOW biological event — no need to scan for mates every
    // fired frame (the O(n^2) pair-scan was ~2/3 of game_update). Run it on a low
    // cadence instead; the spatial grid (sim::spatial_grid) bounds the per-pass cost.
    let mut last_breed_check = Instant::now();
    // r294: per-tick SURFACE-HEIGHT MEMO. surface_y_at scans a 64-deep column doing
    // density() + is_cave() (full procedural noise) at EACH block — and the roamer
    // loop probes it many times per creature (grounding, posture's 4 directions,
    // path steps), all calling cpu.surface_y_at DIRECTLY. With creatures clustered
    // near the player (operator M2: 19 active roamers) this dominated roamer_tick at
    // ~90ms. The SurfaceCache (already written, never wired) memoizes (x,z)→height
    // within a tick so repeated probes of the same/neighbouring column become a
    // HashMap hit instead of 64 noise evals. Cleared each tick (terrain changes
    // between ticks). UNENDLICH_SURFACE_CACHE=1 (default ON; "0" reverts).
    let surface_cache_enabled: bool = std::env::var("UNENDLICH_SURFACE_CACHE")
        .map(|v| v != "0").unwrap_or(true);
    if surface_cache_enabled {
        log::info!("[surface_cache] ENABLED — per-tick memo of surface-height column scans");
    }
    let surface_cache = std::cell::RefCell::new(
        unendlich_engine::sim::surface_cache::SurfaceCache::new(),
    );
    // r300: GRAZE GATE — the herbivore food-ring scan (up to 30 block_at) was the
    // roamer loop's single largest block-read source, run every tick. With the
    // gate, the scan runs only to ACQUIRE a target (on a cell change or every
    // GRAZE_SCAN_INTERVAL ticks) and the creature pursues the remembered target in
    // between. UNENDLICH_GRAZE_GATE=1 (default ON; "0" reverts to per-tick scan).
    let graze_gate_enabled: bool = std::env::var("UNENDLICH_GRAZE_GATE")
        .map(|v| v != "0").unwrap_or(true);
    if graze_gate_enabled {
        log::info!("[graze_gate] ENABLED — food-ring scan gated by motion + cadence, target remembered");
    }
    const GRAZE_SCAN_INTERVAL: u32 = 6;
    const BREED_INTERVAL: std::time::Duration = std::time::Duration::from_millis(1500);
    const PROFILE_DUMP_SECS: f32 = 5.0;

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

    // Spatial/menu music — opus-native, decode-once (audio::SpatialMusic). Built only
    // with `--features audio`; needs an audio device at runtime. Tries $UNENDLICH_MUSIC,
    // then the shipped assets. Logs which track loaded (or why it didn't) so a silent
    // run is diagnosable. Held for the program lifetime so the stream stays alive.
    #[cfg(feature = "audio")]
    let fg_music_player = {
        let candidates: Vec<String> = std::env::var("UNENDLICH_MUSIC")
            .ok()
            .into_iter()
            .chain([
                "assets/music/title_koyopper.opus".to_string(),
                "assets/music/koYopper.opus".to_string(),
                "assets/title_koyopper.opus".to_string(),
                // cwd-independent: anchored at the workspace root via the crate
                // manifest, so launching from crates/ (or anywhere) still finds it.
                concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/music/title_koyopper.opus").to_string(),
                concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/music/koYopper.opus").to_string(),
            ])
            .collect();
        let mut started = None;
        // r179: the three-track split plays as SPATIAL EMITTERS when the
        // stems exist (music centre, bass left, drums right — the engine's
        // own menu_stem_layout); the merged single is the fallback.
        for stems_dir in ["assets/music/stems", "../../assets/music/stems", "../assets/music/stems"] {
            let d = std::path::Path::new(stems_dir);
            if d.is_dir() {
                let stem_paths: Vec<(&'static str, std::path::PathBuf)> = vec![
                    ("music", d.join("koYopper (music).opus")),
                    ("bass", d.join("koYopper (bass).opus")),
                    ("drums", d.join("koYopper (drums).opus")),
                ];
                if stem_paths.iter().all(|(_, p)| p.exists()) {
                    if let Some(m) = unendlich_engine::audio::SpatialMusic::start_spatial(&stem_paths) {
                        log::info!("audio: spatial title stems from {} (music/bass/drums emitters)", stems_dir);
                        started = Some(m);
                        break;
                    }
                }
            }
        }
        for p in &candidates {
            if started.is_some() { break; }
            if std::path::Path::new(p).exists() {
                match unendlich_engine::audio::SpatialMusic::start_single(std::path::Path::new(p)) {
                    Some(m) => {
                        log::info!("audio: playing music from {p}");
                        started = Some(m);
                        break;
                    }
                    None => log::warn!("audio: found {p} but could not start playback (no device / decode failed)"),
                }
            }
        }
        if started.is_none() {
            log::warn!("audio: no music started (tried {candidates:?}) — drop an .opus there or set UNENDLICH_MUSIC");
        }
        started
    };

    // One-shot: once spawn-area chunks have streamed in, drop the UNENDLICH block
    // monument + a flora patch at the player's start position, regardless of whether
    // we came through the menu Launch path. Logged so a run can be verified from the
    // terminal even without spotting it in-world.
    let mut landmarks_placed = false;
    let mut fg_last_preview_seed: Option<u64> = Option::None; // r181
    // r346 SEED FIX: the single live world seed. Starts at the menu-backdrop default;
    // a world launch sets it to the picked seed. ALL in-game seed consumers (creature
    // spawns, colony, field overlays) read this so the world is one coherent seed,
    // not terrain-from-picked-seed + creatures/fields-from-default (two seeds, one world).
    let mut fg_world_seed: u64 = WORLD_SEED_DEFAULT as u64;
    // r670: the last bending technique the player cast (named via
    // technique_catalogue::identify) + when — shown briefly in the HUD.
    let mut fg_last_technique: Option<(&'static str, std::time::Instant)> = None;
    // r674: an environment-gated hybrid discipline available at the last cast
    // (sim::hybrid_disciplines) + when — announced briefly in the HUD.
    let mut fg_last_hybrid: Option<(&'static str, std::time::Instant)> = None;
    // r461: named-location discovery (SPEC-010). Per-world registry of discovered biome-scale regions.
    let mut session = unendlich_engine::world::world_session::WorldSession::new(fg_world_seed);
    // r514: load any persisted crop/flora growth state for this world (sparse — only planted
    // cells). It round-trips to flora.json under the world root; absent → empty store.
    session.flora = unendlich_engine::world::flora_store::FloraStore::load(&flora_root);
    if !session.flora.is_empty() {
        log::info!("[farm] loaded {} persisted crop(s) from disk", session.flora.len());
    }
    // r518: OPEN-SERVER ACCESS CONTROL. When hosting, `whitelist.open` lets anyone join;
    // closed admits only listed names (matched against the joiner's CH_AVATAR character name).
    // Persisted per-world as whitelist.json next to the flora store. Default open.
    let mut whitelist = unendlich_engine::sim::whitelist::Whitelist::load(&flora_root);
    log::info!(
        "[lan-mp] server access: {} ({} on allow-list)",
        if whitelist.open { "OPEN (anyone may join)" } else { "CLOSED (whitelist only)" },
        whitelist.len()
    );
    let mut fg_create_spree: u8 = 0; // r186: Create World presses-through
    // r661: (removed fg_host_pending — Host World now opens the wizard normally)
    let mut fg_pending_load: Option<(String, u64, bool)> = None; // r197 load-world
    // r661: Bending Tutorial — when set, the tutorial world was just launched and
    // the bounded arena should be stamped at spawn once its chunks are loaded.
    const TUTORIAL_SEED: u64 = 0xBE57_1233;
    let mut fg_tutorial_arena_pending: bool = false;
    let mut fg_tutorial_launch_frame: u64 = 0;
    // r662: the stack the player is holding on the cursor in the survival
    // inventory (picked up from a slot; dropped into another on the next click).
    let mut fg_inv_cursor: Option<unendlich_engine::items::ItemStack> = None;
    let mut fg_rollback_world: Option<String> = None; // r203 world whose rollbacks are shown
    // r394: LAN multiplayer discovery (bin I/O shell, gated by UNENDLICH_LAN_MP=1).
    // The beacon is a real UDP multicast socket (sim::lan_discovery); while in-game
    // we announce the live world on a cadence, and each frame we poll discovered
    // hosts into a ServerBrowser to populate the Multiplayer screen list. Default
    // off, so single-player and the normal build are unaffected.
    // r401: LAN multiplayer is now ON BY DEFAULT (the feature is reachable without an
    // env var). Set UNENDLICH_LAN_MP=0 to disable. Socket setup fails gracefully to an
    // inert state, so single-player is unaffected if binding can't happen.
    let lan_mp_enabled = std::env::var("UNENDLICH_LAN_MP").map(|v| v != "0").unwrap_or(true);
    let lan_beacon: Option<unendlich_engine::sim::lan_discovery::LanBeacon> = if lan_mp_enabled {
        match unendlich_engine::sim::lan_discovery::LanBeacon::bind() {
            Ok(b) => {
                log::info!("[lan-mp] discovery beacon bound (UNENDLICH_LAN_MP=1)");
                Some(b)
            }
            Err(e) => {
                log::warn!("[lan-mp] beacon bind failed: {e} — LAN discovery disabled");
                None
            }
        }
    } else {
        None
    };
    let lan_start = std::time::Instant::now();
    let mut lan_browser = unendlich_engine::sim::session_mode::ServerBrowser::new();
    let mut lan_announcer =
        unendlich_engine::sim::net::presence_loop::HostAnnouncer::with_period(2000);
    let mut lan_last_ms: u64 = 0;
    let mut lan_seen_keys: std::collections::HashSet<(String, String)> =
        std::collections::HashSet::new();
    // r397: addresses of discovered hosts, parallel to renderer.menu_mp_list, so a
    // clicked index maps to a candidate to send a JOIN to.
    let mut lan_host_addrs: Vec<std::net::SocketAddrV4> = Vec::new();
    // r399: peer-to-peer transform replication state. Each side is authoritative for
    // its own player and sends it to the connected peer; the receiver newest-wins per
    // entity id (`SnapshotGate`). The local entity id is the session port, distinct per
    // process on a LAN, so host + joiner ids never collide.
    let mut lan_peer_addr: Option<std::net::SocketAddrV4> = None;
    let mut lan_stamper = unendlich_engine::sim::net::entity_snapshot::AuthorityStamper::new();
    let mut lan_gate = unendlich_engine::sim::net::entity_snapshot::SnapshotGate::new();
    let mut lan_repl_last_ms: u64 = 0;
    let mut lan_repl_tick: u64 = 0;
    // r400: visible remote-player avatars. One Creature per remote entity id, parked
    // at the latest replicated position and baked into the creature render path each
    // frame. Lives in this map (not the streaming entity store) so it never despawns.
    let mut lan_remote_creatures: std::collections::HashMap<u64, unendlich_engine::creature::Creature> =
        std::collections::HashMap::new();
    // r494: appearance (chosen character) per remote id, learned via CH_AVATAR, so
    // remotes render as each other's actual character, not a default dragon.
    let mut lan_remote_appearance: std::collections::HashMap<u64, unendlich_engine::player_character::CharacterProfile> =
        std::collections::HashMap::new();
    let mut lan_avatar_last_sent_ms: u64 = 0;
    // r404: last time (ms) each remote entity was heard from, for liveness timeout.
    let mut lan_remote_last_seen: std::collections::HashMap<u64, u64> =
        std::collections::HashMap::new();
    // r403: local block edits (place/break) queued here, drained to peers each frame on
    // CH_WORLDCMD so building/mining propagates. Applied on receipt via set_block.
    // r396: this host's connectable LAN endpoint. We bind an ephemeral UDP socket
    // (the future game-session socket) so the advertised port is real and reserved,
    // and resolve our LAN IPv4 once. Joiners read this candidate to know where to
    // connect. Only set up when LAN-MP is enabled.
    let lan_session_sock: Option<std::net::UdpSocket> = if lan_beacon.is_some() {
        // r405: prefer a fixed port (9878) so a peer can reach us via direct-connect
        // (UNENDLICH_LAN_HOST) without multicast discovery; fall back to an ephemeral
        // port if 9878 is taken (e.g. a second instance on the same machine).
        let bound = std::net::UdpSocket::bind("0.0.0.0:9878")
            .or_else(|_| std::net::UdpSocket::bind("0.0.0.0:0"));
        match bound {
            Ok(s) => {
                let _ = s.set_nonblocking(true); // per-frame recv must not block the loop
                let port = s.local_addr().map(|a| a.port()).unwrap_or(0);
                log::info!("[lan-mp] session socket bound on udp/{port}");
                Some(s)
            }
            Err(e) => {
                log::warn!("[lan-mp] session socket bind failed: {e} — adverts won't carry an address");
                None
            }
        }
    } else {
        None
    };
    // r405: optional direct-connect target — set UNENDLICH_LAN_HOST=<ip>[:port] to join
    // a host by address (port defaults to 9878), bypassing multicast discovery. The
    // joiner retries a JOIN to this address until accepted.
    let lan_direct_host: Option<std::net::SocketAddrV4> = std::env::var("UNENDLICH_LAN_HOST")
        .ok()
        .and_then(|s| {
            let s = s.trim();
            if let Ok(a) = s.parse::<std::net::SocketAddrV4>() {
                Some(a)
            } else if let Ok(ip) = s.parse::<std::net::Ipv4Addr>() {
                Some(std::net::SocketAddrV4::new(ip, 9878))
            } else {
                log::warn!("[lan-mp] UNENDLICH_LAN_HOST='{s}' is not a valid ip or ip:port");
                None
            }
        });
    if let Some(h) = lan_direct_host {
        log::info!("[lan-mp] direct-connect target {h} (will JOIN until accepted)");
    }
    let mut lan_direct_last_ms: u64 = 0;
    let lan_session_port: u16 = lan_session_sock
        .as_ref()
        .and_then(|s| s.local_addr().ok())
        .map(|a| a.port())
        .unwrap_or(0);
    let lan_local_ip: Option<[u8; 4]> = if lan_beacon.is_some() {
        let ip = unendlich_engine::sim::lan_discovery::local_lan_ipv4();
        match ip {
            Some(o) => log::info!("[lan-mp] local LAN ip {}.{}.{}.{}", o[0], o[1], o[2], o[3]),
            None => log::warn!("[lan-mp] no routable LAN ipv4 found — adverts won't carry an address"),
        }
        ip
    } else {
        None
    };
    // r200: eviction is now BUDGETED to kill movement stutter. A 1-chunk
    // move at high render distance pushes an O(R^2) shell out of range;
    // evicting it all in one frame spiked frame time exactly when moving.
    // Out-of-range chunks are invisible, so draining them over a few
    // frames is imperceptible. This queue holds the pending evictions;
    // up to EVICT_BUDGET_PER_FRAME drain each frame.
    const EVICT_BUDGET_PER_FRAME: usize = 64;

    /// r191: two-part competing ramp — speed = BASE + ACCEL*|err|, no
    /// overshoot. Far targets fly, near targets settle. Used by the menu
    /// camera pan; the SAME curve is the player rubber-band for rollback
    /// reconciliation (tick_authority step 2): on restore, set a
    /// reconcile target and approach it with this instead of teleporting.
    fn ramp_approach(cur: &mut f32, target: f32, dt: f32, base: f32, accel: f32) {
        let err = target - *cur;
        let step = (base + accel * err.abs()) * dt;
        if err.abs() <= step { *cur = target; } else { *cur += step.copysign(err); }
    }
    let mut fg_menu_yaw: f32 = 0.0; // r186: screen pan + parallax state
    let mut fg_menu_pitch_par: f32 = 0.0;
    let mut fg_rb_travel: f32 = 0.0; // r186 rollback-point accumulators
    let mut fg_rb_force_point = false; // r206: dev "create history point now"
    // r206: enable the F4 debug overlay at boot when UNENDLICH_DEV=1.
    if unendlich_engine::dev_mode::is_dev() {
        debug_scene.enabled = true;
        log::info!("[dev] UNENDLICH_DEV=1 — debug overlay on, history cadence shortened");
    }


    let mut fg_rb_last_pos: [f32; 2] = [0.0, 0.0];
    let mut fg_rb_craft_flag: bool = false;
    let mut fg_prev_settings_open: bool = false; // r187
    let mut fg_music_fadein: f32 = 0.0; // r187: starts silent
    // r187: sticky settings path + tiny serde helpers.
    let settings_path: std::path::PathBuf = std::env::var("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|_| std::env::var("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))
        .map(|p| p.join("unendlich/settings.json"))
        .unwrap_or_else(|_| std::path::PathBuf::from("unendlich-settings.json"));
    if let Ok(txt) = std::fs::read_to_string(&settings_path) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&txt) {
            let g = |k: &str| v.get(k).cloned().unwrap_or(serde_json::Value::Null);
            if let Some(b) = g("fog").as_bool() { renderer.ui_fog_on = b; }
            if let Some(b) = g("shadows").as_bool() { renderer.ui_shadows_on = b; }
            if let Some(f) = g("music_vol").as_f64() { renderer.ui_music_vol = f as f32; }
            if let Some(f) = g("day_length").as_f64() { renderer.ui_day_length = f as f32; }
            if let Some(n) = g("plant_stride").as_u64() { renderer.ui_plant_stride = n; }
            if let Some(n) = g("leaf_stride").as_u64() { renderer.ui_leaf_stride = n; }
            if let Some(n) = g("apply_budget").as_u64() { renderer.ui_apply_budget = n as u32; }
            if let Some(n) = g("render_distance").as_i64() { renderer.ui_render_distance = (n as i32).clamp(2, 32); } // r197
            if let Some(f) = g("gui_scale").as_f64() { renderer.ui_scale = (f as f32).clamp(1.0, 3.0); } // r200
            if let Some(n) = g("fps_target").as_u64() { renderer.ui_fps_target = n as u32; }
            if let Some(b) = g("autotune").as_bool() { renderer.ui_autotune = b; }
            if let Some(n) = g("gpu_mesh_yield").as_u64() { renderer.ui_gpu_mesh_yield = n.min(200); }
            if let Some(b) = g("adaptive_rd").as_bool() { renderer.ui_adaptive_rd = b; }
            if let Some(b) = g("octree_tile").as_bool() { renderer.ui_octree_tile = b; }
            if let Some(b) = g("detached_terrain").as_bool() { renderer.ui_detached_terrain = b; }
            log::info!("[settings] loaded {}", settings_path.display());
        }
    }
    fn save_settings(path: &std::path::Path, r: &unendlich_engine::render::world_renderer::WorldRenderer) {
        let v = serde_json::json!({
            "fog": r.ui_fog_on, "shadows": r.ui_shadows_on,
            "music_vol": r.ui_music_vol, "day_length": r.ui_day_length,
            "plant_stride": r.ui_plant_stride, "leaf_stride": r.ui_leaf_stride,
            "apply_budget": r.ui_apply_budget, "fps_target": r.ui_fps_target,
            "render_distance": r.ui_render_distance,
            "gui_scale": r.ui_scale,
            "autotune": r.ui_autotune,
            "gpu_mesh_yield": r.ui_gpu_mesh_yield,
            "adaptive_rd": r.ui_adaptive_rd,
            "octree_tile": r.ui_octree_tile,
            "detached_terrain": r.ui_detached_terrain,
        });
        if let Some(parent) = path.parent() { let _ = std::fs::create_dir_all(parent); }
        match std::fs::write(path, serde_json::to_string_pretty(&v).unwrap_or_default()) {
            Ok(_) => log::info!("[settings] saved {}", path.display()),
            Err(e) => log::warn!("[settings] save failed: {e}"),
        }
    }

    while !window.should_close() {
        // r237: total frame wall-time (whole loop iteration) for the profiler,
        // so the FPS distribution + the stutter tail are visible alongside the
        // per-phase zones. Cheap; only the dump is env-gated.
        let t_frame_start = Instant::now();
        // Crossfade spatial music by listener (camera) position. No-op for a single
        // mixed track; drives the mix once per-stem audio is loaded.
        #[cfg(feature = "audio")]
        if let Some(ref m) = fg_music_player {
            let p = local_player.view.position;
            // r181: on the title the orbit is 46 units out, far past the
            // stems' falloff (the cutout). Pull the listener toward the
            // emitter origin: gentle stereo drift, never silence.
            if renderer.menu_open || renderer.wizard_open {
                m.update([p.x * 0.08, 0.0, p.z * 0.08]);
            } else {
                m.update([p.x, p.y, p.z]);
            }
        }

        // r322: the mesh-cache trial survived the GPU-init risk window — clear
        // the marker so a later, unrelated crash isn't blamed on it.
        if !safe_tune_disarmed && frame_count > 600 {
            safe_tune.disarm();
            safe_tune_disarmed = true;
            if effective_mesh_cache > 0 {
                log::info!("[safe-tune] mesh_cache survived {frame_count} frames; marker cleared");
            }
        }

        // r661: Bending Tutorial — stamp the bounded arena at spawn once the
        // tutorial world's chunks are loaded (~90 frames after launch). One-shot.
        if fg_tutorial_arena_pending && frame_count > fg_tutorial_launch_frame + 90 {
            fg_tutorial_arena_pending = false;
            let p = local_player.view.position;
            let (px, pz) = (p.x.floor() as i32, p.z.floor() as i32);
            let gen = chunk_manager.generator().cpu();
            let sy = gen
                .surface_y_at(px, pz, probe_y_min, probe_y_max)
                .unwrap_or(p.y as i32);
            let mut n = 0;
            for (b, block) in unendlich_engine::sim::tutorial_arena::arena_blocks([px, sy, pz]) {
                if chunk_manager.set_block(b[0], b[1], b[2], block) {
                    n += 1;
                }
            }
            log::info!(
                "[tutorial] stamped bending arena: {n} blocks at spawn ({px},{sy},{pz})"
            );
            // r666: place two "trainer" creatures inside the arena (Lauscher +
            // Spaeher — the watchful observer kinds) for the player to practise
            // bending near. Built through the shared build_roamer helper.
            let trainer_kinds = [
                unendlich_engine::item::CreatureKind::Lauscher,
                unendlich_engine::item::CreatureKind::Spaeher,
            ];
            for (ti, tk) in trainer_kinds.iter().enumerate() {
                let ang = std::f32::consts::TAU * (ti as f32) / (trainer_kinds.len() as f32) + 0.5;
                let tx = px as f32 + 0.5 + 4.0 * ang.cos();
                let tz = pz as f32 + 0.5 + 4.0 * ang.sin();
                let tseed = fg_world_seed ^ 0x7A1E ^ (ti as u64);
                roamers.push(build_roamer(*tk, [tx, (sy + 1) as f32, tz], tseed));
            }
            log::info!("[tutorial] placed {} arena trainers", trainer_kinds.len());
        }

        // One-shot landmark placement (see flag declaration). Wait ~1.5s so the
        // spawn-area chunks are loaded and set_block lands on real chunks.
        if !landmarks_placed && frame_count > 90 {
            landmarks_placed = true;
            let p = local_player.view.position;
            let (px, pz) = (p.x.floor() as i32, p.z.floor() as i32);
            let gen = chunk_manager.generator().cpu();
            let sy = gen
                .surface_y_at(px, pz, probe_y_min, probe_y_max)
                .unwrap_or(p.y as i32);
            // UNENDLICH monument, floating + in front of spawn
            let mut mon = 0;
            for b in unendlich_engine::world::title_structure::unendlich_blocks([px - 26, sy + 14, pz - 22]) {
                if chunk_manager.set_block(b[0], b[1], b[2], unendlich_engine::world::title_structure::TITLE_BLOCK) {
                    mon += 1;
                }
            }
            // flora patch around spawn
            let gen2: &unendlich_engine::world::cpu_generator::WorldGenerator =
                chunk_manager.generator().cpu();
            let flora = unendlich_engine::world::flora_blocks::region_flora_blocks(gen2, px - 20, pz - 20, 40, 40);
            let flora_total = flora.len();
            let mut fl = 0;
            for (b, block) in flora {
                if chunk_manager.set_block(b[0], b[1], b[2], block) {
                    fl += 1;
                }
            }
            log::info!(
                "landmarks: placed monument {mon} blocks + flora {fl}/{flora_total} blocks near spawn ({px},{pz},{sy})"
            );
            // r129: STONE RING + BUILDERS — wired through the real structure
            // stack: SiteFeatures sampled from worldgen, structure_place::
            // rank_sites picks spots, structure_template::make_ruin stamps
            // walls. One stone ring + two ruins per world, near spawn.
            {
                use unendlich_engine::world::structure_place::{rank_sites, SiteFeatures, StructurePrefs};
                use unendlich_engine::world::structure_template::make_ruin;
                use unendlich_engine::world::blocks::BlockId;
                // 16x16 cell grid of 8-block cells around spawn -> features.
                let (gw, gh, cell) = (16u32, 16u32, 8i32);
                let mut feats = Vec::with_capacity((gw * gh) as usize);
                for gy in 0..gh as i32 {
                    for gx in 0..gw as i32 {
                        let wx = px + (gx - gw as i32 / 2) * cell;
                        let wz = pz + (gy - gh as i32 / 2) * cell;
                        let s0 = gen.surface_y_at(wx, wz, probe_y_min, probe_y_max).unwrap_or(sy);
                        let s1 = gen.surface_y_at(wx + cell, wz, probe_y_min, probe_y_max).unwrap_or(s0);
                        let s2 = gen.surface_y_at(wx, wz + cell, probe_y_min, probe_y_max).unwrap_or(s0);
                        let slope = ((s1 - s0).abs() + (s2 - s0).abs()) as f32 / cell as f32;
                        feats.push(SiteFeatures {
                            flatness: (1.0 - slope).clamp(0.0, 1.0),
                            elevation: ((s0 - 40) as f32 / 60.0).clamp(0.0, 1.0),
                            water_dist: if s0 <= 63 { 0.0 } else { 0.6 },
                        });
                    }
                }
                let sites = rank_sites(&feats, gw, gh, &StructurePrefs::default(), 3);
                let mut placed_structs = 0usize;
                for (si, cand) in sites.iter().enumerate() {
                    let wx = px + (cand.x as i32 - gw as i32 / 2) * cell;
                    let wz = pz + (cand.y as i32 - gh as i32 / 2) * cell;
                    let base = gen.surface_y_at(wx, wz, probe_y_min, probe_y_max).unwrap_or(sy) + 1;
                    if si == 0 {
                        // STONE RING: 8 pillars, radius 7, heights 2-4.
                        for k in 0..8 {
                            let a = (k as f32) * std::f32::consts::TAU / 8.0;
                            let (ox, oz) = ((a.cos() * 7.0).round() as i32, (a.sin() * 7.0).round() as i32);
                            let h = 2 + (k % 3);
                            let py0 = gen.surface_y_at(wx + ox, wz + oz, probe_y_min, probe_y_max).unwrap_or(base - 1) + 1;
                            for dy in 0..h {
                                if chunk_manager.set_block(wx + ox, py0 + dy, wz + oz, BlockId::Stone) {
                                    placed_structs += 1;
                                }
                            }
                        }
                        log::info!("[builders] stone ring at ({wx},{wz}) score {:.2}", cand.score);
                    } else {
                        // BUILDERS: a 7x7 ruin (cobble walls, plank floor).
                        let tpl = make_ruin(7, 7, BlockId::Cobblestone as u8, BlockId::OakPlanks as u8, 0xB11D ^ (si as u64));
                        for cy in 0..tpl.h as i32 {
                            for cx in 0..tpl.w as i32 {
                                let code = tpl.cells[(cy as u32 * tpl.w + cx as u32) as usize];
                                if code == 0 { continue; }
                                let (bx, bz) = (wx + cx - 3, wz + cz_off(cy));
                                let ground = gen.surface_y_at(bx, bz, probe_y_min, probe_y_max).unwrap_or(base - 1) + 1;
                                if code == BlockId::Cobblestone as u8 {
                                    for dy in 0..3 {
                                        if chunk_manager.set_block(bx, ground + dy, bz, BlockId::Cobblestone) { placed_structs += 1; }
                                    }
                                } else if chunk_manager.set_block(bx, ground - 1, bz, BlockId::OakPlanks) {
                                    placed_structs += 1;
                                }
                            }
                        }
                        log::info!("[builders] ruin at ({wx},{wz}) score {:.2}", cand.score);
                    }
                }
                log::info!("[builders] {} structure blocks placed (1 ring + {} ruins)", placed_structs, sites.len().saturating_sub(1));
            }
            // r118: test item drops near spawn so proximity pickup is visible.
            {
                use unendlich_engine::items::{ItemId, ItemStack};
                use unendlich_engine::world::blocks::BlockId;
                for (dx, dz, block) in [
                    (3.0_f32, 0.0_f32, BlockId::Stone),
                    (-3.0, 0.0, BlockId::Dirt),
                    (0.0, 3.0, BlockId::Grass),
                ] {
                    let stack = ItemStack { item: ItemId(block as u32), count: 1 };
                    fg_app.drops_mut().spawn(
                        stack,
                        [px as f32 + dx, sy as f32 + 1.0, pz as f32 + dz],
                        [0.0, 0.0, 0.0],
                    );
                }
                log::info!("[items] spawned 3 test drops near spawn — walk over them to collect");
            }
        }

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
        let mut ui_text_frame = String::new(); // r181: chars typed this frame

        for (_, event) in glfw::flush_messages(&events) {
            events_received += 1;
            match &event {
                WindowEvent::Key(_, _, _, _) => key_events += 1,
                WindowEvent::MouseButton(_, _, _) => mouse_events += 1,
                WindowEvent::CursorPos(_, _) => cursor_events += 1,
                _ => {}
            }
            if let Some(ev) = unendlich_engine::input_codes::input_event_from_glfw(&event) {
                input.handle_event(&ev);
            }
            match event {
                WindowEvent::Char(ch) => {
                    // r181: text input for egui fields (name/seed boxes).
                    ui_text_frame.push(ch);
                }
                WindowEvent::Scroll(dx, dy) => {
                    scroll_accum += dy as f32; // r122: consumed below per frame
                    scroll_accum_x += dx as f32; // r318: horizontal (trackpad)
                }
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
        esc_pressed |= input.is_key_just_pressed(Keycode::Escape);

        // ── r525: UNIFIED UI STATE (light in-world overlays) ──────────────
        // `menu_state` owns the overlays the player toggles in-world: Inventory
        // (p, or the rebindable OpenInventory alias) and Debug (F4). NoMenu is
        // the default — playing with the cursor captured. The heavyweight FSM
        // screens (Pause/Settings/Craft on Esc/C, and the Title screen) keep
        // their existing plumbing but are reconciled here: opening one closes
        // these light overlays, and these light overlays only toggle when no
        // heavyweight screen is up. One overlay at a time, one key each.
        let heavy_open = renderer.menu_open
            || renderer.pause_menu_open
            || renderer.settings_menu_open
            || renderer.character_menu_open // r577: the customizer is a heavy surface too
            || renderer.craft_menu_open;
        if !heavy_open {
            let typing = renderer.ui_wants_keyboard;
            let prev = menu_state;
            // p → inventory (Dvorak-aware p_pressed). Only OPENING is blocked
            // while typing; closing always works.
            if p_pressed {
                p_pressed = false;
                if menu_state == MenuState::Inventory || !typing {
                    menu_state = menu_state.toggle(MenuState::Inventory);
                }
            }
            // Rebindable OpenInventory (default R/I) is an alias for inventory.
            if bindings.is_action_just_pressed(GameAction::OpenInventory, &input) && !typing {
                menu_state = menu_state.toggle(MenuState::Inventory);
            }
            // F4 (or F3) → debug panels, on their own key (separate from inventory).
            if input.is_key_just_pressed(Keycode::F4) || input.is_key_just_pressed(Keycode::F3) {
                menu_state = menu_state.toggle(MenuState::Debug);
            }
            // Cursor follows the state: free it when an overlay opens, recapture
            // it the instant we return to NoMenu — no extra click (the sticky fix).
            if menu_state != prev {
                if menu_state.captures_cursor() {
                    window.set_cursor_mode(CursorMode::Disabled);
                    mouse_captured = true;
                    input.cursor_delta = (0.0, 0.0);
                } else {
                    window.set_cursor_mode(CursorMode::Normal);
                    mouse_captured = false;
                }
                log::info!("[ui] overlay = {:?}", menu_state);
            }
        } else {
            // A heavyweight screen is up — the light overlay yields to it.
            menu_state = MenuState::NoMenu;
            p_pressed = false; // don't let a queued 'p' leak into the next frame
        }
        // PUSH: derive the legacy flags the rest of the frame still reads.
        let show_inventory = menu_state == MenuState::Inventory || renderer.pause_menu_open;
        let ui_panel_open = menu_state.is_open();
        renderer.settings_open = menu_state == MenuState::Debug; // shader/debug panel
        renderer.perf_window_open = menu_state == MenuState::Debug; // frame-time dashboard
        // r554: pull a hotbar/picker click (from the centered inventory's hotbar
        // switcher last frame) back into the gameplay slot so clicking a slot
        // selects it. Only valid hotbar slots; the scroll/key handlers below push
        // the reverse direction, so the two stay in sync without fighting.
        if renderer.creative_selected != selected_palette_idx
            && renderer.creative_selected < PLACE_PALETTE.len()
        {
            selected_palette_idx = renderer.creative_selected;
            selected_block = PLACE_PALETTE[selected_palette_idx];
            fg_app.inventory_mut().set_selected(selected_palette_idx);
            if survival_mode {
                hand_item = fg_app.inventory().slot(selected_palette_idx).map(|s| s.item);
                if let Some(stack) = fg_app.inventory().slot(selected_palette_idx) {
                    if let Some(b) = unendlich_engine::world::blocks::BlockId::ALL
                        .iter()
                        .copied()
                        .find(|b| *b as u32 == stack.item.0)
                    {
                        selected_block = b;
                    }
                }
            }
        }
        // r657: in creative, place the EXACT item the inventory UI shows. The
        // renderer publishes the player's pick (palette click or hotbar slot) in
        // `creative_place_item`; a block becomes the placement block. This replaces
        // the old decoupled 9-block PLACE_PALETTE (clicks used to select the wrong
        // block, or be ignored past slot 9 — so most catalog items were untestable).
        if !survival_mode {
            if let Some(unendlich_engine::item::ItemKind::Block(b)) = renderer.creative_place_item {
                selected_block = b;
            }
        }
        // r449: K sorts the backpack (hotbar preserved) via the inventory_sort core.
        if mouse_captured && input.is_key_just_pressed(Keycode::K) {
            fg_app.inventory_mut().sort_display();
            log::info!("[inventory] backpack sorted");
        }
        // r662: apply an inventory slot click. Empty cursor + filled slot → pick
        // the stack up onto the cursor; held cursor → drop it into the slot,
        // swapping whatever was there back onto the cursor. (The panel only sets
        // inv_click while the survival inventory is open, so no extra gating.)
        if renderer.inv_click >= 0 {
            let i = renderer.inv_click as usize;
            renderer.inv_click = -1;
            let inv = fg_app.inventory_mut();
            if i < inv.len() {
                match fg_inv_cursor.take() {
                    None => fg_inv_cursor = inv.take(i),
                    Some(held) => {
                        let prev = inv.slot(i);
                        inv.set(i, Some(held));
                        fg_inv_cursor = prev;
                    }
                }
            }
        }
        // r454: CREATIVE TABS — creative mode only. G cycles the category tab, V cycles the block
        // within the current tab; the chosen block becomes the placement block (selected_block).
        if mouse_captured && !survival_mode {
            use unendlich_engine::sim::creative_tabs as ct;
            let cycle_tab = input.is_key_just_pressed(Keycode::G);
            let cycle_item = input.is_key_just_pressed(Keycode::V);
            if cycle_tab || cycle_item {
                if cycle_tab {
                    creative_tab_idx = (creative_tab_idx + 1) % ct::ALL_TABS.len();
                    creative_item_idx = 0;
                }
                let tab = ct::ALL_TABS[creative_tab_idx];
                let items = ct::tab_items(tab);
                if !items.is_empty() {
                    if cycle_item {
                        creative_item_idx = (creative_item_idx + 1) % items.len();
                    }
                    let idx = creative_item_idx.min(items.len() - 1);
                    selected_block = items[idx];
                    // r657: mirror the pick into the UI place-item so the per-frame
                    // override (which reads creative_place_item) stays consistent
                    // with the G/V keyboard cycling instead of reverting it.
                    renderer.creative_place_item =
                        Some(unendlich_engine::item::ItemKind::Block(selected_block));
                    log::info!(
                        "[creative] {} [{}/{}]: {:?}",
                        ct::label(tab),
                        idx + 1,
                        items.len(),
                        selected_block
                    );
                }
            }
        }
        let game_paused = renderer.settings_open
            || show_inventory
            || renderer.pause_menu_open
            || renderer.settings_menu_open // r169
            || renderer.craft_menu_open // r169
            || timebend.active; // r476: scrubbing freezes the live world

        // r317: any open UI surface owns the scroll wheel (so dialog / controls
        // ScrollAreas scroll) — don't let the hotbar eat it even if the cursor
        // capture flag lags the menu state.
        let ui_scroll_target = renderer.settings_open
            || renderer.settings_menu_open
            || renderer.pause_menu_open
            || renderer.craft_menu_open
            || renderer.menu_open
            || renderer.wizard_open
            || show_inventory;
        // r319 diag: log the routing decision for any pending scroll so the
        // controls-list capture bug can be pinned from a real run.
        if scroll_accum.abs() > 1e-4 || scroll_accum_x.abs() > 1e-4 {
            log::info!(
                "[scroll-dbg] pending dx={:.3} dy={:.3} cap={} target={} -> {}",
                scroll_accum_x, scroll_accum, mouse_captured, ui_scroll_target,
                if mouse_captured && !ui_scroll_target { "hotbar" } else { "egui" }
            );
        }
        // r318: in game mode a horizontal trackpad swipe also cycles the hotbar.
        if mouse_captured && !ui_scroll_target {
            scroll_accum += scroll_accum_x;
            scroll_accum_x = 0.0;
        }
        // r476: while scrubbing, the wheel walks the recorded vantages instead
        // of the hotbar. One snapshot per notch; consumes the scroll so the
        // palette doesn't also change.
        if timebend.active {
            while scroll_accum.abs() >= 1.0 {
                let step: i32 = if scroll_accum > 0.0 { 1 } else { -1 }; // scroll up = forward in time
                scroll_accum -= scroll_accum.signum();
                timebend.scrub(step);
            }
            scroll_accum = 0.0;
        }
        // r122: scroll wheel cycles the hotbar / place palette (swipe-scroll on
        // touchpads lands here too). One step per wheel notch, wraps at the ends.
        while scroll_accum.abs() >= 1.0 && mouse_captured && !ui_scroll_target && !timebend.active {
            let step: i32 = if scroll_accum > 0.0 { -1 } else { 1 }; // scroll up = previous
            scroll_accum -= scroll_accum.signum();
            let n = PLACE_PALETTE.len() as i32;
            selected_palette_idx =
                (((selected_palette_idx as i32 + step) % n + n) % n) as usize;
            selected_block = PLACE_PALETTE[selected_palette_idx];
            fg_app.inventory_mut().set_selected(selected_palette_idx);
            if survival_mode {
                // r125: in survival the hand holds the INVENTORY slot's item,
                // not the creative palette.
                hand_item = fg_app.inventory().slot(selected_palette_idx).map(|s| s.item);
                if let Some(stack) = fg_app.inventory().slot(selected_palette_idx) {
                    if let Some(b) = unendlich_engine::world::blocks::BlockId::ALL
                        .iter()
                        .copied()
                        .find(|b| *b as u32 == stack.item.0)
                    {
                        selected_block = b;
                    }
                }
            }
            log::info!("[hotbar] scroll -> slot {} ({:?})", selected_palette_idx + 1, selected_block);
            // r554: make the selection VISIBLE — drive the hotbar UI highlight (the
            // renderer's hotbar reads creative_selected) so scrolling actually shows.
            renderer.creative_selected = selected_palette_idx;
        }
        // r487: keep hand_item synced to the held thing EVERY frame so the
        // first-person viewmodel actually renders in CREATIVE too. Previously
        // hand_item was only assigned inside survival-gated branches, so the
        // default (creative) game had an empty hand and drew no viewmodel.
        hand_item = if survival_mode {
            fg_app.inventory().slot(selected_palette_idx).map(|s| s.item)
        } else if (selected_block as u32) != 0 {
            Some(unendlich_engine::items::ItemId(selected_block as u32))
        } else {
            None
        };
        // r509: keep the equipped-weapon durability tracker in sync with what's held —
        // (re)initialise to full when a different forged weapon becomes the hand item, clear
        // when the hand isn't a weapon. Holding the same weapon preserves its current wear.
        match hand_item {
            Some(h)
                if unendlich_engine::sim::viewmodel::weapon_class_for_id(h.0).is_some() =>
            {
                if equipped_weapon_dura.map_or(true, |(id, _, _)| id != h) {
                    equipped_weapon_dura = Some((h, WEAPON_DURA_MAX, WEAPON_DURA_MAX));
                }
            }
            _ => equipped_weapon_dura = None,
        }
        // r488: when the character menu commits a pick, adopt it as the active
        // character (already persisted as the global default by the renderer).
        if let Some(chosen) = renderer.character_chosen.take() {
            active_character = chosen;
            self_avatar_dirty = true; // rebuild the third-person avatar from the new pick
            log::info!(
                "[character] now playing as '{}' (plan={}, seed={:#x}) — saved as global default",
                active_character.name, active_character.arche, active_character.seed
            );
        }
        // r490/r515: third-person toggle (rebindable — default F5).
        if bindings.is_action_just_pressed(
            unendlich_engine::keybindings::GameAction::ToggleThirdPerson, &input,
        ) && !renderer.ui_wants_keyboard {
            camera_mode = if camera_mode == 1 { 0 } else { 1 };
            self_avatar_prev = None; // avoid a velocity spike on re-entry
            log::info!("[view] camera = {}", match camera_mode {
                1 => "third-person", 2 => "cinematic shoulder", _ => "first-person",
            });
        }
        // r495: season for this frame. O advances it ~1/8 year (live testing); the
        // offset feeds both the climate day-of-year (temp/snow) and plant season01.
        if input.is_key_just_pressed(Keycode::O) && !renderer.ui_wants_keyboard {
            season_offset_days += season_days_per_year / 8.0;
        }
        let season_day = world_clock_elapsed / 1440.0 + season_offset_days;
        let current_season01 =
            season_day.rem_euclid(season_days_per_year) / season_days_per_year;
        {
            let s = unendlich_engine::season::Season::from_phase(current_season01);
            if last_logged_season != Some(s) {
                last_logged_season = Some(s);
                log::info!(
                    "[season] now {:?} (phase {:.2}, year {} days)",
                    s, current_season01, season_days_per_year as u32
                );
            }
        }
        if !mouse_captured || ui_scroll_target {
            // r205: when the cursor is free (a menu/dialog is open), the
            // scroll wheel belongs to egui, not the hotbar. Forward the
            // accumulated delta to the renderer so ScrollAreas inside
            // dialogs scroll (operator: "scroll not passing to dialogs").
            // Was: discarded here, which is why nothing scrolled.
            // r318: forward BOTH axes to egui — it ignores them unless a ScrollArea
            // is hovered, so this is the "just works" path: egui owns scroll whenever
            // any UI surface is up.
            renderer.ui_scroll_delta += scroll_accum;
            renderer.ui_scroll_delta_x += scroll_accum_x;
            scroll_accum = 0.0;
            scroll_accum_x = 0.0;
        }

        // r116b/r119: SPAWN EGG — hatch a creature ~6 blocks ahead of the player,
        // cycling species each press. Moved G -> H: G is GameAction::ToggleFlight
        // in the qwerty preset, so the egg and player-flight were fighting over
        // one key (operator report: "eggs place blocks" — G was toggling flight
        // and the click placed the block). H = Hatch, unbound in the preset.
        // r164: F6 — direct mode toggle (operator: "still not getting
        // survival mode"). Flips the ruleset live: survival revokes flight
        // (re-attaching the physics body if you were flying), creative
        // restores the permission. The HUD line confirms instantly.
        if mouse_captured && input.is_key_just_pressed(Keycode::F6) {
            survival_mode = !survival_mode;
            local_player.permissions.can_toggle_flight = !survival_mode;
            if survival_mode && body.is_none() {
                // was flying — survival means gravity applies NOW.
                body = Some(unendlich_engine::physics::PhysicsBody::new_player(
                    [
                        local_player.view.position.x,
                        local_player.view.position.y,
                        local_player.view.position.z,
                    ],
                ));
                self_avatar_dirty = true; // r561: size the new collider to the stature
            }
            log::info!(
                "[mode] F6 -> {} mode{}",
                if survival_mode { "SURVIVAL" } else { "CREATIVE" },
                if survival_mode { " (flight revoked)" } else { "" }
            );
            // r165: persist the choice — rewrite this world's catalog entry
            // so the next LOAD keeps the mode (old entries said Creative).
            if let Some(name) = current_world_name.as_ref() {
                use unendlich_engine::sim::world_catalog::GameMode as CatMode;
                let mut entries = load_catalog_entries();
                for e in entries.iter_mut() {
                    if &e.name == name {
                        e.mode = if survival_mode { CatMode::Survival } else { CatMode::Creative };
                    }
                }
                save_catalog(&entries);
                log::info!("[saves] world '{}' mode updated in catalog", name);
            }
        }
        // r476: F8 toggles the timebend snapshot/scrub camera. Enter freezes the
        // world and jumps to the latest recorded vantage; exit restores the live
        // pose + world clock. Both kick a fade so the cut reads cinematically.
        if mouse_captured && input.is_key_just_pressed(Keycode::F8) {
            if timebend.active {
                if let Some((p, y, pi, sk)) = timebend.exit() {
                    local_player.view.position = p;
                    local_player.view.yaw = y;
                    local_player.view.pitch = pi;
                    sky_state.time_seconds = sk;
                }
                renderer.loading_label = String::new();
                log::info!("[timebend] exit scrub — restored live vantage");
            } else if timebend.enter(
                local_player.view.position,
                local_player.view.yaw,
                local_player.view.pitch,
                sky_state.time_seconds,
            ) {
                log::info!("[timebend] enter scrub — {} vantages recorded", timebend.len());
            } else {
                log::info!("[timebend] no history recorded yet — roam a bit first");
            }
        }
        if mouse_captured && input.is_key_just_pressed(Keycode::H) {
            use unendlich_engine::item::CreatureKind;
            let p = local_player.view.position;
            let yaw = local_player.view.yaw;
            let sx = p.x + yaw.cos() * 6.0;
            let sz = p.z + yaw.sin() * 6.0;
            let cpu = chunk_manager.generator().cpu();
            let sy = cpu
                .surface_y_at(sx.floor() as i32, sz.floor() as i32, probe_y_min, probe_y_max)
                .unwrap_or(p.y as i32)
                + 1;
            let kinds = [
                CreatureKind::Fauna,
                CreatureKind::Dinosaur,
                CreatureKind::Lauscher,
                CreatureKind::Spaeher,
                CreatureKind::Dragon,
            ];
            // r659: if the player has a spawn egg picked in the creative inventory,
            // hatch THAT species; otherwise H cycles through the roster as before.
            let kind = match renderer.creative_place_item {
                Some(unendlich_engine::item::ItemKind::SpawnEgg(k)) => k,
                _ => kinds[roamers.len() % kinds.len()],
            };
            let pos = [sx, sy as f32, sz];
            let wseed = (fg_world_seed)
                ^ ((sx as i32 as u32 as u64) << 20)
                ^ (sz as i32 as u32 as u64)
                ^ (roamers.len() as u64);
            roamers.push(build_roamer(kind, pos, wseed));
            log::info!(
                "[spawn-egg] hatched {:?} at ({:.1}, {}, {:.1}) — {} roamers (H cycles species)",
                kind, sx, sy, sz, roamers.len()
            );
        }

        // P handling: toggle panel mode (cursor visible + UI clicks routed).
        // r525: the old 'p' "Panel mode" (ui_panel_open) is gone — 'p' now toggles
        // MenuState::Inventory in the unified block above.

        // Esc handling: in panel mode -> close panel; else first press releases mouse, second quits.
        // r175: UI keys are handled UNCONDITIONALLY while their window is
        // open (the gameplay block is gated on captured+unpaused, which is
        // exactly the state an open window puts us out of — "C opens but
        // can't close"). Esc closes windows before it means "pause".
        // r187: save settings on ANY close path (Done button included).
        if fg_prev_settings_open && !renderer.settings_menu_open {
            save_settings(&settings_path, &renderer);
        }
        fg_prev_settings_open = renderer.settings_menu_open;
        let typing = renderer.ui_wants_keyboard; // r185: fields eat letters
        if !typing && renderer.craft_menu_open && input.is_key_just_pressed(Keycode::C) {
            // r187: parity — an even tap-count nets to "still open".
            if input.presses_this_frame(Keycode::C) % 2 == 1 {
                renderer.craft_menu_open = false;
            }
        }
        if renderer.settings_menu_open && esc_pressed {
            renderer.settings_menu_open = false;
            save_settings(&settings_path, &renderer); // r187 sticky
            esc_pressed = false; // consumed
        } else if renderer.craft_menu_open && esc_pressed {
            renderer.craft_menu_open = false;
            esc_pressed = false; // consumed
        }
        // r175: B diagnostic — the operator reports "b does nothing"; this
        // unconditional edge log localizes whether the bending block is
        // even reachable (captured/paused state) on their machine.
        if !typing && input.is_key_just_pressed(Keycode::B) {
            log::info!(
                "[bending] B pressed (captured={}, paused={}, survival={})",
                mouse_captured, game_paused, survival_mode
            );
        }
        // r525: F4 now toggles MenuState::Debug (handled in the unified block
        // above), which drives BOTH the perf dashboard and the debug panel.
        // r481: J cycles through the eight forged weapon models in hand (debug).
        if mouse_captured && input.is_key_just_pressed(Keycode::J) {
            let n = unendlich_engine::sim::viewmodel::WEAPON_CLASSES.len();
            debug_weapon_class = match debug_weapon_class {
                None => Some(0),
                Some(i) if i + 1 < n => Some(i + 1),
                _ => None,
            };
            println!(
                "[debug] hand weapon = {:?}",
                debug_weapon_class.map(|i| unendlich_engine::sim::viewmodel::WEAPON_CLASSES[i])
            );
        }
        // r481: Y gives one of each forged weapon as a real inventory item
        // (ids 4000..4007) — in survival, scroll the hotbar to one to hold it.
        // r515: cinematic over-the-shoulder toggle (rebindable — default Y).
        if bindings.is_action_just_pressed(
            unendlich_engine::keybindings::GameAction::ToggleCinematicCam, &input,
        ) && !renderer.ui_wants_keyboard {
            camera_mode = if camera_mode == 2 { 0 } else { 2 };
            self_avatar_prev = None;
            log::info!("[view] camera = {}", match camera_mode {
                1 => "third-person", 2 => "cinematic shoulder", _ => "first-person",
            });
        }
        // r518: F10 toggles the server between OPEN (anyone may join) and CLOSED (whitelist
        // only). Persists immediately so the policy survives a restart. The full add/remove-name
        // management lives in the Multiplayer menu (see DESIGN_NOTES UI plan).
        if input.is_key_just_pressed(Keycode::F10) && !renderer.ui_wants_keyboard {
            let now_open = whitelist.toggle_open();
            let _ = whitelist.save(&flora_root);
            log::info!(
                "[lan-mp] server is now {} ({} on allow-list)",
                if now_open { "OPEN — anyone may join" } else { "CLOSED — whitelist only" },
                whitelist.len()
            );
        }
        // r534: +/- adjust the third-person / cinematic camera distance. In those
        // views the keys zoom the chase camera (− pulls farther, + brings it closer);
        // in first-person they fall through to their normal bindings (e.g. the debug
        // weapon-give on Minus below).
        if camera_mode != 0 && mouse_captured && !renderer.ui_wants_keyboard {
            if input.is_key_just_pressed(Keycode::Minus) {
                cam_zoom = (cam_zoom * 1.12).min(3.0);
                log::info!("[view] camera distance x{:.2}", cam_zoom);
            }
            if input.is_key_just_pressed(Keycode::Equal) {
                cam_zoom = (cam_zoom * 0.89).max(0.4);
                log::info!("[view] camera distance x{:.2}", cam_zoom);
            }
        }
        if camera_mode == 0 && mouse_captured && !show_inventory && input.is_key_just_pressed(Keycode::Minus) {
            for c in unendlich_engine::sim::viewmodel::WEAPON_CLASSES {
                let id = unendlich_engine::items::ItemId(
                    unendlich_engine::sim::viewmodel::weapon_id_for_class(c));
                fg_app.inventory_mut().add_item(id, 1, &fg_item_registry);
            }
            log::info!("[items] gave all 8 forged weapons to inventory");
        }
        // r525: F3 is a second alias for MenuState::Debug (handled in the unified
        // block above, alongside F4) — both open the debug/shaderpack panel.
        if esc_pressed && !renderer.menu_open {
            // r167: Esc = PAUSE MENU (Resume / Settings / Save & Quit).
            renderer.pause_menu_open = !renderer.pause_menu_open;
            if renderer.pause_menu_open {
                window.set_cursor_mode(CursorMode::Normal);
                mouse_captured = false;
            }
            // (r170: no re-capture here — authority below.)
        }
        // r181: seed preview — regenerate when the wizard's fixed seed
        // changes. Cheap by design: 96x48 surface samples, biome color +
        // height shading, top-down equirect strip (x wraps, z half-band).
        {
            let wiz_seed: Option<u64> = fg_app.menu().wizard().map(|w| {
                match w.config().seed.clone() {
                    unendlich_engine::sim::new_world_config::SeedChoice::Fixed(v) => v,
                    // r185: Random previews a CONCRETE rolled seed (kept in
                    // fg_last_preview_seed; Create World adopts it).
                    _ => fg_last_preview_seed.unwrap_or_else(|| {
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_nanos() as u64)
                            .unwrap_or(7)
                    }),
                }
            });
            if wiz_seed != fg_last_preview_seed {
                fg_last_preview_seed = wiz_seed;
                if let Some(v) = wiz_seed {
                    use unendlich_engine::world::biomes::Biome;
                    let g = unendlich_engine::world::cpu_generator::WorldGenerator::new(v as u32);
                    let (w, h) = (96i32, 48i32);
                    let mut rgb = vec![0u8; (w * h * 3) as usize];
                    for py in 0..h {
                        for px in 0..w {
                            let wx = (px - w / 2) * 8;
                            let wz = (py - h / 2) * 8;
                            let sy = g.surface_y_at(wx, wz, -64, 200).unwrap_or(64);
                            let b = g.biome_at_3d(wx, sy, wz).biome;
                            let base: [f32; 3] = match b {
                                Biome::Ocean => [38.0, 84.0, 148.0],
                                Biome::Beach => [214.0, 198.0, 148.0],
                                Biome::Desert | Biome::RedSandstoneCanyon => [212.0, 170.0, 96.0],
                                Biome::Tundra | Biome::AlpineHighlands => [228.0, 234.0, 240.0],
                                Biome::Swamp => [86.0, 108.0, 70.0],
                                Biome::Jungle => [44.0, 120.0, 52.0],
                                Biome::Savanna => [168.0, 158.0, 84.0],
                                Biome::Taiga | Biome::CloudForest | Biome::MistyMountains => [70.0, 110.0, 92.0],
                                _ => [88.0, 140.0, 76.0],
                            };
                            let shade = 0.65 + 0.5 * ((sy.clamp(0, 160) as f32) / 160.0);
                            let i = ((py * w + px) * 3) as usize;
                            rgb[i] = (base[0] * shade).min(255.0) as u8;
                            rgb[i + 1] = (base[1] * shade).min(255.0) as u8;
                            rgb[i + 2] = (base[2] * shade).min(255.0) as u8;
                        }
                    }
                    renderer.wizard_preview_rgb = rgb;
                    renderer.wizard_preview_dirty = true;
                } else {
                    renderer.wizard_preview_rgb.clear();
                    renderer.wizard_preview_dirty = false;
                }
            }
        }
        // r186: ROLLBACK POINTS — timer (5 min), craft, distance (1000
        // blocks). Each snapshot = save_all + a reason-stamped marker in
        // worlds/<name>/rollbacks/ (the restore UI's future input).
        {
            let pp = local_player.view.position;
            let moved = ((pp.x - fg_rb_last_pos[0]).powi(2)
                + (pp.z - fg_rb_last_pos[1]).powi(2))
                .sqrt();
            if moved.is_finite() && moved < 64.0 {
                fg_rb_travel += moved;
            }
            fg_rb_last_pos = [pp.x, pp.z];
            // r206: dev "create history point now" button.
            if renderer.dev_make_history_point {
                renderer.dev_make_history_point = false;
                fg_rb_force_point = true;
            }
            // r206: dev mode drops these so history fills in a short test
            // session (operator: "history still not writing anything").
            let rb_timer = unendlich_engine::dev_mode::rollback_timer_frames();

            let rb_travel = unendlich_engine::dev_mode::rollback_travel_blocks();
            let timer_due = frame_count > 0 && frame_count % rb_timer == 0;
            let travel_due = fg_rb_travel >= rb_travel;
            let craft_due = fg_rb_craft_flag || fg_rb_force_point;

            if timer_due || travel_due || craft_due {
                fg_rb_travel = 0.0;
                fg_rb_craft_flag = false;
                let reason = if fg_rb_force_point { "manual" }
                    else if craft_due { "craft" }
                    else if travel_due { "travel" }
                    else { "timer" };
                fg_rb_force_point = false;

                let saved = chunk_manager.save_all();
                if let Some(name) = &current_world_name {
                    let dir = format!("worlds/{}/rollbacks", name.replace(['/', '\\', ' '], "_"));
                    let _ = std::fs::create_dir_all(&dir);
                    let ts = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    let _ = std::fs::write(
                        format!("{}/{}.point", dir, ts),
                        format!("reason={}\nchunks={}\n", reason, saved),
                    );
                }
                log::info!("[rollback] point ({reason}) — {saved} chunk(s) saved");
            }
        }
        // r185: preview click -> seed search. The clicked region names a
        // biome; ten rolled candidates are scored by that biome's coverage
        // and the best (left click) or least (right click) wins.
        if let Some((u, v, negative)) = renderer.wizard_preview_click.take() {
            if let Some(cur) = fg_last_preview_seed {
                use unendlich_engine::world::cpu_generator::WorldGenerator;
                let g0 = WorldGenerator::new(cur as u32);
                let wx = ((u - 0.5) * 96.0) as i32 * 8;
                let wz = ((v - 0.5) * 48.0) as i32 * 8;
                // r186: blended brush — 3x3 neighborhood majority vote.
                let mut votes: std::collections::HashMap<unendlich_engine::world::biomes::Biome, u8> =
                    std::collections::HashMap::new();
                for dz in -1i32..=1 {
                    for dx in -1i32..=1 {
                        let (bx, bz) = (wx + dx * 24, wz + dz * 24);
                        let by = g0.surface_y_at(bx, bz, -64, 200).unwrap_or(64);
                        *votes.entry(g0.biome_at_3d(bx, by, bz).biome).or_insert(0) += 1;
                    }
                }
                let want = votes.into_iter().max_by_key(|(_, n)| *n).map(|(b, _)| b)
                    .unwrap_or(unendlich_engine::world::biomes::Biome::Plains);
                let mut best: (u64, i64) = (cur, if negative { i64::MAX } else { i64::MIN });
                for k in 0..10u64 {
                    let cand = cur
                        .wrapping_mul(6364136223846793005)
                        .wrapping_add(1442695040888963407 + k * 0x9e3779b97f4a7c15);
                    let g = WorldGenerator::new(cand as u32);
                    let mut score: i64 = 0;
                    for py in 0..16 {
                        for px in 0..32 {
                            let sx = (px - 16) * 24;
                            let sz = (py - 8) * 24;
                            let sy = g.surface_y_at(sx, sz, -64, 200).unwrap_or(64);
                            if g.biome_at_3d(sx, sy, sz).biome == want {
                                score += 1;
                            }
                        }
                    }
                    let better = if negative { score < best.1 } else { score > best.1 };
                    if better {
                        best = (cand, score);
                    }
                }
                log::info!(
                    "[wizard] preview {}: {:?} -> seed {} (coverage {})",
                    if negative { "avoid" } else { "match" }, want, best.0, best.1
                );
                let _ = fg_app.tick(
                    unendlich_engine::app::AppInput::WizardSetSeed(
                        unendlich_engine::sim::new_world_config::SeedChoice::Fixed(best.0),
                    ),
                    0,
                );
            }
        }
        // r167/r178: act on the pause menu's choice — THROUGH the FSM now.
        // (r167 set renderer.menu_open directly; the app stayed InGame and
        // the fake title relaunched fresh worlds per click — operator bug.)
        match renderer.pause_action {
            1 => {
                renderer.pause_action = 0;
                renderer.pause_menu_open = false;
                let _ = fg_app.tick(unendlich_engine::app::AppInput::Resume, 0);
            }
            2 => {
                renderer.pause_action = 0;
                renderer.pause_menu_open = false;
                renderer.settings_menu_open = true; // r169: the real settings
            }
            3 => {
                renderer.pause_action = 0;
                renderer.pause_menu_open = false;
                let saved = chunk_manager.save_all();
                log::info!("[pause] Save & Quit to Title ({} chunk(s) saved)", saved);
                // r204 BUGFIX: quitting to title left the world's uploaded
                // meshes resident (operator: "mesh not cleared when ... going
                // to title screen"), so the title showed stale geometry and
                // it bled into the next world. Clear the renderer's mesh map;
                // the next launch's rebuild_for_seed resets the chunk manager
                // and bumps world_epoch (dropping any in-flight mesh jobs).
                renderer.clear_all_chunk_meshes();
                let _ = fg_app.tick(unendlich_engine::app::AppInput::QuitToTitle, 0);
                // renderer.menu_open now follows fg_app.state() in the sync
                // block — the title is the REAL root screen, not an overlay.
            }
            _ => {}
        }
        // r178: opening/closing the pause menu informs the FSM too.
        if renderer.pause_menu_open && fg_app.state() == unendlich_engine::sim::game_flow::AppState::InGame {
            let _ = fg_app.tick(unendlich_engine::app::AppInput::Pause, 0);
        } else if !renderer.pause_menu_open
            && fg_app.state() == unendlich_engine::sim::game_flow::AppState::Paused
        {
            let _ = fg_app.tick(unendlich_engine::app::AppInput::Resume, 0);
        }
        if unendlich_engine::debug_settings::DEBUG
            .request_quit
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            window.set_should_close(true);
        }

        // Click-to-capture mouse — suppressed whenever ANY ui surface owns
        // the cursor so its clicks land (r168: the pause menu was missing
        // from this guard — clicking it recaptured the mouse and ate the
        // click; menu/wizard included for safety, one guard for all).
        let any_ui_owns_cursor = ui_panel_open
            || renderer.settings_open
            || renderer.pause_menu_open
            || renderer.settings_menu_open // r169
            || renderer.craft_menu_open // r169
            || renderer.menu_open
            || renderer.wizard_open
            || show_inventory;
        // r170: CURSOR AUTHORITY — the single reconciliation point.
        // UI open while captured -> release NOW (covers windows opened by
        // any path: latches, actions, keys). Recapture happens only below
        // via click, and Resume recaptures by clicking into the world.
        if any_ui_owns_cursor && mouse_captured {
            window.set_cursor_mode(CursorMode::Normal);
            mouse_captured = false;
            input.cursor_delta = (0.0, 0.0);
            log::info!("[ui] cursor released to UI (authority)");
        }
        if !mouse_captured && !any_ui_owns_cursor && input.is_mouse_button_just_pressed(MouseBtn::Left) {
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
        let frame_dt_s = (dt_ms as f32) / 1000.0; // r486: for the held-item swing decay

        // ── r526: MOB AI + MOVEMENT ──────────────────────────────────────
        // Tick each test mob's brain (wander / seek / flee) and advance it with
        // the SAME `step_character` kernel the player uses, against the live
        // voxel terrain, at a fixed 20 Hz (frame-rate independent). Static test
        // spawns now actually move. Skipped while paused / in a menu.
        if !game_paused && !test_mobs.is_empty() {
            use unendlich_engine::sim::fixed::{Fixed, Vec3};
            use unendlich_engine::sim::mob::MobKind;
            use unendlich_engine::sim::state::Character;
            mob_tick_accum += dt;
            let pp = local_player.view.position;
            let players = [Character {
                pos: Vec3::new(
                    Fixed::from_int(pp.x.floor() as i64),
                    Fixed::from_int(pp.y.floor() as i64),
                    Fixed::from_int(pp.z.floor() as i64),
                ),
                ..Character::default()
            }];
            let mut guard = 0;
            while mob_tick_accum >= 0.05 && guard < 4 {
                mob_tick_accum -= 0.05;
                guard += 1;
                mob_sim_tick += 1;
                let prey: Vec<Vec3> = test_mobs
                    .iter()
                    .filter(|m| m.kind == MobKind::Grazer)
                    .map(|m| m.body.pos)
                    .collect();
                let predators: Vec<Vec3> = test_mobs
                    .iter()
                    .filter(|m| m.kind == MobKind::Seeker)
                    .map(|m| m.body.pos)
                    .collect();
                let world = MobWorld { cm: &chunk_manager };
                for (i, m) in test_mobs.iter_mut().enumerate() {
                    let input = m.brain(
                        fg_world_seed,
                        i as u64,
                        mob_sim_tick,
                        &players,
                        &prey,
                        &predators,
                        &world,
                    );
                    m.body = unendlich_engine::sim::physics::step_character(&m.body, &input, &world);
                }
            }
        }
        last_frame = now;

        // r476: dynamic render-distance controller. Wall-clock cadence (every
        // 400ms) so it reacts even at single-digit FPS. Shrinks the cap when the
        // last frame ran long, grows it when frames are comfortably under budget.
        // Catastrophic frames (>3x shrink threshold) cut two rings at once so the
        // game can climb out of a streaming death-spiral quickly. The cap only
        // limits BELOW the player's chosen distance (applied at the override).
        if renderer.ui_adaptive_rd && adaptive_rd_last.elapsed() >= std::time::Duration::from_millis(400) {
            adaptive_rd_last = now;
            if dt_ms > adaptive_rd_shrink_ms && adaptive_rd_cap > adaptive_rd_min {
                let step = if dt_ms > adaptive_rd_shrink_ms * 3.0 { 2 } else { 1 };
                adaptive_rd_cap = (adaptive_rd_cap - step).max(adaptive_rd_min);
                adaptive_rd_good_streak = 0; // a heavy frame breaks the grow streak
                log::info!("[adaptive_rd] frame {:.0}ms -> render-distance cap {}", dt_ms, adaptive_rd_cap);
            } else if dt_ms < adaptive_rd_grow_ms && adaptive_rd_cap < 32 {
                // Only grow after a run of consecutive good frames, and never more
                // than +1 per grow — so adding a ring can't pile onto a fresh spike.
                adaptive_rd_good_streak += 1;
                if adaptive_rd_good_streak >= adaptive_rd_grow_streak {
                    adaptive_rd_good_streak = 0;
                    adaptive_rd_cap += 1;
                    log::debug!("[adaptive_rd] frame {:.0}ms -> render-distance cap {} (grew)", dt_ms, adaptive_rd_cap);
                }
            } else {
                // a middling frame (between grow and shrink) neither grows nor resets
                // hard — let the streak hold so brief jitter doesn't stall the ramp.
            }
            // Apply immediately on this cadence (the %15 slider block below is far
            // too slow to react when the frame rate has already collapsed).
            let effective_rd = renderer.ui_render_distance.min(adaptive_rd_cap).max(2);
            unendlich_engine::constants::set_render_distance_override(effective_rd);
        }
        // rNNN: WEATHER SIM readout — every ~2s, sample the physical weather for
        // the player's column and log it (no gameplay/render side effects).
        // effective_temp falls with altitude and drifts with the game-clock
        // season; weather_state composes cell + storm organization + snowpack.
        if weather_sim_enabled && weather_last_sample.elapsed().as_secs_f32() >= 2.0 {
            let sample_dt = weather_last_sample.elapsed().as_secs_f32();
            weather_last_sample = now;
            let pos = local_player.view.position;
            // REAL climate field at the player's column: temperature (°C) + moisture.
            let clim = unendlich_engine::sim::biome_climate::climate_at(
                fg_world_seed, pos.x, pos.z, pos.y,
            );
            // °C → [0,1] base temp (−10 °C → 0, 30 °C → 1) for the climate core.
            let base_temp = ((clim.temperature_c + 10.0) / 40.0).clamp(0.0, 1.0);
            last_climate_temp_c = clim.temperature_c as f32; // r492: rain-vs-snow input
            let day_of_year = season_day; // r495: season-offset adjustable
            // Run the REAL climate composition (season offset + snow line + cover).
            let surface = unendlich_engine::world::climate_surface::resolve(
                unendlich_engine::world::climate_surface::ColumnClimate {
                    day_of_year,
                    year_length: season_days_per_year,
                    seasonal_amplitude: 0.3,
                    base_temp,
                    precip: clim.moisture01,
                    slope: 0.0,
                    altitude: pos.y,
                    snow_line_altitude: 120.0,
                },
            );
            let altitude_norm = ((pos.y - 60.0) / 140.0).clamp(0.0, 1.0);
            let inputs = unendlich_engine::world::weather_state::StepInputs {
                atmo: unendlich_engine::world::seasonal_weather::AtmoInputs {
                    windward_moisture: clim.moisture01,
                    peak_height: 0.0,
                    dryness: 0.0,
                    altitude_norm,
                    pressure_diff: 15.0,
                    pressure_distance: 100.0,
                    wind_coeff: 2.0,
                },
                shear_top_altitude: 4000.0,
                shear: 1.0,
                temp_min_c: -10.0,
                temp_max_c: 30.0,
                dt: sample_dt,
            };
            let w = unendlich_engine::world::weather_state::step(
                &surface, &inputs, weather_snow_depth,
            );
            weather_snow_depth = w.snow_depth;
            log::info!(
                "[weather] {:.0}\u{b0}C temp01={:.2} moist={:.2} | cloud={:.2} precip={:.2} storm={:.2} org={:.2} {:?} wind={:.2} snow={:.2}",
                clim.temperature_c, surface.effective_temp, clim.moisture01,
                w.cell.cloud, w.cell.precip, w.cell.storm, w.organization, w.mode,
                w.cell.wind, w.snow_depth
            );
        }
        // r265: advance the CPU particle pool (no-op when disabled). Gravity
        // pulls dust down; a gentle constant wind drifts it (matches the smoke
        // field's wind). The pool + render_buffer() are ready; DRAWING them needs
        // a new sprite/point render pipeline in world_renderer (no particle
        // upload path exists today — upload_entities is creature meshes). That
        // pipeline is GPU work for the M2; until then the system ticks but is
        // invisible. To verify the tick on hardware: log ps.len() here.
        if let Some(ps) = particle_sys.as_mut() {
            ps.step(dt, [0.0, -9.81, 0.0], [0.4, 0.0, 0.2]);
        }
        frame_times_ms.push(dt_ms);
        // r177: ASCII frame-time graph to the log (analytics button) —
        // 100 cols x 12 rows, newest right, fixed 0..50ms scale; plus the
        // STUTTER ADVISOR: live counters vs known levers.
        if renderer.perf_export_ascii {
            renderer.perf_export_ascii = false;
            let n = frame_times_ms.len();
            let lo = n.saturating_sub(100);
            let win: Vec<f32> = frame_times_ms[lo..].iter().map(|v| *v as f32).collect();
            let rows = 12usize;
            let ceil_ms = 50.0f32;
            let mut lines = vec![String::new(); rows];
            for v in &win {
                let h = ((v / ceil_ms).min(1.0) * rows as f32).ceil() as usize;
                for r in 0..rows {
                    lines[r].push(if rows - r <= h { '#' } else { ' ' });
                }
            }
            log::info!("[perf] frame-time ascii ({} frames, ceiling {:.0}ms, newest right)", win.len(), ceil_ms);
            for (r, l) in lines.iter().enumerate() {
                log::info!("[perf] {:>3.0}ms |{}", ceil_ms * (rows - r) as f32 / rows as f32, l);
            }
            // ── advisor: live counters -> the levers you already run ──
            let mut sorted = win.clone();
            sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let p50 = sorted.get(sorted.len() / 2).copied().unwrap_or(0.0);
            let p99 = sorted.get(sorted.len() * 99 / 100).copied().unwrap_or(0.0);
            log::info!("[perf] p50={:.1}ms p99={:.1}ms (stutter ratio {:.1}x)", p50, p99, if p50 > 0.0 { p99 / p50 } else { 0.0 });
            let mut tips: Vec<String> = Vec::new();
            if heat_sources.len() > 200 {
                tips.push(format!("heat sources {} — lava registration high; expect smoke cost", heat_sources.len()));
            }
            if roamers.len() > 20 {
                tips.push(format!("herd {} — mob ticks (incl. grazing probes) scale per-creature; try lower spawn caps", roamers.len()));
            }
            if fg_app.drops().len() > 60 {
                tips.push(format!("drops {} — drop physics per item; sweep or wait for despawn", fg_app.drops().len()));
            }
            tips.push("your last paste: mesher near=12-15ms x 66-84/s while descending — SUBMIT dominates; UNENDLICH_GPU_MESH_BATCH=4 -> 2 and UNENDLICH_MAX_UPLOADS_PER_FRAME=4 -> 2 trade load-in speed for smoothness".to_string());
            tips.push("plant_lifecycle + leaf_decay run in the chunk-tick slot — if the 'tick:' log shows >5ms there, lower UNENDLICH_APPLY_BUDGET (you run 8) or expect periodic spikes after tree-felling".to_string());
            tips.push("shadow re-render spikes track UNENDLICH_SHADOW_CHUNK_STRIDE=5 cadence — stride up or shrink UNENDLICH_SHADOW_MAP_SIZE to test".to_string());
            for tip in tips {
                log::info!("[advisor] {}", tip);
            }
        }
        // r175: SLIDING strip — push EVERY frame into a capped ring so the
        // graph scrolls continuously (rebuilding a window snapshot made it
        // look frozen between updates at low fps). Stats refresh stays slow.
        renderer.perf_samples.push(dt_ms as f32);
        if renderer.perf_samples.len() > 240 {
            let overflow = renderer.perf_samples.len() - 240;
            renderer.perf_samples.drain(0..overflow);
        }
        if renderer.perf_window_open || renderer.settings_menu_open {
            let n = frame_times_ms.len();
            let lo = n.saturating_sub(120);
            if frame_count % 30 == 0 {
                let avg = if n > lo { frame_times_ms[lo..].iter().sum::<f64>() / (n - lo) as f64 } else { 0.0 };
                // r178: narrative tier first (prose), raw tier below.
                let avg_fps = if avg > 0.0 { 1000.0 / avg } else { 0.0 };
                renderer.perf_stats = vec![
                    format!(
                        "Running at about {:.0} fps. The herd is {} strong with {} item(s) on the ground.",
                        avg_fps, roamers.len(), fg_app.drops().len()
                    ),
                    format!(
                        "{} chunks resident, {} still generating; {} heat source(s) burning.",
                        chunk_manager.loaded_count(),
                        chunk_manager.pending_count(),
                        heat_sources.len()
                    ),
                    String::new(),
                    format!("raw: fps~{:.0} avg={:.2}ms herd={} drops={} chunks={}/{} heat={}",
                        avg_fps, avg, roamers.len(), fg_app.drops().len(),
                        chunk_manager.loaded_count(), chunk_manager.pending_count(),
                        heat_sources.len()),
                ];
            }
        }

        // r316: perpetual fuzz sweep — opt-in (see fg_sim_sweep above). A degenerate
        // generated input here was crashing normal play (panic=abort defeats the
        // kernel host's catch_unwind; the service sweep has none).
        if fg_sim_sweep {
            fg_services.tick_all(&unendlich_engine::sim::service_registry::ServiceCtx { dt });
            fg_kernels.exercise();
        }
        if !game_paused {
            world_clock_elapsed += dt; // r121 world clock (r161: frozen while paused)
        }
        // r139: popdyn step — humidity-scaled grass regen vs herd grazing.
        // r461: named-location discovery — when the player crosses into a new biome-scale region,
        // give it a deterministic lore name and announce it (Collections tab is the device-side view).
        if mouse_captured && frame_count % 45 == 17 {
            use unendlich_engine::world::named_locations as nl;
            let pp = local_player.view.position;
            let reg = nl::region_of(pp.x.floor() as i32, pp.z.floor() as i32);
            if session.current_region != Some(reg) {
                session.current_region = Some(reg);
                if !session.locations.is_discovered(reg.0, reg.1) {
                    let biome = chunk_manager
                        .generator()
                        .cpu()
                        .biome_at_3d(pp.x.floor() as i32, pp.y.floor() as i32, pp.z.floor() as i32)
                        .biome;
                    let name = nl::location_name(reg.0, reg.1, biome, fg_world_seed);
                    if session.locations.discover(reg.0, reg.1, name.clone()) {
                        log::info!(
                            "[location] Discovered: {name} ({} catalogued)",
                            session.locations.count()
                        );
                    }
                }
            }
        }
        if frame_count % 30 == 0 {
            let humid = {
                let m = chunk_manager.model_arc();
                let pp = local_player.view.position;
                m.humidity(pp.x as f64, pp.y as f64, pp.z as f64, 0.0) as f32
            };
            // r641: refresh the fog extinction used by creature vision from the
            // same base (0.012) + humidity the renderer fogs with; NO_FOG clears it.
            frame_fog_sigma = if no_fog_enabled {
                0.0
            } else {
                unendlich_engine::sim::fog_model::shader_fog_density(0.012, humid)
            };
            fg_colony.step(0.5, 0.4 + 0.8 * humid, 0.05, ROAMER_CAP);
            let carrying = fg_colony.population().max(8);
            // Overshoot: cull the hungriest non-persistent roamer (starvation).
            if roamers.len() > carrying + 12 {
                if let Some((idx, _)) = roamers
                    .iter()
                    .enumerate()
                    .filter(|(_, r)| !r.persistent)
                    .min_by(|a, b| {
                        a.1.vitality
                            .energy
                            .partial_cmp(&b.1.vitality.energy)
                            .unwrap_or(std::cmp::Ordering::Equal)
                    })
                {
                    let k = roamers[idx].kind;
                    let p = roamers[idx].creature.body.position;
                    roamers.swap_remove(idx);
                    fg_app.drops_mut().spawn(
                        unendlich_engine::items::ItemStack {
                            item: unendlich_engine::items::ItemId(MEAT_ITEM),
                            count: 1,
                        },
                        [p[0], p[1] + 0.4, p[2]],
                        [0.0, 1.0, 0.0],
                    );
                    if frame_count % 600 == 0 {
                        log::info!(
                            "[popdyn] {:?} starved (herd {} > capacity {})",
                            k, roamers.len() + 1, carrying
                        );
                    }
                }
            }
        }
        // r145: SPAWN DIRECTOR — every ~45s the engine's mob_spawn_director
        // plans time-of-day/biome-appropriate spawns (its own no-spawn ring
        // and per-kind caps), gated by popdyn capacity. Night gets teeth.
        if frame_count % 2700 == 1350 && mouse_captured {
            use unendlich_engine::sim::mob_spawn_director as msd;
            let pp = local_player.view.position;
            let cap = fg_colony.population().max(8).min(ROAMER_CAP);
            if roamers.len() < cap {
                let biome_here = chunk_manager
                    .generator()
                    .cpu()
                    .biome_at_3d(pp.x.floor() as i32, pp.y.floor() as i32, pp.z.floor() as i32)
                    .biome;
                let mut counts = [0u32; unendlich_engine::sim::mob_spawn_director::KINDS];
                for r in &roamers {
                    let ki = match r.kind {
                        unendlich_engine::item::CreatureKind::Dinosaur => 1,
                        unendlich_engine::item::CreatureKind::Dragon => 2,
                        _ => 0,
                    };
                    if ki < counts.len() { counts[ki] += 1; }
                }
                // r177: no land herds in the water — Ocean gets its own
                // ecology at the coral session (fluid_pois doc).
                let reqs = if matches!(biome_here, unendlich_engine::world::biomes::Biome::Ocean) {
                    Vec::new()
                } else {
                    msd::plan_spawns(
                    msd::SpawnCtx {
                        seed: fg_world_seed ^ (frame_count as u64),
                        biome: biome_here,
                        time_of_day01: sky_state.day_phase(),
                        center: [pp.x, pp.y, pp.z],
                        player_pos: [pp.x, pp.y, pp.z],
                        no_spawn_radius: 24.0,
                        spawn_radius: 56.0,
                        active_counts: counts,
                        // r200: dragon cap 2 -> 1 (operator: "way too
                        // common"). With spacing below, at most one dragon
                        // lives near the player at a time.
                        caps: [24, 6, 1, 4, 4],
                    },
                    (cap - roamers.len()).min(3) as u32,
                    )
                }; // r177: end ocean gate
                for rq in reqs {
                    if roamers.len() >= cap { break; }
                    let sx = rq.position[0];
                    let sz = rq.position[2];
                    let Some(sy) = chunk_manager.generator().cpu()
                        .surface_y_at(sx.floor() as i32, sz.floor() as i32, probe_y_min, probe_y_max)
                    else { continue };
                    let kind = rq.kind;
                    // r200: DRAGON SPACING. A dragon may not spawn within
                    // DRAGON_MIN_SPACING of another dragon, so they spread
                    // across the world instead of clustering near the player
                    // (operator: "their spawn needs to be spread out"). Cheap
                    // O(active dragons) scan; dragons are rare so this is a
                    // handful of checks at most.
                    if matches!(kind, unendlich_engine::item::CreatureKind::Dragon) {
                        // r545 (Roll 2 / B3): hard cap on concurrent dragons so heavy
                        // seeds don't swarm the player (operator note r441). The 220u
                        // spacing below stops clustering; this caps the global count.
                        // Tunable via UNENDLICH_MAX_DRAGONS (default 6).
                        let max_dragons = std::env::var("UNENDLICH_MAX_DRAGONS")
                            .ok()
                            .and_then(|s| s.parse::<usize>().ok())
                            .unwrap_or(6);
                        let dragon_count = roamers
                            .iter()
                            .filter(|r| {
                                matches!(r.kind, unendlich_engine::item::CreatureKind::Dragon)
                            })
                            .count();
                        if dragon_count >= max_dragons {
                            continue;
                        }
                        const DRAGON_MIN_SPACING: f32 = 220.0;
                        let too_close = roamers.iter().any(|r| {
                            if !matches!(r.kind, unendlich_engine::item::CreatureKind::Dragon) {
                                return false;
                            }
                            let dx = r.creature.body.position[0] - sx;
                            let dz = r.creature.body.position[2] - sz;
                            (dx * dx + dz * dz) < DRAGON_MIN_SPACING * DRAGON_MIN_SPACING
                        });
                        if too_close {
                            continue;
                        }
                    }

                    let wseed = (fg_world_seed)
                        ^ ((sx as i32 as u32 as u64) << 20)
                        ^ (sz as i32 as u32 as u64)
                        ^ 0xD1EC;
                    let mut cr = unendlich_engine::spawn::spawn(kind, [sx, sy as f32 + 1.0, sz]);
                    cr.body.position[1] = cr.grounded_y(sy as f32 + 1.0);
                    let genome = unendlich_engine::genetics::Genome::from_seed(wseed);
                    let ph = genome.express(wseed, 0);
                    // r462: pick a biome-appropriate wild species from the roster for generic Fauna.
                    let picked_species = if matches!(kind, unendlich_engine::item::CreatureKind::Fauna) {
                        let _b = chunk_manager.generator().cpu().biome_at_3d(sx as i32, sy as i32, sz as i32).biome;
                        let _roll = ((wseed >> 16) & 0xffff) as f32 / 65535.0;
                        let _s = unendlich_engine::sim::fauna_roster::pick_for_biome(_b, _roll);
                        if let Some(sp) = _s { log::info!("[fauna] {} appears ({:?})", unendlich_engine::sim::fauna_roster::data(sp).name, _b); }
                        _s
                    } else { None };
                    let spawn_hp = picked_species.map(|s| unendlich_engine::sim::fauna_roster::data(s).base_health as i32).unwrap_or(40);
                    roamers.push(Roamer {
                        bond: unendlich_engine::sim::bond_tame::Bond::Wild,
                        trust: 0.0,
                        hp: spawn_hp,
                        creature: cr,
                        wander: unendlich_engine::sim::cognition::wander::Wander::new(
                            wseed, 0.8 + ph.speed * 1.4,
                        ),
                        home: (
                            (sx.floor() as i32).div_euclid(unendlich_engine::constants::CHUNK_SIZE),
                            (sz.floor() as i32).div_euclid(unendlich_engine::constants::CHUNK_SIZE),
                        ),
                        persistent: false,
                        senses: unendlich_engine::sim::senses::senses_for(kind, &genome, wseed, 0),
                        turn_rate: 1.5 + ph.turn_rate * 4.0,
                        turn_profile: unendlich_engine::sim::cognition::steering::TurnProfile::from_seed(wseed),
                        vitality: unendlich_engine::sim::feeding::Vitality::full(),
                        sex: unendlich_engine::genetics::sex_from_seed(wseed),
                        kind,
                        species: picked_species,
                        genome,
                        forage_memory: unendlich_engine::sim::forage_memory::ForageMemory::new(16),
                        leap: unendlich_engine::sim::leap::Leap::from_genes(ph.speed, ph.speed),
                        airborne: None,
                        posture: unendlich_engine::sim::posture::Posture::new(0.0),
                        stumble: unendlich_engine::sim::stumble::Stumble::new(),
                        flying: false,
                        flight_phase: (wseed % 30) as f32,
                        last_tick_frame: 0,
                        aggro: 0.0,
                        prev_behavior: 0,
                        combat_habit: 0.0,
                        disposition_cache: None,
                        graze_target: None,
                        graze_cell: (i32::MIN, i32::MIN),
                        graze_scan_tick: 0,
                    });
                    log::info!(
                        "[director] {:?} spawned at ({:.0},{:.0}) — {} (phase {:.2})",
                        kind, sx, sz, roamers.len(), sky_state.day_phase()
                    );
                }
            }
        }
        // r165: SLEEP — at night (day_phase in the dark band), holding
        // Sneak while standing still for ~3s with no predator within 24
        // blocks rolls time forward to sunrise. The night has dangers;
        // sleeping is how you opt out — if it's safe.
        if mouse_captured && !game_paused && survival_mode {
            let phase = sky_state.day_phase();
            let is_night = phase > 0.55 && phase < 0.95;
            let still = last_player_speed < 0.2;
            // r166: retriggered per operator design — Sneak is the HIDING
            // verb (see player_hidden below); the rest is its own deliberate
            // gesture: HOLD F7 (placeholder until the actions/ritual menu).
            let resting_gesture = input.is_key_pressed(Keycode::F7);
            if is_night && still && resting_gesture {
                let pp = local_player.view.position;
                let predator_near = roamers.iter().any(|r| {
                    matches!(
                        r.kind,
                        unendlich_engine::item::CreatureKind::Dinosaur
                            | unendlich_engine::item::CreatureKind::Dragon
                    ) && {
                        let dx = r.creature.body.position[0] - pp.x;
                        let dz = r.creature.body.position[2] - pp.z;
                        dx * dx + dz * dz < 24.0 * 24.0
                    }
                });
                if predator_near {
                    if frame_count % 240 == 0 {
                        log::info!("[sleep] too dangerous — something prowls nearby");
                    }
                    sleep_hold = 0.0;
                } else {
                    sleep_hold += dt;
                    if sleep_hold > 3.0 {
                        // roll sky time to the next sunrise (phase 0).
                        let day = sky_state.day_length as f64;
                        let into = sky_state.time_seconds % day;
                        sky_state.time_seconds += day - into;
                        world_clock_elapsed += ((day - into) / day) as f32 * 1200.0;
                        sleep_hold = 0.0;
                        log::info!("[sleep] the rest ritual completes — dawn breaks");
                    }
                }
            } else {
                sleep_hold = 0.0;
            }
        }
        // r164: AUTOSAVE — flush edited chunks every ~2 min; crash-loss is
        // bounded by the window (eviction + shutdown saves still apply).
        if frame_count % 7200 == 7100 {
            let saved = chunk_manager.save_all();
            if saved > 0 {
                log::info!("[saves] autosave: {} edited chunk(s) flushed", saved);
            }
        }
        // r156: heavy rain DOUSES unsheltered fires — one check per ~4s;
        // a fire under solid cover survives the storm.
        if precip01 > 0.7 && frame_count % 240 == 60 && !session.fire_positions.is_empty() {
            let before = session.fire_positions.len();
            session.fire_positions.retain(|fp: &[f32; 3]| {
                (1..=4).any(|dy| {
                    !matches!(
                        chunk_manager.block_at(
                            fp[0].floor() as i32,
                            fp[1].floor() as i32 + dy,
                            fp[2].floor() as i32,
                        ),
                        unendlich_engine::world::blocks::BlockId::Air
                    )
                })
            });
            let doused = before - session.fire_positions.len();
            if doused > 0 {
                log::info!("[weather] the storm doused {} fire(s) — build under cover", doused);
            }
        }
        // r153: CAVE GARDENS — underground (solid well above the head), a
        // cave_flora stem occasionally grows from a nearby floor cell using
        // REAL moisture/temperature closures over the climate model and a
        // REAL occupancy closure over block_at; its grown nodes become Moss.
        if frame_count % 1800 == 900 && mouse_captured {
            let pp = local_player.view.position;
            let py = pp.y.floor() as i32;
            let underground = (8..=24).any(|dy| {
                !matches!(
                    chunk_manager.block_at(pp.x.floor() as i32, py + dy, pp.z.floor() as i32),
                    unendlich_engine::world::blocks::BlockId::Air
                )
            });
            if underground {
                use unendlich_engine::world::cave_flora as cf;
                let m = chunk_manager.model_arc();
                let occupied = |c: (i32, i32, i32)| {
                    !matches!(
                        chunk_manager.block_at(c.0, c.1, c.2),
                        unendlich_engine::world::blocks::BlockId::Air
                    )
                };
                // find a floor cell: air with solid below, within 10 blocks
                let rseed = (frame_count as u64) ^ ((pp.x as i64 as u64) << 24) ^ (pp.z as i64 as u64);
                let ox = ((rseed >> 4) % 17) as i32 - 8;
                let oz = ((rseed >> 9) % 17) as i32 - 8;
                let bx = pp.x.floor() as i32 + ox;
                let bz = pp.z.floor() as i32 + oz;
                let mut root: Option<(i32, i32, i32)> = None;
                for dy in -6..=4 {
                    let c = (bx, py + dy, bz);
                    if !occupied(c) && occupied((c.0, c.1 - 1, c.2)) {
                        root = Some(c);
                        break;
                    }
                }
                if let Some(rc) = root {
                    let moisture = |c: (i32, i32, i32)| {
                        m.humidity(c.0 as f64, c.1 as f64, c.2 as f64, 0.0) as f32
                    };
                    let temperature = |c: (i32, i32, i32)| {
                        m.temperature(c.0 as f64, c.1 as f64, c.2 as f64, 0.0) as f32
                    };
                    let exposure = |c: (i32, i32, i32)| cf::sky_exposure(c, &occupied);
                    let flora = cf::CaveFlora::grow(
                        rc,
                        (0, 1, 0),
                        rseed,
                        10,
                        &moisture,
                        &temperature,
                        &occupied,
                        &exposure,
                    );
                    let mut placed = 0;
                    for n in &flora.stem.nodes {
                        if chunk_manager.set_block(
                            n.cell.0,
                            n.cell.1,
                            n.cell.2,
                            unendlich_engine::world::blocks::BlockId::Moss,
                        ) {
                            placed += 1;
                        }
                    }
                    if placed > 2 {
                        log::info!(
                            "[cave] a moss garden grew nearby ({} node(s)) — follow the green",
                            placed
                        );
                    }
                }
            }
        }
        // r582: AIR-CLEANUP — cut flora + floating tree trunks that are growing on air.
        // Root cause is the FP32/FP64 surface mismatch (NEXT_SESSION_GUIDE §4a): placement
        // uses the CPU FP64 surface prediction while the GPU generates voxels in FP32, so
        // things land where the predicted surface was, not the real one. The proper fix is
        // to switch placement onto sim::surface_scan::top_solid_y_in_chunk (reads the actual
        // top solid voxel — handoff/HANDOFF_world_surface.md); physics-anchored plants come
        // later. Until then this budgeted pass removes the visible offenders near the player.
        if air_cleanup_enabled && mouse_captured && frame_count % air_cleanup_interval == 73 {
            use unendlich_engine::world::blocks::BlockId as B;
            let pp = local_player.view.position;
            let (pxi, pyi, pzi) = (pp.x.floor() as i32, pp.y.floor() as i32, pp.z.floor() as i32);
            let r = flora_spread_radius.max(2);
            let base = (frame_count as u64)
                .wrapping_mul(0x2545_F491_4F6C_DD1D)
                ^ ((pxi as i64 as u64) << 24)
                ^ (pzi as i64 as u64);
            let is_flora = |b: B| matches!(b, B::Flower | B::TallGrass | B::Fern | B::Mushroom);
            let is_log = |b: B| matches!(b, B::OakLog | B::BirchLog);
            let no_support = |b: B| matches!(b, B::Air | B::Water);
            let mut cut = 0u32;
            for s in 0..(flora_spread_scan as u64 * 2) {
                if cut >= air_cleanup_budget {
                    break;
                }
                let h = unendlich_engine::util::hash_util::mix64(base ^ s.wrapping_mul(0x9E37_79B9));
                let ox = (h % (2 * r as u64 + 1)) as i32 - r;
                let oz = ((h >> 21) % (2 * r as u64 + 1)) as i32 - r;
                let (bx, bz) = (pxi + ox, pzi + oz);
                // scan the column around the player's height for something growing on air.
                for dy in (-14..=14).rev() {
                    let ay = pyi + dy;
                    let blk = chunk_manager.block_at(bx, ay, bz);
                    let below = chunk_manager.block_at(bx, ay - 1, bz);
                    if is_flora(blk) && no_support(below) {
                        if chunk_manager.set_block(bx, ay, bz, B::Air) {
                            cut += 1;
                        }
                        break;
                    }
                    if is_log(blk) {
                        // Found a trunk. Walk to its base (lowest connected log) and judge
                        // its footing. Two distinct conditions:
                        //   • base on AIR  → a placement artifact (FP surface gap): cut whole.
                        //   • base in WATER → genome decides. An aquatic lineage (rare, ~10%)
                        //     belongs there (mangrove-like) and is spared; any other lineage
                        //     ROTS — one crown layer removed per cleanup cycle — so it dies
                        //     back gradually and organically instead of vanishing at once.
                        let mut base_y = ay;
                        let mut g = 0;
                        while is_log(chunk_manager.block_at(bx, base_y - 1, bz)) && g < 48 {
                            base_y -= 1;
                            g += 1;
                        }
                        let base_below = chunk_manager.block_at(bx, base_y - 1, bz);
                        let water_beside = [(-1, 0), (1, 0), (0, -1), (0, 1)]
                            .iter()
                            .any(|&(dx, dz)| {
                                chunk_manager.block_at(bx + dx, base_y, bz + dz) == B::Water
                            });
                        let in_water = base_below == B::Water || water_beside;
                        let floating = base_below == B::Air;
                        if floating {
                            // placement artifact — remove the whole floating trunk + leaves
                            let mut yy = base_y;
                            let mut guard = 0;
                            while is_log(chunk_manager.block_at(bx, yy, bz)) && guard < 48 {
                                for (dx, dz) in [(0, 0), (-1, 0), (1, 0), (0, -1), (0, 1)] {
                                    let nb = chunk_manager.block_at(bx + dx, yy, bz + dz);
                                    if matches!(nb, B::OakLeaves | B::BirchLeaves) || is_log(nb) {
                                        let _ =
                                            chunk_manager.set_block(bx + dx, yy, bz + dz, B::Air);
                                    }
                                }
                                yy += 1;
                                guard += 1;
                            }
                            cut += 1;
                            water_dirty = true;
                        } else if in_water {
                            let genes = unendlich_engine::sim::plant_survival::SurvivalGenes::from_seed(
                                (bx as i64 as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
                                    ^ ((bz as i64 as u64) << 1),
                            );
                            if !genes.is_aquatic() {
                                // ROT: strip the topmost log layer (+ its hugging leaves).
                                let mut top_y = base_y;
                                let mut guard = 0;
                                while is_log(chunk_manager.block_at(bx, top_y + 1, bz)) && guard < 48
                                {
                                    top_y += 1;
                                    guard += 1;
                                }
                                for (dx, dz) in [(0, 0), (-1, 0), (1, 0), (0, -1), (0, 1)] {
                                    let nb = chunk_manager.block_at(bx + dx, top_y, bz + dz);
                                    if matches!(nb, B::OakLeaves | B::BirchLeaves) || is_log(nb) {
                                        let _ =
                                            chunk_manager.set_block(bx + dx, top_y, bz + dz, B::Air);
                                    }
                                }
                                cut += 1;
                                water_dirty = true;
                            }
                            // aquatic lineage → leave it standing (a water-loving tree)
                        }
                        break; // a trunk owns this column — judged, move on
                    }
                }
            }
            if cut > 0 {
                log::info!("[flora] cleared {} plant(s)/tree(s) that were growing on air", cut);
            }
        }
        // r450: FLORA SPREAD — surface plants seed nearby suitable ground using the
        // sim::flora_spread core (plant_propagation dispersal × plant_growth_env viability).
        // Knobs declared near plant_stride. Budgeted + strided; default ON.
        if flora_spread_enabled && mouse_captured && frame_count % flora_spread_interval == 11 {
            use unendlich_engine::sim::flora_spread as fs;
            use unendlich_engine::sim::plant_growth_env::Conditions;
            use unendlich_engine::world::blocks::BlockId as B;
            let pp = local_player.view.position;
            let (pxi, pyi, pzi) = (pp.x.floor() as i32, pp.y.floor() as i32, pp.z.floor() as i32);
            let m = chunk_manager.model_arc();
            let wtime = start.elapsed().as_secs_f64();
            let r = flora_spread_radius.max(2);
            let base_seed = (frame_count as u64)
                .wrapping_mul(0x9E37_79B9_7F4A_7C15)
                ^ ((pxi as i64 as u64) << 20)
                ^ (pzi as i64 as u64);
            // 1) gather candidate parent flora cells via budgeted random probes around the player.
            let mut parents: Vec<(i32, i32, i32, B)> = Vec::new();
            for s in 0..flora_spread_scan as u64 {
                if parents.len() >= flora_spread_budget as usize * 2 {
                    break;
                }
                let h = unendlich_engine::util::hash_util::mix64(base_seed ^ s.wrapping_mul(0xD1B5_4A32));
                let ox = (h % (2 * r as u64 + 1)) as i32 - r;
                let oz = ((h >> 21) % (2 * r as u64 + 1)) as i32 - r;
                let (bx, bz) = (pxi + ox, pzi + oz);
                for dy in (-8..=10).rev() {
                    let by = pyi + dy;
                    let blk = chunk_manager.block_at(bx, by, bz);
                    if matches!(blk, B::Flower | B::TallGrass | B::Fern | B::Mushroom) {
                        parents.push((bx, by, bz, blk));
                        break;
                    }
                }
            }
            // 2) each parent disperses downwind; viable seeds land on suitable bare ground.
            let mut placed = 0u32;
            for (i, (bx, by, bz, blk)) in parents.iter().enumerate() {
                if placed >= flora_spread_budget {
                    break;
                }
                let moisture =
                    (m.humidity(*bx as f64, *by as f64, *bz as f64, 0.0) as f32).clamp(0.0, 1.0);
                let temp_c = m.temperature(*bx as f64, *by as f64, *bz as f64, 0.0) as f32;
                let mut blocked = 0i32;
                for up in 1..=16 {
                    if !matches!(chunk_manager.block_at(*bx, *by + up, *bz), B::Air) {
                        blocked += 1;
                    }
                }
                let light01 = (1.0 - blocked as f32 / 16.0).clamp(0.0, 1.0);
                let cond = Conditions { moisture01: moisture, light01, temp_c, season01: current_season01 };
                let (tol, ground_ok): (_, fn(B) -> bool) = match blk {
                    B::Mushroom => (
                        fs::mushroom_tolerance(),
                        |b| matches!(b, B::Dirt | B::Stone | B::Moss | B::Gravel),
                    ),
                    B::Flower => {
                        (fs::flower_tolerance(), |b| matches!(b, B::Grass | B::Moss | B::Dirt))
                    }
                    _ => (
                        fs::ground_cover_tolerance(),
                        |b| matches!(b, B::Grass | B::Moss | B::Dirt),
                    ),
                };
                let (wx, wz, _) =
                    unendlich_engine::wind::wind_at(*bx as f64, *by as f64, *bz as f64, wtime);
                let pseed = base_seed ^ (i as u64).wrapping_mul(0x0100_0001_B3);
                let targets = fs::spread_targets(
                    [*bx as f32 + 0.5, *bz as f32 + 0.5],
                    &tol,
                    &cond,
                    [wx, wz],
                    pseed,
                    4,
                );
                for t in targets {
                    if placed >= flora_spread_budget {
                        break;
                    }
                    let (tx, tz) = (t[0], t[1]);
                    for dy in (-5..=5).rev() {
                        let ay = *by + dy;
                        let here = chunk_manager.block_at(tx, ay, tz);
                        let below = chunk_manager.block_at(tx, ay - 1, tz);
                        if matches!(here, B::Air) && ground_ok(below) {
                            // r578: SPACING / COMPETITION — don't carpet every cell. Count
                            // the plant neighbours around this spot (8 at the same height);
                            // if it's already crowded, leave a GAP instead of filling solid.
                            // Keeps the world lush but patchy and WALKABLE, and makes the
                            // spread self-limiting (it stops pushing into dense patches).
                            let mut neigh = 0i32;
                            for (dx, dz) in [
                                (-1, 0), (1, 0), (0, -1), (0, 1),
                                (-1, -1), (-1, 1), (1, -1), (1, 1),
                            ] {
                                if matches!(
                                    chunk_manager.block_at(tx + dx, ay, tz + dz),
                                    B::Flower | B::TallGrass | B::Fern | B::Mushroom
                                ) {
                                    neigh += 1;
                                }
                            }
                            if neigh >= 3 {
                                break; // crowded — keep a walkable gap here
                            }
                            if chunk_manager.set_block(tx, ay, tz, *blk) {
                                placed += 1;
                            }
                            break;
                        }
                    }
                }
            }
            if placed > 0 {
                log::info!("[flora] {} new plant(s) took root nearby", placed);
            }
        }
        // r452: SAPLING GROWTH — advance living session.saplings via sim::plant_growth (env from the climate
        // model); mature ones become real trees (flora_blocks). Occasionally plant a new sapling.
        if flora_spread_enabled && mouse_captured && frame_count % sapling_grow_interval == 37 {
            use unendlich_engine::sim::plant_growth as pg;
            use unendlich_engine::world::blocks::BlockId as B;
            use unendlich_engine::world::flora_placement::FloraSpecies;
            let m = chunk_manager.model_arc();
            let pp = local_player.view.position;
            let (pxi, pyi, pzi) = (pp.x.floor() as i32, pp.y.floor() as i32, pp.z.floor() as i32);
            // 1) ADVANCE each sapling — OakLog cells in the persistent flora store. r519: saplings
            //    were migrated out of the in-memory session.saplings into `flora`, so they now
            //    persist across save/load like crops. Crops share the store but are advanced by the
            //    farming tick (filtered out here by block type).
            let oak = B::OakLog as u8;
            let mut matured: Vec<[i32; 3]> = Vec::new();
            let mut invalid: Vec<[i32; 3]> = Vec::new();
            for (pos_ref, cell) in session.flora.iter_mut() {
                if cell.block != oak {
                    continue;
                }
                let pos = *pos_ref;
                if !matches!(chunk_manager.block_at(pos[0], pos[1], pos[2]), B::OakLog) {
                    invalid.push(pos);
                    continue;
                }
                let h = (m.humidity(pos[0] as f64, pos[1] as f64, pos[2] as f64, 0.0) as f32)
                    .clamp(0.0, 1.0);
                let temp_c = m.temperature(pos[0] as f64, pos[1] as f64, pos[2] as f64, 0.0) as f32;
                let r = (temp_c / 30.0).clamp(0.0, 1.0);
                let mut blocked = 0i32;
                for up in 1..=12 {
                    if !matches!(chunk_manager.block_at(pos[0], pos[1] + up, pos[2]), B::Air) {
                        blocked += 1;
                    }
                }
                let l = (1.0 - blocked as f32 / 12.0).clamp(0.0, 1.0);
                let st = pg::PlantState {
                    growth01: cell.growth01, stage: cell.stage, element: cell.element, vigor: cell.vigor,
                };
                let step = pg::advance(st, &pg::Fields { h, r, l, s: 0.0, b: 0.0 }, sapling_dt);
                cell.growth01 = step.state.growth01;
                cell.stage = step.state.stage;
                cell.element = step.state.element;
                cell.vigor = step.state.vigor;
                if step.event == pg::GrowthEvent::Matured || cell.stage >= pg::FINAL_STAGE {
                    matured.push(pos);
                }
            }
            if !matured.is_empty() || !invalid.is_empty() {
                session.flora.mark_dirty();
            }
            // 2) grow matured saplings into trees.
            for &pos in &matured {
                // r578: COMPETITION — a sapling that matured while an established tree
                // grew up close to it loses the contest for light/space: it does NOT
                // become a tree (it's removed below). Combined with the spacing gate at
                // planting, dense patches thin toward well-spaced canopies over time.
                let spacing = 6i32;
                let mut crowded = false;
                'm: for ddz in -spacing..=spacing {
                    for ddx in -spacing..=spacing {
                        if ddx == 0 && ddz == 0 {
                            continue; // skip the sapling's own column
                        }
                        if ddx * ddx + ddz * ddz > spacing * spacing {
                            continue;
                        }
                        for hy in 0..=4 {
                            if matches!(
                                chunk_manager.block_at(pos[0] + ddx, pos[1] + hy, pos[2] + ddz),
                                B::OakLog | B::BirchLog
                            ) {
                                crowded = true;
                                break 'm;
                            }
                        }
                    }
                }
                if crowded {
                    continue; // lost the competition — removed below, no tree grown
                }
                let temp = m.temperature(pos[0] as f64, pos[1] as f64, pos[2] as f64, 0.0) as f32;
                let species = if temp < 5.0 { FloraSpecies::Conifer } else { FloraSpecies::Broadleaf };
                let mut placed = 0u32;
                for (cell, blk) in
                    unendlich_engine::world::flora_blocks::flora_to_blocks(species, pos)
                {
                    if chunk_manager.set_block(cell[0], cell[1], cell[2], blk) {
                        placed += 1;
                    }
                }
                if placed > 0 {
                    log::info!("[flora] a sapling matured into a tree ({placed} blocks)");
                }
            }
            // 3) remove matured + invalid saplings from the store.
            for pos in matured.iter().chain(invalid.iter()) {
                session.flora.remove(*pos);
            }
            // 4) occasionally plant a new sapling on open grass/dirt near the player.
            let sapling_count = session.flora.iter().filter(|(_, c)| c.block == oak).count();
            if sapling_count < sapling_cap {
                let hh = unendlich_engine::util::hash_util::mix64(
                    (frame_count as u64).wrapping_mul(0xA24B_9D7F)
                        ^ ((pxi as i64 as u64) << 16)
                        ^ (pzi as i64 as u64),
                );
                if hh % 100 < sapling_plant_pct {
                    let rad = 18u64;
                    let ox = (hh % (2 * rad + 1)) as i32 - rad as i32;
                    let oz = ((hh >> 20) % (2 * rad + 1)) as i32 - rad as i32;
                    let (bx, bz) = (pxi + ox, pzi + oz);
                    for dy in (-12..=12).rev() {
                        let ay = pyi + dy;
                        let here = chunk_manager.block_at(bx, ay, bz);
                        let below = chunk_manager.block_at(bx, ay - 1, bz);
                        if matches!(here, B::Air) && matches!(below, B::Grass | B::Dirt) {
                            // r578: TREE COMPETITION (spacing) — a sapling only takes root
                            // in OPEN ground. Scan a radius for existing trunks; if a tree is
                            // already close, skip, so trees end up spaced apart instead of
                            // clumping into impassable thickets. ~6-block spacing.
                            let spacing = 6i32;
                            let mut crowded = false;
                            'scan: for ddz in -spacing..=spacing {
                                for ddx in -spacing..=spacing {
                                    if ddx * ddx + ddz * ddz > spacing * spacing {
                                        continue;
                                    }
                                    for hy in 0..=3 {
                                        if matches!(
                                            chunk_manager.block_at(bx + ddx, ay + hy, bz + ddz),
                                            B::OakLog | B::BirchLog
                                        ) {
                                            crowded = true;
                                            break 'scan;
                                        }
                                    }
                                }
                            }
                            if crowded {
                                break; // a tree already owns this spot — no new sapling
                            }
                            let clear =
                                (1..=6).all(|u| matches!(chunk_manager.block_at(bx, ay + u, bz), B::Air));
                            if clear && chunk_manager.set_block(bx, ay, bz, B::OakLog) {
                                let mut fc = unendlich_engine::world::flora_store::FloraCell::sown(
                                    oak, frame_count,
                                );
                                fc.element = ((hh >> 40) % 6) as u8;
                                fc.vigor = 0.4 + ((hh >> 48) % 100) as f32 / 250.0;
                                session.flora.plant([bx, ay, bz], fc);
                            }
                            break;
                        }
                    }
                }
            }
        }
        // r512/r514: FARMING — advance planted crops in the persistent flora store via
        // sim::plant_growth (climate-driven, like saplings). Ripe at FINAL_STAGE; crops whose
        // host block was removed/changed drop out of tracking. Mutated growth is marked dirty so
        // it gets saved. Runs in both modes.
        if mouse_captured && frame_count % 90 == 41 && !session.flora.is_empty() {
            use unendlich_engine::sim::plant_growth as pg;
            let m = chunk_manager.model_arc();
            let mut remove: Vec<[i32; 3]> = Vec::new();
            let mut ripened: Vec<[i32; 3]> = Vec::new();
            for (pos_ref, cell) in session.flora.iter_mut() {
                // r519: only crops grow here — saplings (OakLog cells) share the store but are
                // advanced by the sapling tick above.
                if !matches!(
                    cell.block,
                    c if c == BlockId::Carrot as u8
                        || c == BlockId::Potato as u8
                        || c == BlockId::Cabbage as u8
                        || c == BlockId::Sprout as u8
                ) {
                    continue;
                }
                let pos = *pos_ref;
                if chunk_manager.block_at(pos[0], pos[1], pos[2]) as u8 != cell.block {
                    remove.push(pos);
                    continue;
                }
                let h = (m.humidity(pos[0] as f64, pos[1] as f64, pos[2] as f64, 0.0) as f32)
                    .clamp(0.0, 1.0);
                let temp_c = m.temperature(pos[0] as f64, pos[1] as f64, pos[2] as f64, 0.0) as f32;
                let r = (temp_c / 30.0).clamp(0.0, 1.0);
                let mut blocked = 0i32;
                for up in 1..=8 {
                    if !matches!(chunk_manager.block_at(pos[0], pos[1] + up, pos[2]), BlockId::Air) {
                        blocked += 1;
                    }
                }
                let l = (1.0 - blocked as f32 / 8.0).clamp(0.0, 1.0);
                let was_ripe = cell.stage >= pg::FINAL_STAGE;
                let st = pg::PlantState {
                    growth01: cell.growth01,
                    stage: cell.stage,
                    element: cell.element,
                    vigor: cell.vigor,
                };
                // r522: the seed's genome sets its growth pace — a high growth_rate gene ripens
                // noticeably faster, so bred lines pay off in time as well as yield.
                let g = unendlich_engine::sim::plant_tree_genetics::PlantGenome::from_array(cell.genome);
                let dt = 0.5 * (0.6 + g.growth_rate * 0.8);
                let step = pg::advance(st, &pg::Fields { h, r, l, s: 0.0, b: 0.0 }, dt);
                cell.growth01 = step.state.growth01;
                cell.stage = step.state.stage;
                cell.vigor = step.state.vigor;
                // (element is preserved — for a Sprout it encodes the target crop, 0/1/2.)
                // r520: once a seedling is established (stage ≥ 2) it becomes its real crop block,
                // so the field visibly progresses sprout → plant → ripe.
                if cell.block == BlockId::Sprout as u8 && cell.stage >= 2 {
                    let crop = match cell.element {
                        1 => BlockId::Potato,
                        2 => BlockId::Cabbage,
                        _ => BlockId::Carrot,
                    };
                    if chunk_manager.set_block(pos[0], pos[1], pos[2], crop) {
                        cell.block = crop as u8;
                        log::info!("[farm] a seedling at ({},{},{}) grew into {:?}", pos[0], pos[1], pos[2], crop);
                    }
                }
                if !was_ripe && cell.stage >= pg::FINAL_STAGE {
                    ripened.push(pos);
                }
            }
            for p in &remove {
                session.flora.remove(*p);
            }
            session.flora.mark_dirty();
            for p in ripened {
                log::info!("[farm] crop at ({},{},{}) is ripe — harvest it for a yield", p[0], p[1], p[2]);
            }
        }
        // r514: persist the flora store to disk on a cadence when it has unsaved changes
        // (planted / harvested / grown). Atomic write; cheap because the store is sparse.
        if frame_count % 1800 == 137 && session.flora.take_dirty() {
            if let Err(e) = session.flora.save(&flora_root) {
                log::warn!("[farm] failed to save flora state: {}", e);
            } else {
                log::debug!("[farm] persisted {} crop(s)", session.flora.len());
            }
        }
        // r455: PIONEER FLORA — climate-driven decoration variety (sim::pioneer_flora) using blocks
        // that already render: dead bushes on arid sand, lichen on cold rock, moss on damp rock, ferns
        // in humid undergrowth. Runtime + recoverable; shares the flora_spread enable. Budgeted probes.
        if flora_spread_enabled && mouse_captured && frame_count % flora_spread_interval == 53 {
            use unendlich_engine::sim::pioneer_flora::pioneer_for;
            use unendlich_engine::world::blocks::BlockId as B;
            let m = chunk_manager.model_arc();
            let pp = local_player.view.position;
            let (pxi, pyi, pzi) = (pp.x.floor() as i32, pp.y.floor() as i32, pp.z.floor() as i32);
            let base = unendlich_engine::util::hash_util::mix64(
                (frame_count as u64).wrapping_mul(0x51ED_270B)
                    ^ ((pxi as i64 as u64) << 8)
                    ^ (pzi as i64 as u64),
            );
            let mut placed = 0u32;
            for s in 0..16u64 {
                if placed >= 3 {
                    break;
                }
                let h = unendlich_engine::util::hash_util::mix64(base ^ s.wrapping_mul(0x9E37_79B9));
                let r = 22u64;
                let ox = (h % (2 * r + 1)) as i32 - r as i32;
                let oz = ((h >> 20) % (2 * r + 1)) as i32 - r as i32;
                let (bx, bz) = (pxi + ox, pzi + oz);
                for dy in (-12..=12).rev() {
                    let ay = pyi + dy;
                    let here = chunk_manager.block_at(bx, ay, bz);
                    let below = chunk_manager.block_at(bx, ay - 1, bz);
                    if matches!(here, B::Air) && below.is_solid() {
                        let temp = m.temperature(bx as f64, ay as f64, bz as f64, 0.0) as f32;
                        let moist = (m.humidity(bx as f64, ay as f64, bz as f64, 0.0) as f32)
                            .clamp(0.0, 1.0);
                        if let Some(deco) = pioneer_for(temp, moist, below) {
                            if chunk_manager.set_block(bx, ay, bz, deco) {
                                placed += 1;
                            }
                        }
                        break;
                    }
                }
            }
            if placed > 0 {
                log::info!("[flora] {placed} pioneer plant(s) colonised bare ground");
            }
        }
        // r516: CAVE ENRICHMENT — seed ore into exposed cave stone near the player. The GPU
        // world generator can't be edited here, so (like pioneer flora) this is a budgeted CPU
        // pass: probe random cells underground, and where plain Stone faces Air (a cave wall)
        // below sea level, depth-weighted sim::cave_ore picks an ore (rare; richer deeper). The
        // CPU mesher renders it, so it's visible regardless of the worldgen backend.
        if mouse_captured && frame_count % 67 == 17 {
            use unendlich_engine::world::blocks::BlockId as B;
            let sea = unendlich_engine::constants::SEA_LEVEL as i32;
            let pp = local_player.view.position;
            let (pxi, pyi, pzi) = (pp.x.floor() as i32, pp.y.floor() as i32, pp.z.floor() as i32);
            // Only bother when the player is below the surface (in/near caves).
            if pyi < sea - 3 {
                let base = unendlich_engine::util::hash_util::mix64(
                    (frame_count as u64).wrapping_mul(0x2545_F491)
                        ^ ((pxi as i64 as u64) << 8)
                        ^ (pzi as i64 as u64),
                );
                let mut placed = 0u32;
                for s in 0..20u64 {
                    if placed >= 2 {
                        break;
                    }
                    let h = unendlich_engine::util::hash_util::mix64(base ^ s.wrapping_mul(0x9E37_79B9));
                    let r = 14u64;
                    let ox = (h % (2 * r + 1)) as i32 - r as i32;
                    let oy = ((h >> 16) % (2 * r + 1)) as i32 - r as i32;
                    let oz = ((h >> 32) % (2 * r + 1)) as i32 - r as i32;
                    let (bx, by, bz) = (pxi + ox, pyi + oy, pzi + oz);
                    if chunk_manager.block_at(bx, by, bz) != B::Stone {
                        continue;
                    }
                    // Must be a cave wall: at least one face-neighbor is Air (exposed surface).
                    let exposed = matches!(chunk_manager.block_at(bx + 1, by, bz), B::Air)
                        || matches!(chunk_manager.block_at(bx - 1, by, bz), B::Air)
                        || matches!(chunk_manager.block_at(bx, by + 1, bz), B::Air)
                        || matches!(chunk_manager.block_at(bx, by - 1, bz), B::Air)
                        || matches!(chunk_manager.block_at(bx, by, bz + 1), B::Air)
                        || matches!(chunk_manager.block_at(bx, by, bz - 1), B::Air);
                    if !exposed {
                        continue;
                    }
                    let depth = sea - by;
                    let roll = ((h >> 48) & 0xFFFF) as f32 / 65535.0;
                    if let Some(ore) = unendlich_engine::sim::cave_ore::ore_for(depth, roll) {
                        if chunk_manager.set_block(bx, by, bz, ore) {
                            placed += 1;
                        }
                    }
                }
                if placed > 0 {
                    water_dirty = true;
                    log::debug!("[cave] enriched {placed} cave wall(s) with ore");
                }
            }
        }
        // r471: quench sites collected this frame → injected into the smoke field as steam puffs below.
        let mut pending_steam: Vec<[i32; 3]> = Vec::new();
        // r464: HOT METAL — metal cells beside lava heat up (sim::hot_metal), quench (steam) in water,
        // and melt to lava at their melting point. Bounded discovery scan + cheap evolution of tracked
        // cells; melt is a one-time set_block (no per-tick remeshing — keeps the T1 hitch budget clean).
        if hot_metal_enabled && mouse_captured && frame_count % 20 == 7 {
            use unendlich_engine::sim::hot_metal as hm;
            use unendlich_engine::world::blocks::BlockId;
            let tp = hm::ThermalParams::default();
            let dt = 20.0 / 60.0;
            let pp = local_player.view.position;
            let (px, py, pz) = (pp.x.floor() as i32, pp.y.floor() as i32, pp.z.floor() as i32);
            let is_metal = |b: BlockId| {
                matches!(b, BlockId::IronBlock | BlockId::GoldBlock | BlockId::CopperOre
                    | BlockId::IronOre | BlockId::GoldOre)
            };
            let src_temp = |b: BlockId| if b == BlockId::Lava { hm::LAVA_C } else { hm::AMBIENT_C };
            const NB: [(i32, i32, i32); 6] =
                [(1, 0, 0), (-1, 0, 0), (0, 1, 0), (0, -1, 0), (0, 0, 1), (0, 0, -1)];
            // DISCOVERY: bounded scan for metal adjacent to lava (only while under the cell cap).
            const R: i32 = 6;
            if session.hot_metal.len() < 96 {
                'scan: for dx in -R..=R {
                    for dy in -R..=R {
                        for dz in -R..=R {
                            let c = [px + dx, py + dy, pz + dz];
                            if session.hot_metal.contains_key(&c) { continue; }
                            if !is_metal(chunk_manager.block_at(c[0], c[1], c[2])) { continue; }
                            let mut hot = hm::AMBIENT_C;
                            for (nx, ny, nz) in NB {
                                hot = hot.max(src_temp(chunk_manager.block_at(c[0] + nx, c[1] + ny, c[2] + nz)));
                            }
                            if hot > hm::AMBIENT_C {
                                session.hot_metal.insert(c, hm::AMBIENT_C);
                                if session.hot_metal.len() >= 96 { break 'scan; }
                            }
                        }
                    }
                }
            }
            // EVOLUTION: step every tracked cell; quench / melt / cool-out.
            let cells: Vec<[i32; 3]> = session.hot_metal.keys().copied().collect();
            let (mut quenched, mut melted, mut hottest) = (0u32, 0u32, hm::AMBIENT_C);
            for c in cells {
                if !is_metal(chunk_manager.block_at(c[0], c[1], c[2])) { session.hot_metal.remove(&c); continue; }
                let (mut hot, mut wet) = (hm::AMBIENT_C, false);
                for (nx, ny, nz) in NB {
                    let nb = chunk_manager.block_at(c[0] + nx, c[1] + ny, c[2] + nz);
                    hot = hot.max(src_temp(nb));
                    if nb == BlockId::Water { wet = true; }
                }
                let t0 = *session.hot_metal.get(&c).unwrap();
                let t1 = hm::step_metal(t0, hot, wet, dt, tp);
                hottest = hottest.max(t1);
                if hm::quench_steam(t1, wet) { quenched += 1; pending_steam.push(c); }
                if hm::is_molten(t1) {
                    chunk_manager.set_block(c[0], c[1], c[2], BlockId::Lava);
                    session.hot_metal.remove(&c);
                    melted += 1;
                } else if t1 <= hm::AMBIENT_C + 2.0 && hot <= hm::AMBIENT_C {
                    session.hot_metal.remove(&c);
                } else {
                    session.hot_metal.insert(c, t1);
                }
            }
            if (quenched > 0 || melted > 0) && frame_count % 120 < 20 {
                log::info!(
                    "[hot-metal] {} cell(s) hot (max {:.0}°C), quench-steam {}, melted→lava {}",
                    session.hot_metal.len(), hottest, quenched, melted
                );
            }
        }
        // r143: ECOSYSTEM CENSUS — every ~2 min, the herd composition by
        // kind + bond, predators vs prey, drops afield. One log line.
        if frame_count % 7200 == 3600 && !roamers.is_empty() {
            let mut kinds = std::collections::HashMap::new();
            let mut tamed = 0usize;
            for r in &roamers {
                *kinds.entry(format!("{:?}", r.kind)).or_insert(0usize) += 1;
                if matches!(r.bond, unendlich_engine::sim::bond_tame::Bond::Tamed) {
                    tamed += 1;
                }
            }
            let mut parts: Vec<String> =
                kinds.into_iter().map(|(k, n)| format!("{k} x{n}")).collect();
            parts.sort();
            log::info!(
                "[census] herd {}: {} | tamed {} | capacity {} | drops afield {}",
                roamers.len(),
                parts.join(", "),
                tamed,
                fg_colony.population().max(8),
                fg_app.drops().len(),
            );
        }
        // r140: NATURAL SPAWNING — when the player enters a new 64-block
        // region, sim::spawn::spawn_region (deterministic in world seed +
        // region coords; dry-land surface sites only) decides who lives
        // there; we hatch real Roamers (Grazer->Fauna, Seeker->Dinosaur),
        // capped by the popdyn carrying capacity. Explore = encounter.
        if frame_count % 60 == 30 && mouse_captured {
            let pp = local_player.view.position;
            let (rx, rz) = (
                (pp.x.floor() as i32).div_euclid(unendlich_engine::sim::spawn::REGION_SIZE),
                (pp.z.floor() as i32).div_euclid(unendlich_engine::sim::spawn::REGION_SIZE),
            );
            for (drx, drz) in [(0, 0), (1, 0), (-1, 0), (0, 1), (0, -1)] {
                let key = (rx + drx, rz + drz);
                if session.spawned_regions.contains(&key) {
                    continue;
                }
                session.spawned_regions.insert(key);
                let cap = fg_colony.population().max(8).min(ROAMER_CAP);
                if roamers.len() >= cap {
                    continue;
                }
                let gen = chunk_manager.generator().cpu();
                let mobs = unendlich_engine::sim::spawn::spawn_region(
                    gen,
                    fg_world_seed,
                    key.0,
                    key.1,
                );
                let mut hatched = 0usize;
                for m in mobs {
                    if roamers.len() >= cap {
                        break;
                    }
                    let kind = match m.kind {
                        unendlich_engine::sim::mob::MobKind::Seeker => {
                            unendlich_engine::item::CreatureKind::Dinosaur
                        }
                        _ => unendlich_engine::item::CreatureKind::Fauna,
                    };
                    let sx = m.body.pos.x.to_f32();
                    let sy = m.body.pos.y.to_f32();
                    let sz = m.body.pos.z.to_f32();
                    let wseed = (fg_world_seed)
                        ^ ((sx as i32 as u32 as u64) << 20)
                        ^ (sz as i32 as u32 as u64);
                    let mut cr = unendlich_engine::spawn::spawn(kind, [sx, sy, sz]);
                    cr.body.position[1] = cr.grounded_y(sy);
                    let genome = unendlich_engine::genetics::Genome::from_seed(wseed);
                    let ph = genome.express(wseed, 0);
                    // r462: pick a biome-appropriate wild species from the roster for generic Fauna.
                    let picked_species = if matches!(kind, unendlich_engine::item::CreatureKind::Fauna) {
                        let _b = chunk_manager.generator().cpu().biome_at_3d(sx as i32, sy as i32, sz as i32).biome;
                        let _roll = ((wseed >> 16) & 0xffff) as f32 / 65535.0;
                        let _s = unendlich_engine::sim::fauna_roster::pick_for_biome(_b, _roll);
                        if let Some(sp) = _s { log::info!("[fauna] {} appears ({:?})", unendlich_engine::sim::fauna_roster::data(sp).name, _b); }
                        _s
                    } else { None };
                    let spawn_hp = picked_species.map(|s| unendlich_engine::sim::fauna_roster::data(s).base_health as i32).unwrap_or(40);
                    roamers.push(Roamer {
                        bond: unendlich_engine::sim::bond_tame::Bond::Wild,
                        trust: 0.0,
                        hp: spawn_hp,
                        creature: cr,
                        wander: unendlich_engine::sim::cognition::wander::Wander::new(
                            wseed,
                            0.8 + ph.speed * 1.4,
                        ),
                        home: (
                            (sx.floor() as i32).div_euclid(unendlich_engine::constants::CHUNK_SIZE),
                            (sz.floor() as i32).div_euclid(unendlich_engine::constants::CHUNK_SIZE),
                        ),
                        persistent: false,
                        senses: unendlich_engine::sim::senses::senses_for(kind, &genome, wseed, 0),
                        turn_rate: 1.5 + ph.turn_rate * 4.0,
                        turn_profile: unendlich_engine::sim::cognition::steering::TurnProfile::from_seed(wseed),
                        vitality: unendlich_engine::sim::feeding::Vitality::full(),
                        sex: unendlich_engine::genetics::sex_from_seed(wseed),
                        kind,
                        species: picked_species,
                        genome,
                        forage_memory: unendlich_engine::sim::forage_memory::ForageMemory::new(16),
                        leap: unendlich_engine::sim::leap::Leap::from_genes(ph.speed, ph.speed),
                        airborne: None,
                        posture: unendlich_engine::sim::posture::Posture::new(0.0),
                        stumble: unendlich_engine::sim::stumble::Stumble::new(),
                        flying: false,
                        flight_phase: (wseed % 30) as f32,
                        last_tick_frame: 0,
                        aggro: 0.0,
                        prev_behavior: 0,
                        combat_habit: 0.0,
                        disposition_cache: None,
                        graze_target: None,
                        graze_cell: (i32::MIN, i32::MIN),
                        graze_scan_tick: 0,
                    });
                    hatched += 1;
                }
                if hatched > 0 {
                    log::info!(
                        "[spawn] region ({},{}) populated: {} creature(s), herd {}",
                        key.0, key.1, hatched, roamers.len()
                    );
                }
            }
        }
        // r137: WORLD-SEAM EXERCISER — the orphan fns that need REAL state
        // (&Chunk under the player; the live &WorldGenerator) and can't be
        // registry-synthesized. Runs 1/240 frames; one family per window,
        // each call quarantined so a panic logs once and skips next time.
        if frame_count % 240 == 120 {
            use std::panic::{catch_unwind, AssertUnwindSafe};
            static SEAM_BROKEN: [std::sync::atomic::AtomicBool; 6] = [
                std::sync::atomic::AtomicBool::new(false),
                std::sync::atomic::AtomicBool::new(false),
                std::sync::atomic::AtomicBool::new(false),
                std::sync::atomic::AtomicBool::new(false),
                std::sync::atomic::AtomicBool::new(false),
                std::sync::atomic::AtomicBool::new(false),
            ];
            use std::sync::atomic::Ordering::Relaxed;
            let pp = local_player.view.position;
            let coord = unendlich_engine::world::chunk::ChunkCoord {
                x: (pp.x.floor() as i32).div_euclid(unendlich_engine::constants::CHUNK_SIZE),
                y: (pp.y.floor() as i32).div_euclid(unendlich_engine::constants::CHUNK_SIZE),
                z: (pp.z.floor() as i32).div_euclid(unendlich_engine::constants::CHUNK_SIZE),
            };
            let which = ((frame_count / 240) % 6) as usize;
            if !SEAM_BROKEN[which].load(Relaxed) {
                let ok = catch_unwind(AssertUnwindSafe(|| {
                    match which {
                        0 => {
                            if let Some(ch) = chunk_manager.get_chunk(coord) {
                                let _ = std::hint::black_box(
                                    unendlich_engine::world::surface::extract_surface(&ch),
                                );
                            }
                        }
                        1 => {
                            if let Some(ch) = chunk_manager.get_chunk(coord) {
                                let _ = std::hint::black_box(
                                    unendlich_engine::world::surface::extract_surface_runs(&ch),
                                );
                            }
                        }
                        2 => {
                            if let Some(ch) = chunk_manager.get_chunk(coord) {
                                let _ = std::hint::black_box(
                                    unendlich_engine::world::water_field::ground_heights_from_chunk(
                                        &ch,
                                        BlockId::Water,
                                    ),
                                );
                            }
                        }
                        3 => {
                            let g = chunk_manager.generator().cpu();
                            let _ = std::hint::black_box(
                                unendlich_engine::world::flora_map::region_flora(
                                    g,
                                    pp.x as i32 - 8,
                                    pp.z as i32 - 8,
                                    16,
                                    16,
                                ),
                            );
                        }
                        4 => {
                            let g = chunk_manager.generator().cpu();
                            let _ = std::hint::black_box(
                                unendlich_engine::sim::spawn::spawn_region(
                                    g,
                                    0xC0FFEE,
                                    pp.x as i32,
                                    pp.z as i32,
                                ),
                            );
                        }
                        _ => {
                            if let Some(ch) = chunk_manager.get_chunk(coord) {
                                let _ = std::hint::black_box(
                                    unendlich_engine::render::mesher::mesh_chunk_naive(&ch),
                                );
                            }
                        }
                    }
                }))
                .is_ok();
                if !ok {
                    SEAM_BROKEN[which].store(true, Relaxed);
                    log::warn!("[seams] world-seam family {} panicked — quarantined", which);
                }
            }
        }
        // r129: smoke step — field origin = spawn-centered 64x48x64 volume.
        if !session.fire_positions.is_empty() || !pending_steam.is_empty() {
            let org = [spawn_pos.x - 32.0, spawn_pos.y - 8.0, spawn_pos.z - 32.0];
            let mut fires: Vec<unendlich_engine::sim::smoke_gpu::FireSource> = session.fire_positions
                .iter()
                .filter_map(|f| {
                    let cx = ((f[0] - org[0]) / 2.0) as i32;
                    let cy = ((f[1] - org[1]) / 2.0) as i32;
                    let cz = ((f[2] - org[2]) / 2.0) as i32;
                    if (0..32).contains(&cx) && (0..24).contains(&cy) && (0..32).contains(&cz) {
                        Some(unendlich_engine::sim::smoke_gpu::FireSource {
                            cell: smoke_field.idx(cx as usize, cy as usize, cz as usize),
                            intensity: 1.0,
                        })
                    } else {
                        None
                    }
                })
                .collect();
            // r471: hot metal quenched in water flashes a brief steam puff into the smoke field
            // (lighter than fire smoke — intensity 0.6). Reuses the smoke advection so the r464
            // hot-metal sim gets a visible cue without a new particle system.
            for s in &pending_steam {
                let cx = ((s[0] as f32 + 0.5 - org[0]) / 2.0) as i32;
                let cy = ((s[1] as f32 + 0.5 - org[1]) / 2.0) as i32;
                let cz = ((s[2] as f32 + 0.5 - org[2]) / 2.0) as i32;
                if (0..32).contains(&cx) && (0..24).contains(&cy) && (0..32).contains(&cz) {
                    fires.push(unendlich_engine::sim::smoke_gpu::FireSource {
                        cell: smoke_field.idx(cx as usize, cy as usize, cz as usize),
                        intensity: 0.6,
                    });
                }
            }
            smoke_field = unendlich_engine::sim::smoke_gpu::step_smoke(&smoke_field, &smoke_params, dt, &fires);
            let pp = local_player.view.position;
            let d = smoke_field.sample(
                (pp.x - org[0]) / 2.0,
                (pp.y - org[1]) / 2.0,
                (pp.z - org[2]) / 2.0,
            );
            let now_in = d > 0.15;
            if now_in != in_smoke {
                in_smoke = now_in;
                log::info!("[smoke] {} the smokescreen (density {:.2})",
                    if now_in { "entered" } else { "left" }, d);
            }
        }

        // r118: gameplay services — stamina + ground-item pickup. Gated on the
        // bin's REAL play state (mouse captured, no menu/pause), not App's FSM,
        // because this bin enters the world without driving AppState::InGame.
        if mouse_captured && !game_paused {
            let sprint_urge = bindings.is_action_pressed(
                unendlich_engine::keybindings::GameAction::Sprint, &input);
            let _stam_mult = fg_app.stamina_mut().tick(sprint_urge, dt);
            // (r160 audit: the multiplier is redundant — the r118 sprint
            // gate at the speed site already applies SPRINT_MULTIPLIER off
            // the same meter. The NEW behavior is the winded penalty below.)
            let ppos = [
                local_player.view.position.x,
                local_player.view.position.y,
                local_player.view.position.z,
            ];
            // r121 (worker Phase 4 port): swimming detection via a REAL block
            // query at the player's feet (not the y<64 guess), then hunger drain
            // (2x while swimming), and V to eat (E/Q/R are all bound already).
            let feet = [
                local_player.view.position.x.floor() as i32,
                (local_player.view.position.y - 1.2).floor() as i32,
                local_player.view.position.z.floor() as i32,
            ];
            // r173: LAVA BURNS — standing in lava is a survival emergency:
            // heavy vitality chip + the body thermal spikes toward heat
            // stroke (the r142 exposure model then drains on top). The 0.20
            // motion factor (already live) makes escape itself the challenge.
            let lpv = local_player.view.position;
            let in_lava = matches!(
                chunk_manager.block_at(
                    lpv.x.floor() as i32,
                    lpv.y.floor() as i32,
                    lpv.z.floor() as i32,
                ),
                unendlich_engine::world::blocks::BlockId::Lava
            ) || matches!(
                chunk_manager.block_at(
                    lpv.x.floor() as i32,
                    (lpv.y - 1.2).floor() as i32,
                    lpv.z.floor() as i32,
                ),
                unendlich_engine::world::blocks::BlockId::Lava
            );
            if in_lava && survival_mode {
                // r177: FIRE ATTUNEMENT (imbued artifact, element Fire,
                // counter > 0.5) reads the vents like a Ryujin Carrier —
                // burn and heat-spike at quarter rate. Deep mining opens up.
                let fire_attuned = fg_imbue.element == 4 && fg_imbue.counter > 0.5;
                let burn = if fire_attuned { 0.45 * 0.25 } else { 0.45 };
                let spike = if fire_attuned { 1.5 } else { 6.0 };
                fg_app.vitality_mut().tick(dt, burn);
                fg_thermal.body_c = (fg_thermal.body_c + spike * dt).min(43.0);
                if frame_count % 30 == 0 {
                    log::warn!("[survival] YOU ARE IN LAVA — get out NOW");
                }
            }
            let in_water = matches!(
                chunk_manager.block_at(feet[0], feet[1], feet[2]),
                unendlich_engine::world::blocks::BlockId::Water
            );
            if in_water != fg_app.swimming() {
                fg_app.set_swimming(in_water);
                log::info!("[survival] {}", if in_water {
                    "entered water — vitality drains 2x"
                } else {
                    "left water"
                });
            }
            // r545 (Roll 3 / C2): THIRST. Drink while standing in water (feet in a
            // water block); otherwise dehydrate — faster while sprinting. Player
            // drain is scaled down from the dragon constant (×0.3 ⇒ ~2.8 min idle)
            // so it's a periodic chore. Parched (<20%) saps vitality, feeding the
            // existing starvation/death loop without a separate death path.
            {
                if in_water {
                    fg_hydration.drink(dt);
                } else {
                    let exertion = if fg_app.stamina().sprinting { 1.0 } else { 0.0 };
                    fg_hydration.tick(dt * 0.3, exertion);
                }
                if fg_hydration.is_parched() {
                    fg_app.vitality_mut().tick(dt, 0.012);
                }
            }
            // r123 (worker Phase 7 port): EXPOSURE from the REAL climate model
            // (model.temperature, the same source as the per-window climate log),
            // not a made-up depth formula. T is normalized ~[0,1]; outside the
            // comfortable band the drain scales up, harshest at the extremes.
            let climate_t = {
                let m = chunk_manager.model_arc();
                let pp = local_player.view.position;
                let base = m.temperature(pp.x as f64, pp.y as f64, pp.z as f64, 0.0) as f32;
                // r128: dynamic heat sources (F9 campfires, synced lava) warm
                // the local climate — shelter by a fire counters freezing.
                // contribution is in °C-ish units; ~12°C ≈ +0.3 normalized.
                let warmth = heat_sources.total_heat_at(pp.x, pp.y, pp.z);
                base + warmth * 0.025
            };
            // r142: the ENGINE'S exposure model replaces the r123 band guess.
            // felt temperature = ambient (climate T mapped to °C) + wind chill
            // + wet chill - fire warmth - shelter; the body's Thermal state
            // lags it, and |comfort| drives the drain. Sheltered = solid
            // block within 3 above the head (a roof matters in a storm).
            let exposure = {
                use unendlich_engine::sim::survival_exposure as sxp;
                let ppx = local_player.view.position;
                let (_wxx, _wzz, wmagx) = unendlich_engine::wind::wind_at(
                    ppx.x as f64, ppx.y as f64, ppx.z as f64, sky_state.time_seconds,
                );
                let sheltered = (1..=3).any(|dy| {
                    !matches!(
                        chunk_manager.block_at(
                            ppx.x.floor() as i32,
                            (ppx.y + 0.6).floor() as i32 + dy,
                            ppx.z.floor() as i32,
                        ),
                        unendlich_engine::world::blocks::BlockId::Air
                    )
                });
                let heat_near = heat_sources.total_heat_at(ppx.x, ppx.y, ppx.z);
                let cond = sxp::Conditions {
                    ambient_c: climate_t * 45.0 - 5.0, // normalized T -> [-5, 40] °C
                    wind01: (wmagx as f32 * 0.2).clamp(0.0, 1.0),
                    // r156: rain soaks you unless the roof holds.
                    wet01: if in_water {
                        1.0
                    } else if !sheltered {
                        precip01 * 0.8
                    } else {
                        0.0
                    },
                    sheltered,
                    near_fire01: (heat_near * 0.05).clamp(0.0, 1.0),
                    clothing01: 0.0,
                };
                fg_thermal = sxp::step_body(fg_thermal, &cond, dt);
                // r178: a living body stays in physiological range — the
                // log showed 282 deg C (saturated fire term, unclamped
                // integrator). Hypo/hyperthermia play out inside 20..45.
                fg_thermal.body_c = fg_thermal.body_c.clamp(20.0, 45.0);
                let sev = fg_thermal.comfort.abs();
                // r178: creative has no body to warn about.
                if sev > 0.4 && frame_count % 240 == 0 && survival_mode {
                    log::info!(
                        "[survival] {} — body {:.1}°C (felt {:.1}°C{}{})",
                        if fg_thermal.comfort < 0.0 { "FREEZING" } else { "OVERHEATING" },
                        fg_thermal.body_c,
                        sxp::felt_temperature_c(&cond),
                        if sheltered { ", sheltered" } else { "" },
                        if cond.near_fire01 > 0.2 { ", by fire" } else { "" },
                    );
                }
                (sev - 0.25).max(0.0) * 0.06
            };
            // r172: vitality drains ONLY in survival (operator log: 97%->76%
            // over a minute in CREATIVE). Creative still ticks for regen.
            // r179: HOT POCKETS — heat_near (live heat map) charges a
            // Fire-attuned artifact: counter trickle + resonance recovery.
            {
                let hp = local_player.view.position;
                let heat_here = heat_sources.total_heat_at(hp.x, hp.y, hp.z);
                let heat01 = (heat_here * 0.02).clamp(0.0, 1.0);
                if heat01 > 0.3 && fg_imbue.element == 4 {
                    fg_imbue.counter = (fg_imbue.counter + 0.04 * heat01 * dt).min(1.0);
                    bend_resonance.charge01 = (bend_resonance.charge01 + 0.06 * heat01 * dt).min(1.0);
                    if frame_count % 600 == 0 {
                        log::info!("[bending] the heat feeds your artifact (imbue {:.0}%)", fg_imbue.counter * 100.0);
                    }
                }
            }
            let drain = if survival_mode {
                // r563: metabolic hunger scales with stature (Kleiber — bigger bodies
                // burn more); exposure is environmental and left unscaled.
                (if in_water { 0.02 } else { 0.01 }) * player_stature.hunger_rate + exposure
            } else {
                0.0
            };
            fg_app.vitality_mut().tick(dt, drain);
            // r128: PREDATOR CONTACT — a Dinosaur or Dragon within 1.6 blocks
            // bites every ~0.7s: vitality chip + log. Run, fight, or keep
            // distance; creative is exempt.
            if survival_mode && frame_count % 40 == 0 {
                use unendlich_engine::item::CreatureKind;
                let pp = local_player.view.position;
                for r in roamers.iter() {
                    if !matches!(r.kind, CreatureKind::Dinosaur | CreatureKind::Dragon) {
                        continue;
                    }
                    let p = r.creature.body.position;
                    let d2 = (p[0] - pp.x).powi(2) + (p[1] - pp.y).powi(2) + (p[2] - pp.z).powi(2);
                    if d2 < 1.6 * 1.6 {
                        fg_app.vitality_mut().tick(1.0, 0.04 * player_dmg_resist);
                        log::info!(
                            "[combat] {:?} bites you — vitality {:.0}%",
                            r.kind, fg_app.vitality().energy * 100.0
                        );
                        break; // one bite per window
                    }
                }
            }
            // r510: COOKING — hold RAW meat near a fire and it roasts over COOK_TIME into
            // cooked meat (the proper meal). Uses the live heat field (fires + lava) so any
            // heat source works: F9 campfire, a Fire-bending bloom, or standing by lava. The
            // progress decays when you step away or stop holding meat, so it's a deliberate act.
            if survival_mode {
                let holding_raw = hand_item.map_or(false, |h| h.0 == MEAT_ITEM)
                    && fg_app.inventory().count(unendlich_engine::items::ItemId(MEAT_ITEM)) > 0;
                let pp = local_player.view.position;
                let heat_near = heat_sources.total_heat_at(pp.x, pp.y, pp.z);
                if holding_raw && heat_near > 3.0 {
                    cook_progress += dt;
                    if cook_progress >= COOK_TIME {
                        cook_progress = 0.0;
                        if fg_app.inventory_mut().remove_item(
                            unendlich_engine::items::ItemId(MEAT_ITEM), 1,
                        ) == 0 {
                            fg_app.inventory_mut().add_item(
                                unendlich_engine::items::ItemId(COOKED_MEAT_ITEM),
                                1,
                                &fg_item_registry,
                            );
                            log::info!("[cooking] the meat sizzles over the fire — cooked! (eat it for more vitality)");
                        }
                    } else if frame_count % 30 == 0 {
                        log::info!("[cooking] roasting… {:.0}%", cook_progress / COOK_TIME * 100.0);
                    }
                } else if cook_progress > 0.0 {
                    cook_progress = (cook_progress - dt * 2.0).max(0.0); // cools when you step away
                }
            }
            // r125: DEATH at zero vitality (survival only) — drop everything
            // where you fell, respawn at world spawn, refill.
            if survival_mode && fg_app.vitality().energy <= 0.0 {
                let here = [
                    local_player.view.position.x,
                    local_player.view.position.y,
                    local_player.view.position.z,
                ];
                let mut dropped = 0u32;
                for i in 0..fg_app.inventory().len() {
                    if let Some(stack) = fg_app.inventory_mut().take(i) {
                        dropped += stack.count as u32;
                        fg_app.drops_mut().spawn(stack, here, [
                            ((i as f32) * 1.7).sin() * 2.0,
                            3.0,
                            ((i as f32) * 1.7).cos() * 2.0,
                        ]);
                    }
                }
                fg_app.vitality_mut().feed(1.0);
                // r161: a fresh body, not just a full meter — otherwise a
                // hypothermia death respawns at 28°C and dies again.
                fg_thermal = unendlich_engine::sim::survival_exposure::Thermal {
                    body_c: 36.5,
                    comfort: 0.0,
                };
                fg_app.stamina_mut().level = 1.0;
                bend_resonance = unendlich_engine::sim::bending_charge::Resonance::full();
                prev_fall_vy = 0.0;
                local_player.view.position = spawn_pos;
                log::info!(
                    "[survival] YOU DIED at ({:.0},{:.0},{:.0}) — {} item(s) dropped there; respawned",
                    here[0], here[1], here[2], dropped
                );
            }
            // r130: PLAYER BENDING (B) — stance from movement, element from
            // the ENVIRONMENT (in water -> Water, near heat -> Fire, airborne
            // -> Air, else Earth), ambient Fields from the real climate /
            // heat / wind samples. The resolved EffectIntent acts on the
            // world: Earth raises a stone pillar at the aim point, Fire
            // plants a heat source, strikes damage the aimed creature.
            if bindings.is_action_pressed(GameAction::Bend, &input)
                || bindings.is_action_pressed_mouse(GameAction::Bend, &input)
            {
                use unendlich_engine::sim::gesture_resolve::{Directional, Element as GElem, Gesture, Stance};
                let pp = local_player.view.position;
                let m = chunk_manager.model_arc();
                let heat_here = heat_sources.total_heat_at(pp.x, pp.y, pp.z);
                let (_wx2, _wz2, wmag2) = unendlich_engine::wind::wind_at(
                    pp.x as f64, pp.y as f64, pp.z as f64, sky_state.time_seconds,
                );
                // r676: DERIVE the seven ambient properties from real world state
                // via sim::bending_field (the spec's producer) instead of hardcoding
                // p/l/s/b. Pressure now rises with depth, seismic with depth+volcanic
                // proximity, biotic with nearby creatures, luminosity with daylight —
                // so affinity_access gates AND the r674 hybrid unlocks read true.
                let depth01 = ((64.0 - pp.y as f32) / 96.0).clamp(0.0, 1.0);
                let raw_env = unendlich_engine::sim::bending_field::RawEnv {
                    biome_moisture: m.humidity(pp.x as f64, pp.y as f64, pp.z as f64, 0.0) as f32,
                    rain_intensity: 0.0,
                    water_proximity: if fg_app.swimming() { 1.0 } else { 0.0 },
                    solar_altitude: 0.7,
                    biome_temperature: m.temperature(pp.x as f64, pp.y as f64, pp.z as f64, 0.0) as f32,
                    volcanic_proximity: (heat_here * 0.05).clamp(0.0, 1.0),
                    weather_wind: (wmag2 as f32 * 0.3).clamp(0.0, 1.0),
                    altitude01: (pp.y as f32 / 128.0).clamp(0.0, 1.0),
                    enclosure: depth01, // deeper reads more enclosed → wind damps
                    depth01,
                    weather_front: 0.0,
                    sky_light: 0.7,
                    weather_dim: 0.0,
                    geometry_block: 0.0,
                    impact_activity: 0.0,
                    entity_density: (roamers.len() as f32 / 12.0).clamp(0.0, 1.0),
                    vegetation_mass: 0.4,
                };
                let mut fields = unendlich_engine::sim::bending_field::sample(&raw_env);
                // r139: ARTIFACT IMBUE — holding an INGOT (ItemId 1000/1001)
                // attunes it: progress scales with how well the environment
                // matches its element (heat for the fire-leaning default).
                // The imbued artifact's FieldDelta amplifies the fields, per
                // the affinity docs' "worn ember-stone opens Embercraft".
                // r151: CANOPY SHIFT — leaves overhead apply the engine's
                // canopy field delta (humidity up, raw light down, bio up):
                // forests are bending sanctuaries for Wood-leaning arts.
                {
                    let under_canopy = (2..=12).any(|dy| {
                        matches!(
                            chunk_manager.block_at(
                                pp.x.floor() as i32,
                                (pp.y + 0.6).floor() as i32 + dy,
                                pp.z.floor() as i32,
                            ),
                            unendlich_engine::world::blocks::BlockId::OakLeaves
                                | unendlich_engine::world::blocks::BlockId::BirchLeaves
                        )
                    });
                    if under_canopy {
                        let cd = unendlich_engine::sim::canopy_field_shift::canopy_shift(
                            &unendlich_engine::sim::canopy_field_shift::PlantState {
                                strain_bias: 0.3,
                                growth01: 0.8,
                                element: 0,
                            },
                        );
                        fields.h = (fields.h + cd.h).clamp(0.0, 1.0);
                        fields.l = (fields.l + cd.l).clamp(0.0, 1.0);
                        fields.b = (fields.b + cd.b).clamp(0.0, 1.0);
                    }
                }
                let holding_ingot = hand_item.map_or(false, |h| h.0 == 1000 || h.0 == 1001);
                if holding_ingot {
                    let match01 = fields.r; // fire-leaning artifact: heat matches
                    fg_imbue = unendlich_engine::sim::artifact_imbue::tick_imbue(
                        fg_imbue, match01, 2, dt,
                    );
                    let d = unendlich_engine::sim::artifact_imbue::field_amplification(&fg_imbue);
                    fields.h += d.h; fields.r += d.r; fields.w += d.w;
                    fields.p += d.p; fields.l += d.l; fields.s += d.s; fields.b += d.b;
                    if fg_imbue.counter > 0.99 && frame_count % 1800 == 0 {
                        log::info!("[imbue] the ingot in your hand is fully attuned (tier {})", fg_imbue.tier);
                    }
                }
                // fly mode <=> no physics body attached (see flight toggle docs).
                let airborne_now = body.is_none() || fg_app.swimming();
                let stance = if airborne_now {
                    Stance::Aerial
                } else if bindings.is_action_pressed(GameAction::Sneak, &input) {
                    Stance::Rooted
                } else if bindings.is_action_pressed(GameAction::Sprint, &input) {
                    Stance::Surge
                } else {
                    Stance::Flow
                };
                let element = if fg_app.swimming() {
                    GElem::Water
                } else if heat_here > 4.0 {
                    GElem::Fire
                } else if body.is_none() {
                    GElem::Air
                } else {
                    GElem::Earth
                };
                // r534: BENDING SEPARATION — a hero character (Heliographer, Tidecaller,
                // Windwalker, Stormweaver, Gravitybender) bends its OWN element regardless
                // of surroundings. Ordinary characters keep the environment-derived element.
                let element = unendlich_engine::player_character::bend_element(&active_character.name)
                    .unwrap_or(element);
                let pitch_up = cam_pitch_for_bend(&local_player);
                let dir = if pitch_up > 0.5 {
                    Directional::Lift
                } else if pitch_up < -0.5 {
                    Directional::Ground
                } else {
                    Directional::Push
                };
                let aimv = local_player.forward();
                let bend_input = unendlich_engine::sim::player_bending::BendInput {
                    stance,
                    gesture: Gesture::Strike,
                    dir,
                    element,
                    aim: [aimv.x, aimv.y, aimv.z],
                };
                // r131: rhythm = stillness (standing builds resonance; sprinting
                // and flailing don't). Feed the REAL charge into the controller.
                let still = if stance == Stance::Flow || stance == Stance::Rooted { 0.9 } else { 0.2 };
                bend_resonance.build(still, dt);
                bend_resonance.decay(dt); // r138: overdraw debt recovers with time
                player_bending.charge01 = bend_resonance.charge01;
                if let Some(mut intent) = player_bending.step(&bend_input, &fields, dt) {
                    // r670: name the technique this cast matches (Tidal Surge, Stone
                    // Pillar, …) via the catalogue, for the HUD announce below.
                    if let Some(t) = unendlich_engine::sim::technique_catalogue::identify(
                        element,
                        stance,
                        Gesture::Strike,
                        dir,
                    ) {
                        fg_last_technique = Some((t.name, std::time::Instant::now()));
                    }
                    // r674: surface any environment-gated HYBRID discipline unlocked
                    // by the current ambient properties + resonance (Blood/Coral/
                    // Temporal Alignment …), per sim::hybrid_disciplines.
                    {
                        use unendlich_engine::sim::hybrid_disciplines as hd;
                        let env = hd::EnvProps {
                            w: fields.w,
                            h: fields.h,
                            r: fields.r,
                            s: fields.s,
                            p: fields.p,
                            l: fields.l,
                            b: fields.b,
                        };
                        let tier = (bend_resonance.charge01 * 3.0).round() as u8;
                        if let Some(d) = hd::all_accessible(&env, tier).first() {
                            fg_last_hybrid = Some((d.label(), std::time::Instant::now()));
                        }
                    }
                    let spent = bend_resonance.spend(0.25 + 0.25 * intent.power01);
                    // r670: accrue lifetime chi toward Resonance Rank.
                    fg_lifetime_chi += (spent.max(0.0) * 20.0) as u64;
                    // r138: forcing techniques without charge accrues OVERDRAW
                    // debt; overdrawn benders cast at 40% power and it costs
                    // stamina to push through. Stand still to recover.
                    if bend_resonance.is_overdrawn() {
                        intent.power01 *= 0.4;
                        fg_app.stamina_mut().level = (fg_app.stamina().level - 0.1).max(0.0);
                        log::info!(
                            "[bending] OVERDRAWN (debt {:.2}) — weakened cast, stamina sapped",
                            bend_resonance.overdraw01
                        );
                    }
                    log::info!("[bending] resonance spent {:.2}, remaining {:.2}", spent, bend_resonance.charge01);
                    // r267: turn this discharge into a TYPED ability effect, gated
                    // by cooldown + amplified by combo chaining. Off by default.
                    if ability_cast_enabled {
                        const ABILITY_COOLDOWN: f32 = 1.5;   // seconds between casts
                        const COMBO_WINDOW: f32 = 2.0;       // chain expires after this gap
                        let now = ability_elapsed.elapsed().as_secs_f32();
                        let cd = unendlich_engine::sim::ability_cooldown::cd_remaining(
                            ability_last_cast, now, ABILITY_COOLDOWN,
                        );
                        if cd <= 0.0 {
                            // chain grows if still inside the combo window, else resets to 1
                            ability_chain = if now - ability_last_chain_t <= COMBO_WINDOW {
                                ability_chain.saturating_add(1)
                            } else {
                                1
                            };
                            let mult = unendlich_engine::sim::ability_combo::combo_mult(
                                ability_chain, 1.0, 0.25, 2.0,
                            );
                            let charge = (spent * mult).max(0.0);
                            let eff = unendlich_engine::sim::ability_cast::cast(fg_imbue.element, charge);
                            if eff.kind != unendlich_engine::sim::ability_cast::KIND_NONE {
                                log::info!(
                                    "[ability] cast kind={} power={:.1} radius={:.1} (combo x{} ×{:.2})",
                                    eff.kind, eff.power, eff.radius, ability_chain, mult,
                                );
                                ability_last_cast = now;
                                ability_last_chain_t = now;
                            }
                        } else {
                            log::info!("[ability] on cooldown ({:.2}s left)", cd);
                        }
                    }
                    // r151: HYBRID — an attuned artifact in hand crosses its
                    // element with the cast (storm = air x water, etc.); the
                    // hybrid's potency multiplies the technique's power.
                    if hand_item.map_or(false, |h| h.0 == 1000 || h.0 == 1001)
                        && fg_imbue.counter > 0.5
                    {
                        use unendlich_engine::sim::hybrid_rules as hyb;
                        // (the `element` local is gesture_resolve's 4-variant
                        // enum — the r130 environment derivation feeds the
                        // gesture pipeline, not sim::elemental)
                        let cast_el = match element {
                            unendlich_engine::sim::gesture_resolve::Element::Earth => hyb::Element::Dust,
                            unendlich_engine::sim::gesture_resolve::Element::Water => hyb::Element::Water,
                            unendlich_engine::sim::gesture_resolve::Element::Fire => hyb::Element::Fire,
                            unendlich_engine::sim::gesture_resolve::Element::Air => hyb::Element::Air,
                        };
                        let art_el = match fg_imbue.element {
                            0 => hyb::Element::Wood,
                            1 => hyb::Element::Water,
                            2 => hyb::Element::Fire,
                            3 => hyb::Element::Air,
                            4 => hyb::Element::Dust,
                            _ => hyb::Element::Storm,
                        };
                        if let Some(hy) = hyb::cross(cast_el, art_el) {
                            if hy.element != cast_el || hy.potency > 1.0 {
                                intent.power01 = (intent.power01 * hy.potency).clamp(0.0, 1.5);
                                log::info!(
                                    "[bending] HYBRID {:?} burst (potency x{:.2}) — the artifact answers",
                                    hy.element, hy.potency
                                );
                            }
                        }
                    }
                    use unendlich_engine::sim::player_bending::Discipline;
                    log::info!(
                        "[bending] {:?} kind{} power {:.2} (fields h{:.2} r{:.2} w{:.2})",
                        intent.discipline, intent.shape.kind, intent.power01,
                        fields.h, fields.r, fields.w
                    );
                    match intent.discipline {
                        Discipline::Elemental(GElem::Earth) => {
                            // Raise a stone pillar at the aimed surface point.
                            let reach = 3.0 + intent.power01 * 6.0;
                            let bx = (pp.x + aimv.x * reach).floor() as i32;
                            let bz = (pp.z + aimv.z * reach).floor() as i32;
                            if let Some(sy0) = chunk_manager.generator().cpu()
                                .surface_y_at(bx, bz, probe_y_min, probe_y_max)
                            {
                                let h = 1 + (intent.power01 * 3.0) as i32;
                                let mut put = 0;
                                for dy in 1..=h {
                                    if chunk_manager.set_block(bx, sy0 + dy, bz, BlockId::Stone) {
                                        put += 1;
                                    }
                                }
                                log::info!("[bending] earth pillar +{} at ({},{})", put, bx, bz);
                            }
                        }
                        Discipline::ThermalPhase | Discipline::Elemental(GElem::Fire) => {
                            let fx = pp.x + aimv.x * 4.0;
                            let fz = pp.z + aimv.z * 4.0;
                            let fy = pp.y - 1.0;
                            heat_sources.add(unendlich_engine::heat_source::HeatSource::new(
                                [fx, fy, fz], 400.0 * (0.5 + intent.power01), 4.0,
                            ));
                            session.fire_positions.push([fx, fy, fz]);
                            // r532: thermal-phase payload — the heat MELTS nearby ice into
                            // water and sublimates snow, a visible phase change scaled by power.
                            let rad = 2 + (intent.power01 * 2.0) as i32; // 2..4 blocks
                            let (cx, cy, cz) = (fx.floor() as i32, fy.floor() as i32, fz.floor() as i32);
                            let mut melted = 0u32;
                            for dy in -2..=2 {
                                for dz in -rad..=rad {
                                    for dx in -rad..=rad {
                                        let (bx, by, bz) = (cx + dx, cy + dy, cz + dz);
                                        match chunk_manager.block_at(bx, by, bz) {
                                            BlockId::Ice => {
                                                if chunk_manager.set_block(bx, by, bz, BlockId::Water) {
                                                    melted += 1;
                                                }
                                            }
                                            BlockId::SnowBlock => {
                                                if chunk_manager.set_block(bx, by, bz, BlockId::Air) {
                                                    melted += 1;
                                                }
                                            }
                                            _ => {}
                                        }
                                    }
                                }
                            }
                            if melted > 0 {
                                renderer.hud_status_lines.push(format!("thermal phase melted {}", melted));
                            }
                            log::info!("[bending] thermal bloom at ({:.0},{:.0}), melted {}", fx, fz, melted);
                        }
                        Discipline::Stormstrike => {
                            // r556: STORMSTRIKE (Air → lightning) — route a chaining bolt
                            // through the most-conductive creatures in the aim cone via the
                            // bending_stormstrike core; mis-aiming grounds on the bender and
                            // saps stamina. A gust follows so close targets are also shoved.
                            use unendlich_engine::sim::bending_stormstrike as ss;
                            let conductors: Vec<ss::Conductor> = roamers
                                .iter()
                                .map(|r| ss::Conductor {
                                    pos: r.creature.body.position,
                                    conductance01: (0.45 + (r.hp as f32 / 50.0)
                                        + if r.airborne.is_some() { 0.25 } else { 0.0 }).clamp(0.0, 1.0),
                                    is_self: false,
                                })
                                .collect();
                            let plan = ss::route(
                                [pp.x, pp.y, pp.z],
                                [aimv.x, aimv.y, aimv.z],
                                &conductors,
                                2 + (intent.power01 * 3.0) as u8,
                                8.0,
                                0.80,
                            );
                            let mut struck = 0u32;
                            for (hop, &ri) in plan.path.iter().enumerate() {
                                if let Some(r) = roamers.get_mut(ri) {
                                    let dmg = (((10.0 + intent.power01 * 26.0)
                                        * 0.78f32.powi(hop as i32)) as i32).max(1);
                                    r.hp -= dmg;
                                    r.creature.torso_bend = 0.7 * intent.power01;
                                    if witness_enabled {
                                        let vk = match r.kind {
                                            unendlich_engine::item::CreatureKind::Dinosaur => 1u8,
                                            unendlich_engine::item::CreatureKind::Dragon => 2u8,
                                            _ => 0u8,
                                        };
                                        attack_events.push((r.creature.body.position, vk));
                                    }
                                    struck += 1;
                                }
                            }
                            if plan.grounded_on_self {
                                fg_app.stamina_mut().level =
                                    (fg_app.stamina().level - 0.2 * (1.0 + plan.penalty01)).max(0.0);
                                renderer.hud_status_lines.push("stormstrike grounded — mis-aimed!".to_string());
                                log::info!("[bending] stormstrike grounded on self (penalty {:.2})", plan.penalty01);
                            } else {
                                log::info!("[bending] stormstrike chained {} creature(s)", struck);
                            }
                            // a following gust shoves nearby creatures too
                            use unendlich_engine::sim::bending_compose::{resolve_bend, BendWorld};
                            use unendlich_engine::sim::bending_targeting::Target;
                            let targets: Vec<Target> = roamers
                                .iter()
                                .map(|r| Target { pos: r.creature.body.position, radius: 0.9 })
                                .collect();
                            let mut hit_n = 0u32;
                            if !targets.is_empty() {
                                let world = BendWorld::for_targets([pp.x, pp.y, pp.z], &targets, &[]);
                                let outcome = resolve_bend(&intent, &world, bend_resonance);
                                for h in &outcome.hits {
                                    if let Some(r) = roamers.get_mut(h.index) {
                                        let push = (2.0 + 7.0 * outcome.power_realized01) * h.strength01;
                                        r.creature.body.velocity[0] += aimv.x * push;
                                        r.creature.body.velocity[1] += aimv.y * push + 0.6 * h.strength01;
                                        r.creature.body.velocity[2] += aimv.z * push;
                                        r.creature.torso_bend = 0.5 * h.strength01;
                                        hit_n += 1;
                                    }
                                }
                            }
                            renderer.hud_status_lines.push(format!(
                                "air gust — {} caught", hit_n
                            ));
                            log::info!("[bending] air gust knocked back {} creature(s)", hit_n);
                        }
                        Discipline::HeavyBrine => {
                            // r556: WATERBENDING (HeavyBrine) — a surge that shoves creatures
                            // back (gentler than air, more horizontal) AND conjures a water
                            // block at the aimed surface point. (Was wired under a dead
                            // Elemental(Water) arm; Water resolves to HeavyBrine.)
                            use unendlich_engine::sim::bending_compose::{resolve_bend, BendWorld};
                            use unendlich_engine::sim::bending_targeting::Target;
                            let targets: Vec<Target> = roamers
                                .iter()
                                .map(|r| Target { pos: r.creature.body.position, radius: 0.9 })
                                .collect();
                            let mut hit_n = 0u32;
                            if !targets.is_empty() {
                                let world = BendWorld::for_targets([pp.x, pp.y, pp.z], &targets, &[]);
                                let outcome = resolve_bend(&intent, &world, bend_resonance);
                                for h in &outcome.hits {
                                    if let Some(r) = roamers.get_mut(h.index) {
                                        let push = (1.5 + 5.0 * outcome.power_realized01) * h.strength01;
                                        r.creature.body.velocity[0] += aimv.x * push;
                                        r.creature.body.velocity[2] += aimv.z * push;
                                        hit_n += 1;
                                    }
                                }
                            }
                            // Conjure water at the aimed surface (on solid ground, into open air).
                            let reach = 2.0 + intent.power01 * 5.0;
                            let bx = (pp.x + aimv.x * reach).floor() as i32;
                            let bz = (pp.z + aimv.z * reach).floor() as i32;
                            if let Some(sy0) = chunk_manager.generator().cpu()
                                .surface_y_at(bx, bz, probe_y_min, probe_y_max)
                            {
                                let _ = chunk_manager.set_block(bx, sy0 + 1, bz, BlockId::Water);
                            }
                            renderer.hud_status_lines.push(format!(
                                "water surge — {} caught", hit_n
                            ));
                            log::info!("[bending] water surge pushed {} creature(s)", hit_n);
                        }
                        _ => {
                            // Strike disciplines: damage the aimed creature.
                            let mut best: Option<(usize, f32)> = None;
                            for (ri, r) in roamers.iter().enumerate() {
                                let p = r.creature.body.position;
                                let to = glm::vec3(p[0] - pp.x, p[1] - pp.y, p[2] - pp.z);
                                let along = glm::dot(&to, &aimv);
                                if !(0.5..=12.0).contains(&along) { continue; }
                                let closest = pp + aimv * along;
                                if glm::distance(&closest, &glm::vec3(p[0], p[1] + 0.5, p[2])) < 1.6
                                    && best.map_or(true, |(_, d)| along < d)
                                {
                                    best = Some((ri, along));
                                }
                            }
                            if let Some((ri, d)) = best {
                                let dmg = (8.0 + intent.power01 * 24.0) as i32;
                                roamers[ri].hp -= dmg;
                                // r284: record for witnessed-attack reaction.
                                if witness_enabled {
                                    let vk = match roamers[ri].kind {
                                        unendlich_engine::item::CreatureKind::Dinosaur => 1u8,
                                        unendlich_engine::item::CreatureKind::Dragon => 2u8,
                                        _ => 0u8,
                                    };
                                    attack_events.push((roamers[ri].creature.body.position, vk));
                                }
                                roamers[ri].creature.torso_bend = 0.8 * intent.power01;
                                log::info!(
                                    "[bending] strike hits {:?} for {} at {:.0} blocks (hp {})",
                                    roamers[ri].kind, dmg, d, roamers[ri].hp.max(0)
                                );
                                if roamers[ri].hp <= 0 {
                                    let p = roamers[ri].creature.body.position;
                                    log::info!("[bending] {:?} slain", roamers[ri].kind);
                                    // r501: route death drops through the tested loot_roll
                                    // core (sim::batch_loot_roll) for varied, table-driven
                                    // loot instead of a fixed single meat. Bigger creatures
                                    // (more base hp) yield more meat.
                                    let seed = (p[0].to_bits() as u64)
                                        ^ ((p[2].to_bits() as u64) << 21)
                                        ^ (frame_count as u64).wrapping_mul(0x9E37_79B1);
                                    let table: [(u16, u32, f32); 4] = [
                                        (MEAT_ITEM as u16, 1, 1.0),   // always ≥1 meat
                                        (MEAT_ITEM as u16, 1, 0.6),   // often a 2nd
                                        (MEAT_ITEM as u16, 1, 0.25),  // sometimes a 3rd
                                        (999, 1, 0.12),               // occasional egg
                                    ];
                                    for (k, (id, count)) in unendlich_engine::sim::batch_loot_roll::loot_one(&table, seed)
                                        .into_iter()
                                        .enumerate()
                                    {
                                        let a = (seed.wrapping_add(k as u64) & 0xFF) as f32 / 255.0 * 6.2831;
                                        fg_app.drops_mut().spawn(
                                            unendlich_engine::items::ItemStack {
                                                item: unendlich_engine::items::ItemId(id as u32),
                                                count: count as u16,
                                            },
                                            [p[0] + a.cos() * 0.3, p[1] + 0.6, p[2] + a.sin() * 0.3],
                                            [a.cos() * 1.4, 2.0, a.sin() * 1.4],
                                        );
                                    }
                                    roamers.swap_remove(ri);
                                }
                            } else {
                                log::info!("[bending] strike finds no target");
                            }
                        }
                    }
                }
            }
            // r123: CRAFTING (C) — the engine's real items::crafting::{Recipe,
            // can_craft, craft} against the App inventory (the worker's version
            // was log-only string recipes). First craftable recipe wins.
            if input.is_key_just_pressed(Keycode::C) {
                // r169: C opens the crafting MENU (Minecraft-nostalgia rows);
                // crafting happens via its buttons now.
                renderer.craft_menu_open = !renderer.craft_menu_open;
                if renderer.craft_menu_open {
                    window.set_cursor_mode(CursorMode::Normal);
                    mouse_captured = false;
                }
                // (r170: authority below.)
            }
            if renderer.craft_menu_open || renderer.craft_pick != 0 || show_inventory {
                use unendlich_engine::items::{crafting, ItemId, ItemStack};
                use unendlich_engine::world::blocks::BlockId;
                let id = |b: BlockId| ItemId(b as u32);
                // r126: ingots live above the block id space (1000+metal idx).
                const INGOT_BASE: u32 = 1000;
                let ing = |b: BlockId, n: u16| (id(b), n);
                let metal = |idx: u32, n: u16| (ItemId(INGOT_BASE + idx), n);
                let out = |item: u32, count: u16| ItemStack { item: ItemId(item), count };
                let out_b = |b: BlockId, count: u16| ItemStack { item: id(b), count };
                // r552: a real survival crafting tree — smelting, building stock, the
                // forged WEAPONS (gather→smelt→forge→fight loop) and cooking. Weapon
                // ids 4000..4007 = Dagger/Blade/Greatsword/Axe/Greataxe/Spear/Staff/
                // Wand; ingots 1000+idx (Iron/Gold/Copper).
                let recipes = [
                    // ── smelting ──
                    ("Iron Ingot  ·  Iron Ore + Coal", crafting::Recipe::new(
                        vec![ing(BlockId::IronOre, 1), ing(BlockId::CoalOre, 1)], out(INGOT_BASE, 1))),
                    ("Gold Ingot  ·  Gold Ore + Coal", crafting::Recipe::new(
                        vec![ing(BlockId::GoldOre, 1), ing(BlockId::CoalOre, 1)], out(INGOT_BASE + 1, 1))),
                    ("Copper Ingot  ·  Copper Ore + Coal", crafting::Recipe::new(
                        vec![ing(BlockId::CopperOre, 1), ing(BlockId::CoalOre, 1)], out(INGOT_BASE + 2, 1))),
                    // ── wood & stone building stock ──
                    ("Oak Planks ×4  ·  1 Oak Log", crafting::Recipe::new(
                        vec![ing(BlockId::OakLog, 1)], out_b(BlockId::OakPlanks, 4))),
                    ("Stone  ·  4 Cobblestone", crafting::Recipe::new(
                        vec![ing(BlockId::Cobblestone, 4)], out_b(BlockId::Stone, 1))),
                    ("Stone Bricks ×4  ·  4 Stone", crafting::Recipe::new(
                        vec![ing(BlockId::Stone, 4)], out_b(BlockId::StoneBricks, 4))),
                    ("Glass  ·  2 Sand", crafting::Recipe::new(
                        vec![ing(BlockId::Sand, 2)], out_b(BlockId::Glass, 1))),
                    ("Bricks  ·  1 Clay", crafting::Recipe::new(
                        vec![ing(BlockId::Clay, 1)], out_b(BlockId::Bricks, 1))),
                    // ── forged weapons (ingots + a plank handle) ──
                    ("Dagger  ·  1 Iron Ingot + 1 Planks", crafting::Recipe::new(
                        vec![metal(0, 1), ing(BlockId::OakPlanks, 1)], out(4000, 1))),
                    ("Blade  ·  2 Iron Ingot + 1 Planks", crafting::Recipe::new(
                        vec![metal(0, 2), ing(BlockId::OakPlanks, 1)], out(4001, 1))),
                    ("Greatsword  ·  3 Iron Ingot + 1 Planks", crafting::Recipe::new(
                        vec![metal(0, 3), ing(BlockId::OakPlanks, 1)], out(4002, 1))),
                    ("Axe  ·  2 Iron Ingot + 2 Planks", crafting::Recipe::new(
                        vec![metal(0, 2), ing(BlockId::OakPlanks, 2)], out(4003, 1))),
                    ("Greataxe  ·  3 Iron Ingot + 2 Planks", crafting::Recipe::new(
                        vec![metal(0, 3), ing(BlockId::OakPlanks, 2)], out(4004, 1))),
                    ("Spear  ·  1 Iron Ingot + 2 Planks", crafting::Recipe::new(
                        vec![metal(0, 1), ing(BlockId::OakPlanks, 2)], out(4005, 1))),
                    ("Staff  ·  1 Gold Ingot + 2 Planks", crafting::Recipe::new(
                        vec![metal(1, 1), ing(BlockId::OakPlanks, 2)], out(4006, 1))),
                    ("Builder Wand  ·  1 Gold Ingot + 1 Glass", crafting::Recipe::new(
                        vec![metal(1, 1), ing(BlockId::Glass, 1)], out(4007, 1))),
                    // ── cooking (campfire alternative) ──
                    ("Cooked Meat  ·  Raw Meat + Coal", crafting::Recipe::new(
                        vec![(ItemId(2000), 1), ing(BlockId::CoalOre, 1)], out(2001, 1))),
                ];
                // r141/r552: the REAL sim::recipe_book indexes recipes by ingredient
                // so holding an item floats its recipes to the top. Built from the
                // recipe inputs above so the index always matches the table.
                let book_sets: Vec<Vec<unendlich_engine::sim::recipe_book::ItemId>> = recipes
                    .iter()
                    .map(|(_, r)| {
                        r.inputs
                            .iter()
                            .map(|(iid, _)| unendlich_engine::sim::recipe_book::ItemId(iid.0 as u16))
                            .collect()
                    })
                    .collect();
                let book = unendlich_engine::sim::recipe_book::RecipeBook::build(&book_sets);
                let preferred: Vec<usize> = hand_item
                    .map(|h| {
                        book.find_by_ingredient(
                            unendlich_engine::sim::recipe_book::ItemId(h.0 as u16),
                        )
                        .to_vec()
                    })
                    .unwrap_or_default();
                let order: Vec<usize> = preferred
                    .iter()
                    .copied()
                    .chain((0..recipes.len()).filter(|i| !preferred.contains(i)))
                    .collect();
                // r169: fill the menu rows (recipe-book preferred order kept:
                // holding an ingredient floats its recipes to the top).
                // r176: canonical tooltip per output (doc item_armor_lore_
                // tooltips: format FINAL, lore placeholder for world voice).
                let tip_for = |out: u32| -> String {
                    // r176b: curly quotes — straight quotes inside the
                    // literal terminated it (the r176 wall).
                    match out {
                        1000 => "Iron Ingot\nSmelting yield: 1 per ore + coal\nBase metal for blades and tools\n\n\u{201c}The mountain gives grudgingly, and only to fire.\u{201d}\n\u{2014} Observed by the Survey, Iron Hills".to_string(),
                        1001 => "Gold Ingot\nSmelting yield: 1 per ore + coal\nSoft; takes imbue readily\n\n\u{201c}Worth more bent than spent.\u{201d}\n\u{2014} Trade-stall tag, river market".to_string(),
                        o if o == unendlich_engine::world::blocks::BlockId::OakPlanks as u32 =>
                            "Oak Planks\nYield: 4 per log\nBasic building stock\n\n\u{201c}Every wall remembers being a tree.\u{201d}\n\u{2014} Dojo inscription, First Grove".to_string(),
                        o if o == unendlich_engine::world::blocks::BlockId::Stone as u32 =>
                            "Stone\nYield: 1 per 4 cobble\nDressed block; even faces\n\n\u{201c}Patience is a mason\u{2019}s first tool.\u{201d}\n\u{2014} Field record, Carrier designation 7".to_string(),
                        o if o == unendlich_engine::world::blocks::BlockId::Glass as u32 =>
                            "Glass\nYield: 1 per 2 sand\nTransparent; shatters on break\n\n\u{201c}Sand that learned to hold light.\u{201d}\n\u{2014} Shrine fragment, East Wall".to_string(),
                        1002 => "Copper Ingot\nSmelting yield: 1 per ore + coal\nConducts the imbue; soft but willing".to_string(),
                        o if o == unendlich_engine::world::blocks::BlockId::StoneBricks as u32 =>
                            "Stone Bricks ×4\nDressed masonry from 4 stone\nClean courses for walls and keeps".to_string(),
                        o if o == unendlich_engine::world::blocks::BlockId::Bricks as u32 =>
                            "Bricks\nFired from clay\nWarm-toned building stock".to_string(),
                        2001 => "Cooked Meat\nRaw meat seared over coal\nRestores far more than raw — and won't turn".to_string(),
                        4000 => "Dagger\nFast, light, cheap to forge\n1 Iron Ingot + a plank handle".to_string(),
                        4001 => "Blade\nBalanced sidearm\n2 Iron Ingot + a plank handle".to_string(),
                        4002 => "Greatsword\nHeavy two-hander; big swing\n3 Iron Ingot + a plank handle".to_string(),
                        4003 => "Axe\nHews wood and foe alike\n2 Iron Ingot + 2 planks".to_string(),
                        4004 => "Greataxe\nBrutal reach and weight\n3 Iron Ingot + 2 planks".to_string(),
                        4005 => "Spear\nKeeps the danger at arm's length\n1 Iron Ingot + 2 planks".to_string(),
                        4006 => "Staff\nA channel for the imbue\n1 Gold Ingot + 2 planks".to_string(),
                        4007 => "Builder Wand\nShapes and places at range\n1 Gold Ingot + 1 Glass".to_string(),
                        _ => "Crafted item\n(no canonical entry yet)".to_string(),
                    }
                };
                renderer.craft_lines = order
                    .iter()
                    .map(|&ri_idx| {
                        let (name, r) = &recipes[ri_idx];
                        let can = crafting::can_craft(fg_app.inventory(), r);
                        let have: String = r
                            .inputs
                            .iter()
                            .map(|(iid, n)| {
                                format!("{}/{} #{}", fg_app.inventory().count(*iid), n, iid.0)
                            })
                            .collect::<Vec<_>>()
                            .join("  ");
                        (name.to_string(), can, have, tip_for(r.output.item.0))
                    })
                    .collect();
                // Execute the menu's pick (1-based index into the same order).
                if renderer.craft_pick != 0 {
                    let pick = renderer.craft_pick as usize - 1;
                    renderer.craft_pick = 0;
                    if let Some(&ri_idx) = order.get(pick) {
                        let (name, r) = &recipes[ri_idx];
                        if crafting::can_craft(fg_app.inventory(), r)
                            && crafting::craft(fg_app.inventory_mut(), r, &fg_item_registry)
                        {
                            log::info!("[craft] {}", name);
                            fg_rb_craft_flag = true; // r186: rollback point
                        }
                    }
                }
            }
            // (r124: V-to-eat retired — eating is the right-mouse USE event on an
            // edible hand item in survival mode, per operator. See survival_use_item.)
            // r122: drop physics — gravity + settle on real voxels (solid =
            // any non-air, non-water block at that cell), then age-out.
            fg_app.drops_mut().update(dt, -18.0, |bx, by, bz| {
                !matches!(
                    chunk_manager.block_at(bx, by, bz),
                    unendlich_engine::world::blocks::BlockId::Air
                        | unendlich_engine::world::blocks::BlockId::Water
                )
            });
            // r448 (items track): dropped items float in water and ride the current. Buoyancy (24)
            // exceeds the gravity above so a submerged drop rises to bob at the surface; the flow
            // field carries it horizontally (reuses the same current the player feels).
            fg_app.drops_mut().apply_water(
                dt,
                24.0,
                4.0,
                |bx, by, bz| {
                    chunk_manager.block_at(bx, by, bz)
                        == unendlich_engine::world::blocks::BlockId::Water
                },
                |bx, bz| {
                    let (fx, fz) = water_flow_tiles.sample(bx, bz);
                    [fx, fz]
                },
            );
            let collected = fg_app.drops_mut().collect_near(ppos);
            for stack in collected {
                let item = stack.item;
                let n = stack.count;
                match fg_app.inventory_mut().add_stack(stack, &fg_item_registry) {
                    None => log::info!("[items] picked up {}x item#{}", n, item.0),
                    Some(overflow) => log::warn!(
                        "[items] inventory full — {}x item#{} dropped back",
                        overflow.count, overflow.item.0),
                }
            }
        }

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
        // r175: the audio slider drives the REAL rodio sink now — master =
        // slider x the director's cue volume (pause-duck and menu cues
        // become audible TODAY, not post-backend).
        // r178: the veil eases off over ~2s; music fades with it.
        if renderer.loading_fade > 0.0 {
            renderer.loading_fade = (renderer.loading_fade - dt * 0.5).max(0.0);
        }
        // r187: cinematic entry — music begins only once the menu is
        // actually visible (veil below 0.6), then fades in over ~2.5s.
        if renderer.menu_open && renderer.loading_fade < 0.6 {
            fg_music_fadein = (fg_music_fadein + dt * 0.4).min(1.0);
        }
        #[cfg(feature = "audio")]
        if frame_count % 15 == 0 {
            if let Some(m) = fg_music_player.as_ref() {
                // r179: crafting/creative menus are IN-WORLD — the title
                // theme doesn't belong there. Title bus: menus only.
                let title_bus = if renderer.menu_open || renderer.wizard_open { 1.0 } else { 0.0 };
                m.set_master_volume(
                    renderer.ui_music_vol
                        * fg_music.volume01.max(0.05)
                        * (1.0 - renderer.loading_fade * 0.8)
                        * title_bus
                        * fg_music_fadein, // r187: cinematic fade-in
                );
            }
        }
        // r184: performance levers — sliders write the live atomics; the
        // autotuner nudges them toward the FPS target (raise strides ->
        // lower budget below target; relax above 1.25x target).
        if frame_count % 15 == 0 {
            plant_stride_atom.store(renderer.ui_plant_stride.max(1), std::sync::atomic::Ordering::Relaxed);
            leaf_stride_atom.store(renderer.ui_leaf_stride.max(1), std::sync::atomic::Ordering::Relaxed);
            chunk_manager.set_apply_budget(renderer.ui_apply_budget as usize);
            // r197: live render distance — the slider drives the override
            // atomic that runtime_render_distance() consults first. r476: the
            // adaptive controller caps it BELOW the chosen value under load.
            let effective_rd = if renderer.ui_adaptive_rd {
                renderer.ui_render_distance.min(adaptive_rd_cap).max(2)
            } else {
                renderer.ui_render_distance
            };
            unendlich_engine::constants::set_render_distance_override(effective_rd);
            // r476: push the live mesh-throttle slider to the global the octree
            // dispatcher reads each dispatch.
            unendlich_engine::constants::set_gpu_mesh_yield_pct(renderer.ui_gpu_mesh_yield);
            unendlich_engine::constants::set_octree_tile(renderer.ui_octree_tile);
        }

        // r199: HOTKEY REBINDING (Controls tab). Runs EVERY frame so the
        // key-capture can't miss a press. Three steps:
        //  (1) feed the page the current (label, key) rows;
        //  (2) a clicked row (keybind_rebind_request) arms capture;
        //  (3) while armed, the first candidate key just-pressed is bound
        //      (Esc cancels), then we persist + refresh the list.
        {
            use unendlich_engine::keybind_ui;
            // (1) build display rows from the live bindings.
            renderer.menu_keybind_list = keybind_ui::rebindable_actions()
                .iter()
                .map(|(action, label)| {
                    // r202: show ALL bound keys (primary + the arrow-key
                    // fallback), so a player can SEE that movement also
                    // works on the arrows regardless of letter layout.
                    let keys = bindings.keys_for_action(*action);
                    let mut parts: Vec<String> =
                        keys.iter().map(|k| keybind_ui::key_label(*k)).collect();
                    // r528: also show any mouse-button binding (e.g. a side button).
                    for b in bindings.mouse_buttons_for_action(*action) {
                        parts.push(keybind_ui::mouse_label(*b));
                    }
                    let key = if parts.is_empty() {
                        "—".to_string()
                    } else {
                        parts.join(" / ")
                    };
                    (label.to_string(), key)
                })
                .collect();


            // (2) a row was clicked -> arm capture for that action index.
            if renderer.keybind_rebind_request >= 0 {
                renderer.keybind_capturing = renderer.keybind_rebind_request;
                renderer.keybind_rebind_request = -1;
            }

            // (3) capture in progress.
            if renderer.keybind_capturing >= 0 {
                let idx = renderer.keybind_capturing as usize;
                if input.is_key_just_pressed(Keycode::Escape) {
                    renderer.keybind_capturing = -1; // cancel
                } else if let Some(&(action, _)) =
                    keybind_ui::rebindable_actions().get(idx)
                {
                    let mut bound = None;
                    for k in keybind_ui::capture_candidates() {
                        if input.is_key_just_pressed(k) {
                            bound = Some(k);
                            break;
                        }
                    }
                    if let Some(k) = bound {
                        bindings.set_binding(action, k);
                        renderer.keybind_capturing = -1;
                        if let Err(e) = bindings.save_to_file(&keybinds_path) {
                            log::warn!("[keybind] save failed: {e}");
                        } else {
                            log::info!("[keybind] {action:?} -> {k:?}");
                        }
                    } else {
                        // r528: no key — try a mouse button (middle / side buttons),
                        // so abilities like Bending can live on Mouse-4/5.
                        let mut mbound = None;
                        for b in keybind_ui::mouse_capture_candidates() {
                            if input.is_mouse_button_just_pressed(b) {
                                mbound = Some(b);
                                break;
                            }
                        }
                        if let Some(b) = mbound {
                            bindings.set_mouse_binding(action, b);
                            renderer.keybind_capturing = -1;
                            if let Err(e) = bindings.save_to_file(&keybinds_path) {
                                log::warn!("[keybind] save failed: {e}");
                            } else {
                                log::info!("[keybind] {action:?} -> {b:?} (mouse)");
                            }
                        }
                    }
                }
            }
        }

        if renderer.ui_autotune && frame_count % 120 == 0 && frame_times_ms.len() >= 60 {
            let lo = frame_times_ms.len() - 60;
            let avg = frame_times_ms[lo..].iter().sum::<f64>() / 60.0;
            let fps = if avg > 0.0 { 1000.0 / avg } else { 0.0 };
            let target = renderer.ui_fps_target as f64;
            if fps < target * 0.95 {
                if renderer.ui_plant_stride < 16 {
                    renderer.ui_plant_stride += 1;
                } else if renderer.ui_leaf_stride < 8 {
                    renderer.ui_leaf_stride += 1;
                } else if renderer.ui_apply_budget > 2 {
                    renderer.ui_apply_budget -= 1;
                }
                log::info!("[autotune] fps {:.0} < {:.0}: plants x{} leaves x{} budget {}",
                    fps, target, renderer.ui_plant_stride, renderer.ui_leaf_stride, renderer.ui_apply_budget);
            } else if fps > target * 1.25 {
                if renderer.ui_apply_budget < 8 {
                    renderer.ui_apply_budget += 1;
                } else if renderer.ui_leaf_stride > 2 {
                    renderer.ui_leaf_stride -= 1;
                } else if renderer.ui_plant_stride > 4 {
                    renderer.ui_plant_stride -= 1;
                }
            }
        }
        // r169: apply the settings sliders (live, idempotent).
        if renderer.settings_menu_open {
            sky_state.day_length = renderer.ui_day_length;
            unendlich_engine::debug_settings::DEBUG
                .disable_fog
                .store(!renderer.ui_fog_on, std::sync::atomic::Ordering::Relaxed);
            unendlich_engine::debug_settings::DEBUG
                .disable_shadows
                .store(!renderer.ui_shadows_on, std::sync::atomic::Ordering::Relaxed);
        }
        // r161: paused = the sun and the storm hold still too.
        if !game_paused {
            sky_state.tick(dt);
            weather_state.tick(dt);
        }
        precip01 = weather_state.params().precipitation; // r156: gameplay-side weather

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
        // r319: double-tap Space (Jump) toggles flight (creative convenience).
        let dbl_space_flight = {
            let mut req = false;
            if bindings.is_action_just_pressed(GameAction::Jump, &input) {
                let now = Instant::now();
                if last_jump_tap.map_or(false, |p| now.duration_since(p).as_millis() < 300) {
                    req = true;
                    last_jump_tap = None;
                } else {
                    last_jump_tap = Some(now);
                }
            }
            req
        };
        if bindings.is_action_just_pressed(GameAction::ToggleFlight, &input) || dbl_space_flight {
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
                let mut new_body = PhysicsBody::new_player(safe_pos);
                // r561: size the collider to the stature, keeping the feet at the safe
                // spawn (shift the centre up by the half-height delta vs the 1.8 default).
                let old_half = new_body.half_extents[1];
                new_body.half_extents = player_stature.half_extents;
                new_body.position[1] += player_stature.half_extents[1] - old_half;
                let center_y = new_body.position[1];
                log::info!(
                    "Switched to PHYSICS mode at ({:.1}, {:.1}, {:.1}) (camera was at ({:.1}, {:.1}, {:.1}))",
                    safe_pos[0], safe_pos[1], safe_pos[2], p.x, p.y, p.z
                );
                body = Some(new_body);
                // Snap camera to the new body's eye level so the user's
                // view doesn't jump weirdly.
                local_player.view.position = glm::vec3(
                    safe_pos[0],
                    center_y + player_eye_offset,
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
                // r561: base walk speed scales with stature (longer stride);
                // = WALK_SPEED for a default 1.8 character. Sprint multiplies on top.
                let mut speed = player_stature.move_speed;
                // r118: sprint costs stamina — gate on the meter (ticked above).
                if bindings.is_action_pressed(GameAction::Sprint, &input)
                    && fg_app.stamina().sprinting {
                    speed *= physics::constants::SPRINT_MULTIPLIER;
                }
                if bindings.is_action_pressed(GameAction::Sneak, &input) {
                    speed *= physics::constants::SNEAK_MULTIPLIER;
                }
                // r160: WINDED — running the meter to empty costs you: walk
                // speed drops 25% until stamina recovers past 15%.
                if survival_mode && fg_app.stamina().level < 0.15 {
                    speed *= 0.75;
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

                // r135: SWIMMING — when the body is in water, the REAL
                // sim::swim parameters govern motion: quadratic drag bounds
                // speed, buoyancy floats you gently, holding Jump is the
                // stroke (thrust up), Sneak dives. Submersion fraction from
                // sim::fluid scales everything so wading barely differs.
                let feet_in_water = matches!(
                    chunk_manager.block_at(
                        b.position[0].floor() as i32,
                        (b.position[1] - 0.4).floor() as i32,
                        b.position[2].floor() as i32,
                    ),
                    BlockId::Water
                );
                if feet_in_water {
                    // r508: CONTINUOUS SUBMERSION + FLOAT-OUT. The old code derived `sub` from a
                    // two-state head check (water level 0.9 or 2.0 over a 1.8 body), so the instant
                    // the head cleared the surface `sub` halved and buoyancy (26·sub) lost to gravity
                    // (28), yanking the head back under — you could never surface (operator report).
                    // Instead, find the REAL water surface at the body's column and measure how much
                    // of feet..head is below it, then float with a peak buoyancy ABOVE gravity so a
                    // submerged body RISES and settles where the head clears (equilibrium sub ≈
                    // GRAVITY/BUOY). Vertical water drag damps the rise into a smooth bob, not a
                    // linear ramp.
                    let col_x = b.position[0].floor() as i32;
                    let col_z = b.position[2].floor() as i32;
                    let feet_yf = b.aabb_min()[1];
                    let head_yf = b.aabb_max()[1];
                    let body_h = (head_yf - feet_yf).max(0.1);
                    let mut water_surface = feet_yf;
                    for yy in (feet_yf.floor() as i32 - 1)..=(head_yf.ceil() as i32 + 1) {
                        if matches!(chunk_manager.block_at(col_x, yy, col_z), BlockId::Water) {
                            water_surface = water_surface.max(yy as f32 + 1.0);
                        }
                    }
                    let sub = ((water_surface - feet_yf) / body_h).clamp(0.0, 1.0);
                    let head_in_water = water_surface >= head_yf - 0.1;
                    let sp = unendlich_engine::sim::swim::SwimParams::default();
                    // Quadratic drag (per swim.rs): a = -drag * |v| * v
                    let vmag = (b.velocity[0].powi(2) + b.velocity[1].powi(2) + b.velocity[2].powi(2)).sqrt();
                    for k in 0..3 {
                        b.velocity[k] -= sp.drag * vmag * b.velocity[k] * dt * sub;
                    }
                    // Archimedes float: peak buoyancy 36 > GRAVITY 28 ⇒ equilibrium sub ≈ 0.78, i.e.
                    // the waterline settles near the shoulders with the head/eyes OUT. Vertical
                    // damping turns the rise into a soft bob instead of a linear shoot-and-overshoot.
                    const SWIM_BUOY: f32 = 36.0;
                    const SWIM_VDAMP: f32 = 3.0;
                    // r567: density (from the muscle slider) modulates float — a dense
                    // body floats less (can sink, must stroke up), a lean one rides high.
                    // = 36 (waterline at the shoulders) for a default character.
                    let buoy = SWIM_BUOY / player_stature.density.clamp(0.4, 2.5);
                    b.velocity[1] += buoy * sub * dt;
                    b.velocity[1] -= SWIM_VDAMP * b.velocity[1] * sub * dt;
                    // Stroke: hold Jump to thrust up; Sneak to dive.
                    if bindings.is_action_pressed(GameAction::Jump, &input) {
                        b.velocity[1] += sp.thrust * sub * dt;
                    } else if bindings.is_action_pressed(GameAction::Sneak, &input) {
                        b.velocity[1] -= sp.thrust * 0.7 * sub * dt;
                    }
                    // r204: SWIMMING IS DIRECTIONAL WITH THE CAMERA (operator
                    // request). When forward movement is held underwater, add
                    // thrust along the FULL 3D look vector (pitch included),
                    // so looking down + forward swims you down-forward and
                    // looking up climbs. The flat XZ apply_input above gives
                    // the planar component; this adds the vertical the flatten
                    // dropped, scaled by submersion. Only when actually moving
                    // forward/back (not strafing), so vertical aim doesn't
                    // fight the Jump/Sneak stroke when idle.
                    let fwd_input = bindings.is_action_pressed(GameAction::MoveForward, &input);
                    let back_input = bindings.is_action_pressed(GameAction::MoveBackward, &input);
                    if fwd_input || back_input {
                        let look = local_player.forward();
                        let sign = if back_input && !fwd_input { -1.0 } else { 1.0 };
                        // Only the vertical part — the horizontal is already
                        // driven by apply_input — so we don't double the speed.
                        b.velocity[1] += look.y * sign * sp.thrust * 0.9 * sub * dt;
                    }
                    // r442: THE CURRENT PUSHES YOU. Sample the water flow field at the body and add
                    // a horizontal drift force (scaled by submersion), so flowing water carries a
                    // swimmer — you can still swim against it, you just get pulled. Gentle by
                    // default; tunable/off via UNENDLICH_WATER_CURRENT*.
                    if water_current_on {
                        let (cfx, cfz) = water_flow_tiles
                            .sample(b.position[0] as i32, b.position[2] as i32);
                        let drift = unendlich_engine::sim::current_force::drift_velocity(
                            [cfx, cfz],
                            water_current_strength,
                            water_current_max,
                            sub,
                        );
                        // r566: WATER-CURRENT RESISTANCE from stature — a heavier (taller)
                        // body needs a stronger current to budge (threshold ∝ mass) and is
                        // pushed less once it does (mult ∝ 1/h). = unchanged for a default.
                        let cur_mag = (drift[0] * drift[0] + drift[1] * drift[1]).sqrt();
                        let thresh = 0.05 * player_stature.water_resist_threshold;
                        if cur_mag > thresh {
                            let m = player_stature.water_resist_mult;
                            // additive acceleration toward the drift; quadratic drag (above) bounds it
                            b.velocity[0] += drift[0] * 4.0 * dt * m;
                            b.velocity[2] += drift[1] * 4.0 * dt * m;
                        }
                    }
                    // Terminal cap so drag math stays sane on bad frames.
                    b.velocity[1] = b.velocity[1].clamp(-8.0, 6.0);
                } else if bindings.is_action_just_pressed(GameAction::Jump, &input) {
                    if b.try_jump() {
                        // r561: take-off velocity scales with stature (= JUMP_VELOCITY
                        // for a default character; taller jumps higher absolutely).
                        b.velocity[1] = player_stature.jump_velocity;
                        jump_ascending = true; // r508: arm variable-height cut
                    }
                }
                // r508: VARIABLE JUMP HEIGHT — releasing Jump while still rising cuts the upward
                // velocity, so a tap is a short hop and a hold is the full jump. Out of water only
                // (water has its own buoyant stroke). Makes the jump read as a deliberate effort
                // instead of one fixed, linear-feeling arc.
                if !feet_in_water {
                    let jump_held = bindings.is_action_pressed(GameAction::Jump, &input);
                    if jump_ascending && b.velocity[1] > 0.0 && !jump_held {
                        b.velocity[1] *= 0.45;
                        jump_ascending = false;
                    }
                    if b.velocity[1] <= 0.0 {
                        jump_ascending = false;
                    }
                }
                // r141: FALL IMPACT — landing converts descent speed into
                // damage/stagger via the REAL sim::fall_impact model
                // (humanoid body). Water landings are exempt (swim drag
                // already bled the speed). Survival only.
                {
                    let vy = b.velocity[1];
                    let landed = prev_fall_vy < -9.0 && vy > -1.0 && !feet_in_water;
                    // r265: kick up a dust puff at the feet on any real landing
                    // (visual, independent of survival damage). count scales a
                    // little with impact speed.
                    if landed {
                        if let Some(ps) = particle_sys.as_mut() {
                            let speed = (-prev_fall_vy).max(0.0);
                            let count = (6.0 + speed * 0.8).min(48.0) as u32;
                            ps.emit(
                                unendlich_engine::sim::particles::Preset::Dust,
                                [b.position[0], b.position[1], b.position[2]],
                                count,
                            );
                        }
                        // r570: pulse the avatar's landing crouch, deeper with impact
                        // speed; the gait block decays it back out over ~¼ s.
                        let speed = (-prev_fall_vy).max(0.0);
                        hero_land_crouch = ((speed - 6.0) / 14.0).clamp(0.35, 1.0);
                        // r542: jolt the camera on a hard landing, scaled by impact
                        // speed (landings below the ~9 m/s threshold don't reach here).
                        if cam_shake_on {
                            let speed = (-prev_fall_vy).max(0.0);
                            // r566: heavier (taller) bodies land harder — scale the jolt by
                            // the stature's ground_shake (= 0 for a default character).
                            let impulse = ((speed - 8.0) / 22.0).clamp(0.0, 1.0)
                                * (1.0 + player_stature.ground_shake * 0.25).min(4.0);
                            shake_mag = shake_mag.max(impulse);
                        }
                    }
                    if landed && survival_mode {
                        let imp = unendlich_engine::sim::fall_impact::impact_for(
                            -prev_fall_vy,
                            unendlich_engine::sim::fall_impact::Body::humanoid(),
                        );
                        if imp.damage > 0.0 {
                            // r177: EARTH ATTUNEMENT (element Earth=imbue 0?,
                        // see element table: hybrid Wood0/Water1/Air2/Dust3/
                        // Fire4/Storm5 — Dust is the earth-line) halves
                        // fall damage: the mountain holds its own.
                        let earth_attuned = fg_imbue.element == 3 && fg_imbue.counter > 0.5;
                        let dmg_scale = if earth_attuned { 0.005 } else { 0.01 };
                        fg_app.vitality_mut().tick(1.0, (imp.damage * dmg_scale).min(0.5) * player_dmg_resist);
                            log::info!(
                                "[survival] fall impact: {:.0} dmg{} — vitality {:.0}%",
                                imp.damage,
                                if imp.knockdown { " (KNOCKDOWN)" } else { "" },
                                fg_app.vitality().energy * 100.0
                            );
                        }
                    }
                    prev_fall_vy = vy;
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
                // r561: auto-step height scales with stature (a giant steps over a wall;
                // = 1.1 for a default character).
                params.step_height = player_stature.step_height;
                // r566: wind resistance from stature (a heavier body drifts less).
                params.wind_resist = player_stature.wind_resist_mult;
                // r508: REALISTIC ARC — fall faster than you rise. A heavier descent gives the jump
                // an apex "hang" then a weighted drop, instead of the symmetric, linear-feeling
                // up-down of constant gravity. Out of water only (water uses the buoyancy model
                // above); terminal_velocity still caps the fall.
                if !feet_in_water && b.velocity[1] < 0.0 {
                    params.gravity *= 1.4;
                }

                // r476: detached-terrain check. Is the ground beneath the feet
                // streamed in yet? We probe the player's cell and one below.
                let fx = b.position[0].floor() as i32;
                let fy = b.position[1].floor() as i32;
                let fz = b.position[2].floor() as i32;
                let ground_ready = !renderer.ui_detached_terrain
                    || (chunk_manager.is_loaded_at(fx, fy, fz)
                        && chunk_manager.is_loaded_at(fx, fy - 1, fz));

                if !ground_ready {
                    // Terrain under the player isn't real yet. Don't freeze them —
                    // turn OFF gravity and zero the downward drift so they keep
                    // gliding forward at altitude while chunks stream in below,
                    // instead of plummeting through the void. (Horizontal velocity
                    // still integrates in step_body, so flight feels continuous.)
                    params.apply_gravity = false;
                    if b.velocity[1] < 0.0 {
                        b.velocity[1] = 0.0;
                    }
                    terrain_suspended = true;
                } else if terrain_suspended {
                    // Terrain just spawned under a gliding player. If they're now
                    // embedded in (or buried under) solid ground, settle them onto
                    // the real surface — the "replace the player when the chunk
                    // spawns and the position differs" reconcile.
                    if chunk_manager.block_at(fx, fy, fz).is_solid() {
                        if let Some(surf_y) = chunk_manager.surface_above(fx, fy, fz, 64) {
                            b.position[1] = surf_y as f32;
                            b.velocity[1] = 0.0;
                            log::info!(
                                "[detached] terrain spawned under player — settled to surface y={}",
                                surf_y
                            );
                        }
                    }
                    terrain_suspended = false;
                }
                // Always step (gravity flag set above) so horizontal flight is
                // continuous whether or not the ground has streamed in.
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
                    b.render_y() + player_eye_offset,
                    b.position[2],
                );
            }
        }

        // Drive the chunk manager with the player's world position.
        // Multi-interest-point ready: today we pass a single-element
        // slice; future split-screen will pass one per local player.
        //
        // During the menu/wizard the cinematic camera ORBITS the spawn — but the
        // spawn area is already loaded, so following the orbiting eye as the
        // streaming center just churns request_visible/gen/upload every frame (the
        // menu "chug" + FPS drop). Pin the streaming center to the spawn while the
        // menu is up; the camera still looks around, nothing new needs streaming.
        let stream_anchor = if renderer.menu_open || renderer.wizard_open {
            spawn_pos
        } else {
            local_player.view.position
        };
        let pos = stream_anchor;
        let t_mgr = Instant::now();
        // r239 (mesh-vision-sched): feed the camera yaw so the chunk manager
        // can build the player view cone for vision-driven mesh scheduling.
        #[cfg(feature = "mesh-vision-sched")]
        chunk_manager.set_player_view_yaw(local_player.view.yaw);
        chunk_manager.update_with_interest_points(&[
            (pos.x as i32, pos.y as i32, pos.z as i32),
        ]);
        let mgr_ms = t_mgr.elapsed().as_secs_f64() * 1e3;
        t_manager_update_us += (mgr_ms * 1000.0) as u64;
        if profile_on { prof.record("manager_update", mgr_ms); }

        // Advance simulation ticks (plant lifecycle, future leaf decay,
        // fluid flow, etc.) by this frame's real elapsed time. Internally
        // the scheduler accumulates and only fires handlers when a full
        // tick period (default 0.5s) has passed.
        let t_tick = Instant::now();
        _render_probe.set(unendlich_engine::thread_probe::Activity::TickSim);
        chunk_manager.tick_simulation(dt);
        _render_probe.set(unendlich_engine::thread_probe::Activity::Working);
        let tick_ms = t_tick.elapsed().as_secs_f64() * 1e3;
        t_tick_sim_us += (tick_ms * 1000.0) as u64;
        if profile_on { prof.record("tick_sim", tick_ms); }
        // r242: apply (deferred block/state edits landing in chunks) is the
        // dominant tick_sim spike on a loading wave — profile it separately so
        // its tail is visible vs the rest of the sim. last_apply_us is the
        // chunk_manager's own measurement of the apply pass just run.
        if profile_on { prof.record("apply", chunk_manager.last_apply_us() as f64 / 1000.0); }

        // ---- Ambient ecosystem: stream chunks in/out + tick roamers ----
        {
            use unendlich_engine::constants::CHUNK_SIZE;
            use unendlich_engine::item::CreatureKind;
            use unendlich_engine::sim::cognition::wander::Wander;
            use unendlich_engine::world::ecosystem::populate_region;

            let pp = stream_anchor; // pinned to spawn during the menu (see above)
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
                // r296: one WorldProber for this whole crossing's populate batch —
                // memoizes the deep per-column surface/biome scans so the biome and
                // spawn passes share one scan per column (and overlapping probe
                // areas across the batch's chunks reuse columns too). Fresh per
                // crossing; the world may have changed since the last one.
                let world_prober = std::cell::RefCell::new(
                    unendlich_engine::sim::world_prober::WorldProber::new(),
                );
                // r297: SEPARATE prober for flora — flora's surface probe range
                // (PROBE_MIN..PROBE_MAX) differs from the spawn/biome pass's
                // (probe_y_min..probe_y_max), and a column's cached surface is only
                // coherent for one range, so the two systems must not share a memo.
                let mut flora_prober = unendlich_engine::sim::world_prober::WorldProber::new();
                let mut flora_placed: usize = 0;
                let mut flora_tried: usize = 0;
                for c in &delta.to_populate {
                    // Flora for every newly-streamed chunk (trees, grass, flowers,
                    // ferns, mushrooms, cacti — same chain as the spawn patch, now
                    // world-wide). Placed BEFORE the roamer cap check so vegetation
                    // never starves when the creature cap is hit.
                    {
                        let fbx = c.0 * CHUNK_SIZE;
                        let fbz = c.1 * CHUNK_SIZE;
                        let flora = unendlich_engine::world::flora_blocks::region_flora_blocks_cached(
                            cpu, &mut flora_prober, fbx, fbz, CHUNK_SIZE, CHUNK_SIZE,
                        );
                        for (b, block) in flora {
                            flora_tried += 1;
                            if chunk_manager.set_block(b[0], b[1], b[2], block) {
                                flora_placed += 1;
                            }
                        }
                    }
                    if roamers.len() >= ROAMER_CAP {
                        continue; // cap creatures, but later chunks still get flora
                    }
                    let bx = c.0 * CHUNK_SIZE;
                    let bz = c.1 * CHUNK_SIZE;
                    let spawns = populate_region(
                        bx,
                        bz,
                        CHUNK_SIZE,
                        CHUNK_SIZE,
                        fg_world_seed,
                        STREAM_RARITY,
                        |x, z| {
                            // r296: surface scan via the shared WorldProber — the
                            // biome pass and the spawn pass below probe the SAME
                            // column, so this memo computes the 768-deep scan once
                            // and the spawn closure reuses it (and the biome itself
                            // is memoized). This is the ecosystem-stream fix for the
                            // ~9s region-crossing freeze (1.5M noise evals/chunk →
                            // one scan per column).
                            let sy = world_prober
                                .borrow_mut()
                                .surface_y_arc(cpu, x, z, probe_y_min, probe_y_max)
                                .unwrap_or(72);
                            world_prober.borrow_mut().biome_arc(cpu, x, sy, z)
                        },
                        |x, z| {
                            world_prober
                                .borrow_mut()
                                .surface_y_arc(cpu, x, z, probe_y_min, probe_y_max)
                                .unwrap_or(64)
                        },
                    );
                    for s in &spawns {
                        if roamers.len() >= ROAMER_CAP {
                            break;
                        }
                        // Observation: UNENDLICH_SPAWN_ARCHE turns every spawn into
                        // a picker body plan (e.g. the super-tall builds), bridged
                        // into a world creature; otherwise the ecology kind.
                        let wseed = (fg_world_seed)
                            ^ ((s.pos[0] as i32 as u32 as u64) << 20)
                            ^ (s.pos[2] as i32 as u32 as u64);
                        let (kind, mut cr) = if let Some(arche) = spawn_arche {
                            let bp = unendlich_engine::creature_design::EggSpec::chosen(arche, wseed).hatch();
                            (CreatureKind::Dragon, unendlich_engine::spawn::from_blueprint(&bp, s.pos))
                        } else {
                            (s.kind, unendlich_engine::spawn::spawn(s.kind, s.pos))
                        };
                        // r545 (Roll 2 / B3): same concurrent-dragon cap on the ecology
                        // stream path (dragons can come from the apportioned budget too,
                        // not just the request queue). UNENDLICH_MAX_DRAGONS (default 6).
                        if matches!(kind, CreatureKind::Dragon) {
                            let max_dragons = std::env::var("UNENDLICH_MAX_DRAGONS")
                                .ok()
                                .and_then(|s| s.parse::<usize>().ok())
                                .unwrap_or(6);
                            let dragon_count = roamers
                                .iter()
                                .filter(|r| matches!(r.kind, CreatureKind::Dragon))
                                .count();
                            if dragon_count >= max_dragons {
                                continue;
                            }
                        }
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
                            bond: unendlich_engine::sim::bond_tame::Bond::Wild,
                            trust: 0.0,
                            hp: 40,
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
                            species: None,
                            genome,
                            forage_memory: unendlich_engine::sim::forage_memory::ForageMemory::new(16),
                            leap: unendlich_engine::sim::leap::Leap::from_genes(ph.speed, ph.speed),
                            airborne: None,
                            posture: unendlich_engine::sim::posture::Posture::new(0.0),
                            stumble: unendlich_engine::sim::stumble::Stumble::new(),
                            flying: false,
                            flight_phase: (wseed % 30) as f32,
                            last_tick_frame: 0,
                            aggro: 0.0,
                            prev_behavior: 0,
                            combat_habit: 0.0,
                            disposition_cache: None,
                            graze_target: None,
                            graze_cell: (i32::MIN, i32::MIN),
                            graze_scan_tick: 0,
                        });
                    }
                }
                log::info!(
                    ">>> ECOSYSTEM STREAM: chunk ({}, {}) | +{} -{} | {} roamers | flora {}/{} blocks (0/0 on an already-vegetated crossing is normal; 0/N means set_block landed on unloaded chunks — frontier gap)",
                    pcx,
                    pcz,
                    delta.to_populate.len(),
                    delta.to_despawn.len(),
                    roamers.len(),
                    flora_placed,
                    flora_tried
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
            last_player_speed = player_speed; // r148
            let player_footstep = unendlich_engine::sim::sound::footstep_loudness(70, player_speed_milli, 20);
            let player_pos3 = [pp.x, pp.y, pp.z];

            // r141: PREDATOR-PREY — snapshot the herd for sim::predator_prey
            // (kind, position, size); predators chase prey decided per tick.
            let herd_snapshot: Vec<(u8, [f32; 3], f32)> = roamers
                .iter()
                .map(|r| {
                    let k = match r.kind {
                        unendlich_engine::item::CreatureKind::Dinosaur => 1u8,
                        unendlich_engine::item::CreatureKind::Dragon => 2u8,
                        _ => 0u8,
                    };
                    (k, r.creature.body.position, r.creature.mass)
                })
                .collect();
            let mut prey_killed: Vec<usize> = Vec::new();
            // r166: HIDING — sneaking inside tall flora conceals you:
            // creature SIGHT fails (hearing still works at reduced loudness,
            // so move slowly), dragon bending can't lock, dragon ears dull.
            let player_hidden = {
                let sneaking = bindings.is_action_pressed(GameAction::Sneak, &input);
                sneaking
                    && matches!(
                        chunk_manager.block_at(
                            pp.x.floor() as i32,
                            (pp.y - 0.4).floor() as i32,
                            pp.z.floor() as i32,
                        ),
                        BlockId::TallGrass | BlockId::Fern
                    )
            };
            roamer_tick = roamer_tick.wrapping_add(1);
            // r134: AI BUDGET (operator: mob lag) — distance tick-bands from
            // the REAL sim::ai_budget: distant brains run every 2/4/8 frames,
            // staggered by a per-individual id hash so full-ticks spread out.
            // Skipped creatures freeze for those frames (fine at 32+ blocks).
            let budget_viewer = local_player.view.position;
            let mut rest_log_budget = 1u8; // r134: at most ONE [rest] line per window
            // r248: localize the steady game_update cost. The r247 profile
            // showed game_update p50≈16.5ms with NO subsystem zone accounting
            // for it (creature_mesh 0.03ms, AI scans nanoscale, draw 1.9ms).
            // This per-creature loop (~1000 lines: observe/rest/grime/posture/
            // forage/dragon over ~200 creatures) is the prime remaining
            // suspect. Time it to confirm.
            let _t_roamer = Instant::now();
            // r294: fresh surface memo each tick (the world is mutated between ticks).
            if surface_cache_enabled {
                surface_cache.borrow_mut().clear();
            }
            // r283: localise the 17ms roamer_tick. Accumulate per-phase time
            // across all creatures into buckets, recorded after the loop:
            //   roamer_blockscan = the per-creature ground-ring + nav3d block_at
            //     storm (suspected always-on cost: every creature, every frame);
            //   roamer_herdscan  = the O(n^2) predator-prey + dragon-hunt herd
            //     scans (only hungry predators / dragons).
            let mut _t_blockscan_us: u64 = 0;
            let mut _t_herdscan_us: u64 = 0;
            // rNNN: WING BUFFET — snapshot the positions of flapping (airborne)
            // dragons BEFORE the mutable loop, so leaping creatures can be deflected
            // by their downwash without aliasing the roamer borrow. Empty when the
            // feature is off, so the per-creature cost is a single is_empty() check.
            let wing_buffet_sources: Vec<[f32; 3]> = if wing_buffet_enabled {
                roamers
                    .iter()
                    .filter(|r| {
                        matches!(r.kind, unendlich_engine::item::CreatureKind::Dragon)
                            && r.airborne.is_some()
                    })
                    .map(|r| r.creature.body.position)
                    .collect()
            } else {
                Vec::new()
            };
            for r in roamers.iter_mut() {
                // r265: catch-up count carried from the gate to the tick body.
                let mut mob_band_catch_up: u32 = 1;
                {
                    let p = r.creature.body.position;
                    let db = (((p[0] - budget_viewer.x).powi(2)
                        + (p[2] - budget_viewer.z).powi(2))
                        .sqrt()) as i32;
                    let band = unendlich_engine::sim::ai_budget::TickBand::from_distance(db);
                    let ent_id = r.flight_phase.to_bits() as u64;
                    // persistent (dragons) are exempt: flight arcs + breath
                    // windows need every frame; there are only a handful.
                    // r265: mob_band_catch_up carries the elapsed-tick count `n`
                    // from the mob_bands path into the tick body so movement/
                    // forage can integrate the whole skipped interval at once.
                    // 1 = ticked this frame with no backlog (the ai_budget path
                    // and Render band both leave it 1).
                    if !r.persistent {
                        let curfewed = if mob_bands_enabled {
                            // r263/r265: gate by the world::scheduler band of this
                            // mob's chunk (same bands as streaming), with TRUE
                            // per-roamer catch-up: ticks_since_last = frame_count -
                            // last_tick_frame, and when it fires we get n elapsed
                            // ticks to integrate (Render→1, Simulation→the gap,
                            // CatchUp→dormant).
                            let cs = unendlich_engine::constants::CHUNK_SIZE;
                            let mob_chunk = unendlich_engine::world::chunk::ChunkCoord::new(
                                (p[0].floor() as i32).div_euclid(cs),
                                (p[1].floor() as i32).div_euclid(cs),
                                (p[2].floor() as i32).div_euclid(cs),
                            );
                            let player_chunk = unendlich_engine::world::chunk::ChunkCoord::new(
                                (budget_viewer.x.floor() as i32).div_euclid(cs),
                                (budget_viewer.y.floor() as i32).div_euclid(cs),
                                (budget_viewer.z.floor() as i32).div_euclid(cs),
                            );
                            let sched_band = unendlich_engine::world::scheduler::band_for_chunk(
                                mob_chunk, player_chunk, &mob_band_params,
                            );
                            let divisor = sched_band.divisor(&mob_band_params);
                            let since_last = (frame_count as u64).saturating_sub(r.last_tick_frame);
                            match unendlich_engine::sim::mob_bands::mob_band_tick(divisor, since_last) {
                                Some(n) => {
                                    // fires: record this tick and carry n forward.
                                    r.last_tick_frame = frame_count as u64;
                                    mob_band_catch_up = n.max(1);
                                    false
                                }
                                None => true, // skip (backlog not yet reached, or dormant)
                            }
                        } else {
                            !unendlich_engine::sim::ai_budget::should_full_tick(
                                frame_count as u64, ent_id, band,
                            )
                        };
                        if curfewed {
                            continue; // brain curfewed this frame
                        }
                    }
                }
                let Roamer {
                    bond,
                    hp,
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
                    flying,
                    flight_phase,
                    kind,
                    aggro,
                    prev_behavior,
                    combat_habit,
                    genome,
                    disposition_cache,
                    graze_target,
                    graze_cell,
                    graze_scan_tick,
                    ..
                } = r;
                let self_xz = (cr.body.position[0], cr.body.position[2]);
                // Advance the forge idle clock. It was never ticked, so anim_time
                // stayed 0 for every live creature and the forge's idle animations
                // (breathing, wing beat, jaw, head idle-scan) were frozen — only
                // gaze + verlet wing physics moved. Wrap at a TAU multiple so f32
                // doesn't drift over a long session (continuous for integer freqs).
                cr.anim_time = (cr.anim_time + dt) % (std::f32::consts::TAU * 256.0);
                // r152: GRIME — the entity baker already shades faces by the
                // creature's GrimeState (belly-weighted); now it actually
                // accumulates. Dusty ground + movement -> dirt in the local
                // soil's tint; standing in water rinses it off.
                {
                    let gx = self_xz.0.floor() as i32;
                    let gz = self_xz.1.floor() as i32;
                    let gy = (cr.body.position[1] - 0.6).floor() as i32;
                    let ground = chunk_manager.block_at(gx, gy, gz);
                    let feet_wet = matches!(
                        chunk_manager.block_at(gx, gy + 1, gz),
                        BlockId::Water
                    );
                    let (soil, tint) = match ground {
                        BlockId::Dirt | BlockId::Grass => (0.7, [0.40, 0.30, 0.20]),
                        BlockId::Sand => (0.5, [0.76, 0.70, 0.50]),
                        BlockId::RedSandstone => (0.6, [0.62, 0.36, 0.24]),
                        _ => (0.1, [0.40, 0.30, 0.20]),
                    };
                    cr.grime.accumulate(&unendlich_engine::sim::creature_grime::GrimeSample {
                        soil_grime01: soil,
                        // wanderers are usually on the move; flat activity
                        // (steering writes position, so velocity is stale)
                        wetness01: if feet_wet { 1.0 } else { precip01 * 0.7 },
                        activity01: 0.5,
                        soil_tint: tint,
                        dt,
                    });
                }
                // r145: HEAD TRACKING — the REAL sim::dragon_observe gaze
                // model. The player is a stimulus (salience grows with
                // proximity and footstep loudness); the head smoothly tracks,
                // scans when idle, and the neck bend follows. The baker
                // already rotates head/neck voxels from gaze_yaw/gaze_pitch,
                // so creatures VISIBLY watch you walk past.
                {
                    use unendlich_engine::sim::dragon_observe as obs;
                    let pdx = player_xz.0 - self_xz.0;
                    let pdz = player_xz.1 - self_xz.1;
                    let d2 = pdx * pdx + pdz * pdz;
                    let mut stimulus: Option<obs::Stimulus> = None;
                    if d2 < 28.0 * 28.0 {
                        let mut bearing = pdz.atan2(pdx) - cr.yaw;
                        while bearing > std::f32::consts::PI { bearing -= std::f32::consts::TAU; }
                        while bearing < -std::f32::consts::PI { bearing += std::f32::consts::TAU; }
                        let sal = (1.0 - d2.sqrt() / 28.0).clamp(0.0, 1.0)
                            + (player_footstep as f32 * 0.005).min(0.5);
                        stimulus = Some(obs::Stimulus { bearing, salience: sal, dist2: d2 });
                    }
                    // r475 (perf): was Vec::with_capacity(1) — a heap alloc per creature per tick.
                    // A stack Option + slice avoids the allocator storm in the hot roamer loop.
                    let stim_slice: &[obs::Stimulus] = match &stimulus {
                        Some(s) => std::slice::from_ref(s),
                        None => &[],
                    };
                    let prev_gaze = obs::Gaze {
                        yaw: cr.gaze_yaw,
                        pitch: cr.gaze_pitch,
                        neck_bend: 0.6 * cr.gaze_yaw,
                    };
                    let stepg = obs::observe(
                        &obs::GazeParams::default(),
                        prev_gaze,
                        stim_slice,
                        cr.anim_time,
                        dt,
                    );
                    cr.gaze_yaw = stepg.gaze.yaw;
                    cr.gaze_pitch = stepg.gaze.pitch;
                }
                // r129: relax any swipe/flinch pose impulse back to neutral.
                cr.torso_bend *= 1.0 - (3.0 * dt).min(0.5);
                // r475 (perf): removed a SECOND, generic grime.accumulate that ran every creature
                // every tick on top of the r152 ground-aware one above — it double-counted grime
                // (2× wear rate) and doubled the grime cost in the hot roamer loop. The ground-aware
                // accumulate above is the intended, more correct feed.
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
                // r294: routed through the per-tick SurfaceCache — the first probe
                // of an (x,z) column runs the 64-deep noise scan, repeats within the
                // tick are HashMap hits. (Falls back to the direct scan if disabled.)
                let surface_at = |x: f32, z: f32| {
                    let (ix, iz) = (x.floor() as i32, z.floor() as i32);
                    if surface_cache_enabled {
                        surface_cache
                            .borrow_mut()
                            .get_or(ix, iz, || cpu.surface_y_at(ix, iz, cy - 48, cy + 16))
                    } else {
                        cpu.surface_y_at(ix, iz, cy - 48, cy + 16)
                    }
                };

                // --- Airborne: integrate a leap arc, then everything else waits ---
                if let Some((v, aloft0)) = *airborne {
                    let mut v = v;
                    // rNNN: WING BUFFET — a flapping dragon's downwash deflects this
                    // creature's leap arc (out + down). Sources precomputed above; skip
                    // near-coincident (the dragon itself). Additive impulse on v.
                    if !wing_buffet_sources.is_empty() {
                        let here = cr.body.position;
                        for &src in &wing_buffet_sources {
                            let dx = here[0] - src[0];
                            let dz = here[2] - src[2];
                            if dx * dx + dz * dz < 1.0 {
                                continue;
                            }
                            let f = unendlich_engine::sim::wing_buffet::buffet_force(
                                src, here, 24.0, 10.0, 0.5,
                            );
                            v[0] += f[0] * dt;
                            v[1] += f[1] * dt;
                            v[2] += f[2] * dt;
                        }
                    }
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
                // r300: player distance + perceived footstep loudness, computed ONCE
                // here and reused by both the hearing branch and the awareness blend
                // below (they each recomputed the identical value before).
                let player_dist = ((player_xz.0 - self_xz.0).powi(2)
                    + (player_xz.1 - self_xz.1).powi(2)).sqrt();
                let player_perc = unendlich_engine::sim::sound::perceived_loudness(
                    player_footstep, player_dist as i32, 0,
                );
                // Sight: from the creature's eye, looking along its heading.
                let eye = [self_xz.0, cr.body.position[1] + cr.body.half_extents[1], self_xz.1];
                let facing = [cr.yaw.cos(), 0.0, cr.yaw.sin()];
                let detection = unendlich_engine::sim::eyes::sees_in_fog(senses.eyes, frame_fog_sigma, eye, facing, player_pos3, 0.4, player_speed);
                let seen = detection.is_detected();
                let perceived: Option<(f32, f32)> = if seen && !player_hidden {
                    Some(player_xz) // sight pins the true position
                } else {
                    // Hearing: the player's footstep loudness perceived here
                    // (occlusion not yet modelled), gated by this creature's ears
                    // and the biome's ambient floor; then localized to a NOISY
                    // estimate it flees from.
                    let floor = unendlich_engine::sim::sound::biome_ambient_loudness(here_biome);
                    if senses.notices(player_perc, floor) {
                        let jseed = ((self_xz.0.floor() as i32 as u32 as u64) << 32)
                            | (self_xz.1.floor() as i32 as u32 as u64);
                        unendlich_engine::sim::localize::localize(
                            [self_xz.0, cr.body.position[1], self_xz.1],
                            &[([player_xz.0, pp.y, player_xz.1], player_perc)],
                            senses.hearing_threshold,
                            jseed,
                        )
                        .map(|e| (e.pos[0], e.pos[2]))
                    } else {
                        None
                    }
                };
                // r289/r300: express this creature's genome → disposition. The
                // genome is fixed for life, so this is a pure function with a
                // stable result — cache it on the roamer and reuse, instead of
                // re-deriving every tick. (The original per-tick call flagged
                // exactly this: "caching it on the roamer is the perf refinement".)
                // Gated: only filled when a consumer (POI / aggro / arbiter /
                // witness) is on. Identical value → behaviour unchanged.
                let disposition: Option<&unendlich_engine::sim::disposition::Disposition> =
                    if poi_enabled || aggro_enabled || arbiter_enabled || witness_enabled {
                        if disposition_cache.is_none() {
                            let gs = genome.genes();
                            let ent_seed = (gs[4].a.value.to_bits() as u64) << 32
                                ^ (gs[5].a.value.to_bits() as u64)
                                ^ (gs[0].b.value.to_bits() as u64);
                            *disposition_cache = Some(
                                unendlich_engine::sim::disposition::express_disposition(&*genome, ent_seed),
                            );
                        }
                        disposition_cache.as_ref()
                    } else {
                        None
                    };
                // Flight-initiation distance: a creature may *notice* the player far
                // off (and we could surface that as wariness later), but it only
                // BOLTS once the threat is within a flight distance. Without this,
                // fauna flee on first sight from ~48 blocks and you never see them
                // roam. Dragons hold their ground (no flight).
                // r289: genetic eyesight scales this range (sharper-eyed lineages
                // notice threats farther out); falls back to the constant otherwise.
                let flight_dist: f32 = disposition.map(|d| d.sight_range).unwrap_or(16.0);
                let threat: Option<(f32, f32)> = if *persist {
                    None // persistent (dragons): don't flee the player
                } else {
                    perceived.filter(|&(tx, tz)| {
                        (tx - self_xz.0).powi(2) + (tz - self_xz.1).powi(2) < flight_dist * flight_dist
                    })
                };

                // r266: graded awareness (sim::awareness_blend) from the same sense
                // channels — sight from the Detection tier, hearing from the
                // perceived footstep loudness vs the biome floor, smell unmodelled
                // (0). Logged as wariness; does not gate flee (yet).
                if awareness_enabled && !*persist {
                    let sight = match detection {
                        unendlich_engine::sim::eyes::Detection::Clear => 1.0,
                        unendlich_engine::sim::eyes::Detection::Motion => 0.5,
                        unendlich_engine::sim::eyes::Detection::Unseen => 0.0,
                    };
                    let hearing = (player_perc as f32 / 255.0).clamp(0.0, 1.0);
                    let wariness = unendlich_engine::sim::awareness_blend::awareness(sight, hearing, 0.0);
                    // sample-log so it's observable without flooding (one in ~256
                    // frames per noticing roamer).
                    if wariness > 0.15 && (frame_count & 0xFF) == 0 {
                        log::info!("[awareness] wariness={:.2} (sight={:.1} hearing={:.2})", wariness, sight, hearing);
                    }
                }

                // Flee the perceived threat (perception + flight distance already
                // gated, so a large radius: if it's a threat at all, it runs).
                // r265: integrate the catch-up interval — when the mob_bands gate
                // let a Simulation-band mob skip frames, mob_band_catch_up holds
                // how many ticks elapsed, so the wander advances by that whole
                // interval at once (scheduler time-homogeneity: a step over
                // n*dt == n steps of dt). 1 on the every-frame paths = unchanged.
                let tick_dt = dt * mob_band_catch_up as f32;
                let (mut rdx, mut rdz) = w.step_aware(tick_dt, self_xz, threat, 1000.0, 4.0);
                // r284: WITNESSED-ATTACK REACTION. Perception-driven, event-based —
                // the local replacement for omniscient herd knowledge. Scan the
                // (usually empty) attack-event buffer for an attack within witness
                // sight range; seeing a SAME-KIND herdmate attacked raises this
                // creature's aggro (toward help/defend — the arbiter then decides
                // approach vs flee by its boldness). Cheap: empty buffer = no-op,
                // and combat is rare. Genetics will later set the help/flee bias.
                if witness_enabled && !*persist && !attack_events.is_empty() {
                    const WITNESS_DIST2: f32 = 20.0 * 20.0; // a bit past flee range
                    let my_kind = match *kind {
                        unendlich_engine::item::CreatureKind::Dinosaur => 1u8,
                        unendlich_engine::item::CreatureKind::Dragon => 2u8,
                        _ => 0u8,
                    };
                    let mut witnessed = false;
                    for (vpos, vkind) in attack_events.iter() {
                        // only react to one's own kind (herdmates), and not to a
                        // self-report (the victim's own position ~= mine).
                        if *vkind != my_kind { continue; }
                        let d2 = (vpos[0] - self_xz.0).powi(2) + (vpos[2] - self_xz.1).powi(2);
                        if d2 > 0.25 && d2 < WITNESS_DIST2 {
                            witnessed = true;
                            break;
                        }
                    }
                    if witnessed {
                        // seeing kin attacked stokes aggression (capped); a bold
                        // creature moves to help, a timid one's flee dominates in
                        // the arbiter. r289: genetic boldness scales the boost — a
                        // bold lineage tips to help, a timid one barely reacts (and
                        // its flee wins). Habit further bends this within a life.
                        let boldness = disposition.map(|d| d.boldness).unwrap_or(0.5);
                        *aggro = (*aggro + 0.2 + 0.4 * boldness).min(1.0);
                    }
                }
                // r268: aggro response. Decay aggro over time + distance to the
                // threat (aggro_decay); when it's high and a threat is present,
                // flip the flee vector into an APPROACH so the creature closes in.
                if aggro_enabled {
                    let tdist = threat
                        .map(|(tx, tz)| ((tx - self_xz.0).powi(2) + (tz - self_xz.1).powi(2)).sqrt())
                        .unwrap_or(64.0);
                    *aggro = unendlich_engine::sim::aggro_decay::decay(*aggro, tick_dt, tdist);
                    // r289: a hostile lineage never relaxes below its genetic floor.
                    if let Some(d) = disposition {
                        *aggro = unendlich_engine::sim::disposition::aggro_with_floor(*aggro, d.aggression_base);
                    }
                    // r269: when the arbiter is on, IT decides fight-vs-flee from
                    // scored drives (supersedes the raw aggro>0.5 threshold).
                    let approach = if arbiter_enabled {
                        use unendlich_engine::sim::behavior_arbiter::Behavior;
                        // r269: behavioral memory. Habit decays slowly every tick
                        // (a tendency fades without reinforcement); it then biases
                        // EFFECTIVE aggression upward — a battle-hardened animal
                        // stays ready to fight. This is the per-individual learned
                        // layer that runs against the population-genetic trend.
                        *combat_habit = (*combat_habit - 0.02 * tick_dt).max(0.0);
                        let eff_aggression = (*aggro).max(*combat_habit * 0.8);
                        let tdn = threat.map(|(tx, tz)| {
                            let d = ((tx - self_xz.0).powi(2) + (tz - self_xz.1).powi(2)).sqrt();
                            (1.0 - (d / flight_dist)).clamp(0.0, 1.0) // nearer = higher threat
                        }).unwrap_or(0.0);
                        let drives = if poi_enabled {
                            // r287: build the perceived-POI list and score it. The
                            // player/threat is a Foe POI; nearby flora a Food POI.
                            // poi::to_drives fills threat from the foe POI (weighted
                            // by this creature's PoiWeights — default until the
                            // genome loci land) and passes body state through.
                            use unendlich_engine::sim::poi;
                            // r289: express this creature's genome → disposition, and
                            // score POIs with ITS weights (a cautious lineage treats
                            // foes as more salient, etc.) instead of the default. The
                            // r289: use the per-tick disposition expressed above
                            // (poi_enabled guarantees it's Some here).
                            let pw = disposition.map(|d| d.poi_weights).unwrap_or_default();
                            let mut pois: Vec<poi::Poi> = Vec::new();
                            if let Some((tx, tz)) = threat {
                                pois.push(poi::Poi {
                                    pos: [tx, cr.body.position[1], tz],
                                    kind: poi::PoiKind::Foe,
                                    salience: 1.0,
                                });
                            }
                            let body = poi::Body {
                                hunger: (1.0 - vitality.energy).clamp(0.0, 1.0),
                                fatigue: 0.0,
                                aggression: eff_aggression,
                                health_frac: (*hp as f32 / 40.0).clamp(0.0, 1.0),
                            };
                            poi::to_drives(
                                &pois,
                                [self_xz.0, cr.body.position[1], self_xz.1],
                                &pw,
                                flight_dist,
                                body,
                                tdn, // a directly-seen threat also raises threat (max)
                            )
                        } else {
                            unendlich_engine::sim::behavior_arbiter::Drives {
                                threat: tdn,
                                hunger: (1.0 - vitality.energy).clamp(0.0, 1.0),
                                fatigue: 0.0, // (rest_cycle wiring is a later refinement)
                                aggression: eff_aggression,
                                health_frac: (*hp as f32 / 40.0).clamp(0.0, 1.0),
                            }
                        };
                        // decode prev_behavior for hysteresis (don't flip-flop on near-ties)
                        let prev = match *prev_behavior {
                            1 => Some(Behavior::Rest),
                            2 => Some(Behavior::Forage),
                            3 => Some(Behavior::Patrol),
                            4 => Some(Behavior::Fight),
                            5 => Some(Behavior::Flee),
                            _ => None,
                        };
                        let choice = unendlich_engine::sim::behavior_arbiter::select(&drives, prev, 0.15);
                        // store the choice back for next tick's hysteresis
                        *prev_behavior = match choice.behavior {
                            Behavior::Rest => 1,
                            Behavior::Forage => 2,
                            Behavior::Patrol => 3,
                            Behavior::Fight => 4,
                            Behavior::Flee => 5,
                        };
                        let fighting = matches!(choice.behavior, Behavior::Fight);
                        // committing to a fight REINFORCES the combat habit (learning)
                        if fighting {
                            *combat_habit = (*combat_habit + 0.04 * tick_dt).min(1.0);
                        }
                        fighting
                    } else {
                        *aggro > 0.5 && threat.is_some()
                    };
                    if approach && threat.is_some() {
                        // approach instead of flee (negate the avoidance vector)
                        rdx = -rdx;
                        rdz = -rdz;
                    }
                }
                // r146: WHISKERS — the creature FEELS ahead with a touch ray
                // (REAL sim::touch_rays over solid block_at); a contact within
                // 2 blocks bends the heading away proportionally to proximity.
                {
                    let hd = (rdx * rdx + rdz * rdz).sqrt();
                    if hd > 0.05 {
                        let dirn = [rdx / hd, 0.0, rdz / hd];
                        let eye = [
                            self_xz.0,
                            cr.body.position[1] + 0.6,
                            self_xz.1,
                        ];
                        let contact = unendlich_engine::sim::touch_rays::feel(
                            eye,
                            dirn,
                            2.0,
                            0.5,
                            &|p: [f32; 3]| {
                                !matches!(
                                    chunk_manager.block_at(
                                        p[0].floor() as i32,
                                        p[1].floor() as i32,
                                        p[2].floor() as i32,
                                    ),
                                    BlockId::Air | BlockId::Water
                                        | BlockId::TallGrass | BlockId::Fern
                                        | BlockId::Flower
                                )
                            },
                        );
                        if contact.hit && contact.distance < 2.0 {
                            let avoid = 1.0 - contact.distance / 2.0;
                            // rotate heading ~90° away, blended by proximity
                            let (px, pz) = (-dirn[2], dirn[0]);
                            rdx = rdx * (1.0 - avoid) + px * hd * avoid;
                            rdz = rdz * (1.0 - avoid) + pz * hd * avoid;
                        }
                    }
                }
                // r144: GRAZING — a hungry herbivore (Fauna) consults the REAL
                // sim::feeding_select::pick over nearby flora blocks (probed
                // ring) and walks to the chosen one; on contact it EATS the
                // block (set_block Air) and feeds. Meadows visibly thin.
                let _t_bs = Instant::now();
                if matches!(
                    *kind,
                    unendlich_engine::item::CreatureKind::Fauna
                ) && vitality.energy < 0.6
                {
                    use unendlich_engine::sim::feeding_select as fsel;
                    let cx = self_xz.0.floor() as i32;
                    let cz = self_xz.1.floor() as i32;
                    let cy = cr.body.position[1].floor() as i32;
                    // r300: decide whether to run the (expensive) ring scan. Gated:
                    // only when there's no current target AND the creature moved to a
                    // new cell or the cadence is due — a stationary grazer's larder
                    // doesn't change, and one already walking to a target shouldn't
                    // keep re-scanning. Ungated: scan every tick (original behaviour).
                    let do_scan = if graze_gate_enabled {
                        let moved = (cx, cz) != *graze_cell;
                        let due = roamer_tick.wrapping_sub(*graze_scan_tick) >= GRAZE_SCAN_INTERVAL;
                        // Acquire when there's no target (on move or cadence); also
                        // re-scan if a target has been held for an unusually long time
                        // without being reached (unreachable pick → give up and re-pick),
                        // so a creature can't fixate forever on a blocked-off block.
                        let stale_target = graze_target.is_some()
                            && roamer_tick.wrapping_sub(*graze_scan_tick) >= GRAZE_SCAN_INTERVAL * 8;
                        (graze_target.is_none() && (moved || due)) || stale_target
                    } else {
                        true
                    };
                    if do_scan {
                        *graze_scan_tick = roamer_tick;
                        *graze_cell = (cx, cz);
                        let ring: [(i32, i32); 6] =
                            [(2, 0), (-2, 1), (0, 2), (1, -2), (3, 2), (-2, -3)];
                        let mut foods: Vec<fsel::Food> = Vec::with_capacity(6);
                        let mut spots: Vec<(i32, i32, i32)> = Vec::with_capacity(6);
                        for (ox, oz) in ring {
                            let (bx, bz) = (cx + ox, cz + oz);
                            for dy in -2..=2 {
                                let by = cy + dy;
                                let f = match chunk_manager.block_at(bx, by, bz) {
                                    BlockId::TallGrass => fsel::Food::Grass,
                                    BlockId::Fern => fsel::Food::Grass,
                                    BlockId::Flower => fsel::Food::Fruit,
                                    BlockId::OakLeaves | BlockId::BirchLeaves => fsel::Food::Leaf,
                                    _ => continue,
                                };
                                foods.push(f);
                                spots.push((bx, by, bz));
                                break;
                            }
                        }
                        *graze_target = fsel::pick(
                            &foods,
                            fsel::Diet::Herbivore,
                            1.0 - vitality.energy,
                        )
                        .map(|pick_i| spots[pick_i]);
                    }
                    // Act on the remembered target every tick: walk to it, or eat it
                    // on contact. On eat (or if it's gone — set_block fails) clear the
                    // target so the next eligible tick re-scans for a fresh one.
                    if let Some((bx, by, bz)) = *graze_target {
                        let gdx = bx as f32 + 0.5 - self_xz.0;
                        let gdz = bz as f32 + 0.5 - self_xz.1;
                        let gd = (gdx * gdx + gdz * gdz).sqrt();
                        if gd > 1.2 {
                            rdx = gdx / gd * 1.4;
                            rdz = gdz / gd * 1.4;
                        } else if chunk_manager.set_block(bx, by, bz, BlockId::Air) {
                            vitality.feed(0.4);
                            *graze_target = None;
                            if frame_count % 300 == 0 {
                                log::info!("[graze] Fauna ate flora at ({},{},{})", bx, by, bz);
                            }
                        } else {
                            *graze_target = None;
                        }
                    }
                }
                // r143: TAMED creatures heel — follow at a respectful 4 blocks,
                // overriding wander (rest/flee still apply via earlier gates).
                if matches!(*bond, unendlich_engine::sim::bond_tame::Bond::Tamed) {
                    let fdx = player_xz.0 - self_xz.0;
                    let fdz = player_xz.1 - self_xz.1;
                    let fd = (fdx * fdx + fdz * fdz).sqrt();
                    if fd > 4.0 {
                        // r154: NAV3D — the engine's 27-cell steering kernel:
                        // sample the 3x3x3 occupancy around the companion and
                        // let step3d bend the goal direction around blockers.
                        let cx = self_xz.0.floor() as i32;
                        let cyc = cr.body.position[1].floor() as i32;
                        let czc = self_xz.1.floor() as i32;
                        let mut blocked = [false; 27];
                        let mut bi = 0;
                        for dy in -1..=1 {
                            for dz in -1..=1 {
                                for dx in -1..=1 {
                                    blocked[bi] = !matches!(
                                        chunk_manager.block_at(cx + dx, cyc + dy, czc + dz),
                                        BlockId::Air | BlockId::Water
                                            | BlockId::TallGrass | BlockId::Fern
                                            | BlockId::Flower
                                    );
                                    bi += 1;
                                }
                            }
                        }
                        let dirv = unendlich_engine::sim::nav3d::step3d(
                            &blocked,
                            [fdx / fd, 0.0, fdz / fd],
                            unendlich_engine::sim::nav3d::Medium::Air,
                            0.0,
                        );
                        rdx = dirv[0] * 2.4;
                        rdz = dirv[2] * 2.4;
                    } else {
                        rdx = 0.0;
                        rdz = 0.0;
                    }
                }
                // r144: COMBAT STANCE — a WOUNDED creature (took player damage)
                // adopts the engine's combat stance: it maintains a standoff
                // distance from you (backing away if you close in) and leans
                // to shield the hurt side — a visible wounded posture.
                if *hp < 40
                    && *hp > 0
                    && !matches!(*bond, unendlich_engine::sim::bond_tame::Bond::Tamed)
                {
                    let pdx = player_xz.0 - self_xz.0;
                    let pdz = player_xz.1 - self_xz.1;
                    let pdist = (pdx * pdx + pdz * pdz).sqrt();
                    if pdist < 24.0 {
                        let st = unendlich_engine::sim::combat_stance::combat_stance(
                            unendlich_engine::sim::combat_stance::CreatureState {
                                overall_hp: *hp as f32,
                                max_hp: 40.0,
                                left_hp: *hp as f32 * 0.5,
                                right_hp: *hp as f32 * 0.5,
                                vital_exposure: 0.3,
                                threat: (1.0 - pdist / 24.0).clamp(0.0, 1.0),
                                base_distance: 8.0,
                                melee_distance: 1.6,
                            },
                        );
                        cr.torso_bend = (cr.torso_bend + st.lean * 0.4).clamp(-1.0, 1.0);
                        if pdist < st.standoff && pdist > 0.1 {
                            // back away to standoff range
                            rdx = -pdx / pdist * 2.0;
                            rdz = -pdz / pdist * 2.0;
                        }
                    }
                }
                _t_blockscan_us += _t_bs.elapsed().as_micros() as u64;
                // r141: PREDATOR-PREY — a hungry Dinosaur consults the REAL
                // sim::predator_prey::decide over the herd snapshot and chases
                // the chosen prey; within 1.5 blocks the prey dies (deferred
                // removal), drops meat, and the predator feeds.
                let _t_hs = Instant::now();
                if matches!(*kind, unendlich_engine::item::CreatureKind::Dinosaur)
                    && vitality.energy < 0.7
                {
                    use unendlich_engine::sim::predator_prey as ppm;
                    let mypos = cr.body.position;
                    let me = ppm::Me { kind: 1, size: cr.mass, hunger: 1.0 - vitality.energy };
                    let others: Vec<ppm::Other> = herd_snapshot
                        .iter()
                        .enumerate()
                        .filter(|(oi, (ok, op, _))| {
                            *ok == 0
                                && !prey_killed.contains(oi)
                                && (op[0] - mypos[0]).abs() < 48.0
                                && (op[2] - mypos[2]).abs() < 48.0
                        })
                        .map(|(oi, (ok, op, osz))| ppm::Other {
                            idx: oi,
                            kind: *ok,
                            dist: ((op[0] - mypos[0]).powi(2) + (op[2] - mypos[2]).powi(2)).sqrt(),
                            size: *osz,
                            edible_by_me: true,
                            threat_to_me: false,
                        })
                        .collect();
                    if let ppm::Act::Hunt(prey_idx) = ppm::decide(&me, &others) {
                        let (_, tp, _) = herd_snapshot[prey_idx];
                        let ddx = tp[0] - mypos[0];
                        let ddz = tp[2] - mypos[2];
                        let dd = (ddx * ddx + ddz * ddz).sqrt();
                        if dd > 1.5 {
                            // chase overrides wander heading at sprint pace
                            rdx = ddx / dd * 3.2;
                            rdz = ddz / dd * 3.2;
                        } else {
                            prey_killed.push(prey_idx);
                            vitality.feed(0.5);
                            log::info!("[ecology] Dinosaur brought down prey at ({:.0},{:.0})", tp[0], tp[2]);
                        }
                    }
                }
                _t_herdscan_us += _t_hs.elapsed().as_micros() as u64;
                // r133: REST CYCLE — sim::rest_cycle decides per-species sleep:
                // Diurnal fauna bed down at night, Nocturnal listeners by day,
                // Crepuscular at the extremes, dragons (Cathemeral) rarely.
                // Threat overrides rest; fatigue = hunger here.
                if threat.is_none() {
                    let rhythm = unendlich_engine::sim::rest_cycle::rhythm_for(*kind);
                    let fatigue = 1.0 - vitality.energy.clamp(0.0, 1.0);
                    // r156: storms drive shelter-seeking — precipitation
                    // biases the urge so the herd hunkers down in the rain.
                    if unendlich_engine::sim::rest_cycle::wants_rest(
                        rhythm, sky_state.day_phase(), fatigue, 0.3 * precip01,
                    ) {
                        rdx = 0.0;
                        rdz = 0.0;
                        if frame_count % 1800 == 0 && rest_log_budget > 0 {
                            rest_log_budget -= 1; // r134: one line per window, not per creature
                            log::info!("[rest] {:?} is resting ({:?}, day_phase {:.2})",
                                kind, rhythm, sky_state.day_phase());
                        }
                    }
                }
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
                // r503: LARGE-BODY LOCOMOTION. A big body can't slide straight to the
                // desired direction and pivot in place (the "fixed-point turn" /
                // moonwalk). It moves ALONG its current facing, carving an arc: it
                // slows as it turns and accelerates once aligned. `axis_bias` scales
                // with body size, so dragons fully carve while small/agile creatures
                // keep the responsive step (bias≈0). Applied to the step direction +
                // (below) the turn rate.
                let body_len = cr.body.half_extents[0].max(cr.body.half_extents[2]);
                let axis_bias = ((body_len - 1.0) / 4.0).clamp(0.0, 1.0);
                let dyaw = {
                    use std::f32::consts::{PI, TAU};
                    (desired - cr.yaw + PI).rem_euclid(TAU) - PI
                };
                let align = dyaw.cos().max(0.0); // 1 facing goal, 0 perpendicular/behind
                let move_heading = cr.yaw + dyaw * (1.0 - axis_bias);
                let eff_step = step * (1.0 - axis_bias * (1.0 - align)).max(0.2);
                let (dx, dz) = sighted_step(self_xz, move_heading, eff_step, ROAMER_HP, desperation, &surface_at);
                if dx != 0.0 || dz != 0.0 {
                    cr.body.position[0] += dx;
                    cr.body.position[2] += dz;
                    cr.gait_phase = (cr.gait_phase + (dx * dx + dz * dz).sqrt() * 2.5) % std::f32::consts::TAU;
                    if !*flying { if let Some(t) = surface_at(cr.body.position[0], cr.body.position[2]) {
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
                    } }
                    // It wants to face where it stepped. Fleeing OR goal-pathing
                    // turns crisply; idle wandering uses the detent + nonlinear map
                    // so it doesn't vacillate or look robotic.
                    // r503: turn toward the GOAL (desired), not the body-axis step
                    // direction — so a large body curves its heading toward the goal
                    // while it moves along its axis, carving an arc. For small bodies
                    // (axis_bias≈0) desired ≈ the step direction, so behaviour is the
                    // same as before.
                    w.heading = desired;
                    // r503: large bodies turn slower (wider arc) — scale the per-tick
                    // turn cap down by body size so a dragon can't spin on a dime.
                    let max_step = *turn_rate * dt / (1.0 + axis_bias * 1.5);
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
                    if !*flying { if let Some(f) = surface_at(self_xz.0, self_xz.1) {
                        let target_y = cr.grounded_y((f + 1) as f32);
                        let gap = target_y - cr.body.position[1];
                        if gap.abs() > 2.0 {
                            cr.body.position[1] = target_y;
                        } else {
                            cr.body.position[1] += gap * (dt * 12.0).min(1.0);
                        }
                    } }
                }

                // --- r119: DRAGON SOARING --- a periodic flight cycle so dragons
                // actually take to the air (and the r117 wing poses are visible).
                // Grounded ~12s -> climb to ~16 blocks over terrain, bank in a wide
                // circle ~25s -> descend and land. Per-individual phase offsets
                // desynchronize the flock. Sets body.velocity so forge_to_vertices
                // picks Beating (climb/cruise) and Flared (descent) wing poses.
                if matches!(*kind, unendlich_engine::item::CreatureKind::Dragon) {
                    *flight_phase += dt;
                    let cycle = *flight_phase % 45.0;
                    let want_air = cycle > 12.0 && cycle < 37.0;
                    let surf = surface_at(cr.body.position[0], cr.body.position[2])
                        .map(|s| (s + 1) as f32)
                        .unwrap_or(cr.body.position[1]);
                    if want_air && !*flying {
                        *flying = true;
                        {
                            // r133: announce this individual's attunement (the
                            // breath element) so the log reads per-dragon.
                            let g = unendlich_engine::sim::dragon_archetype::Genome::from_seed(
                                flight_phase.to_bits() as u64,
                            );
                            let e = unendlich_engine::sim::breath_attack::dominant_element(&g) as usize;
                            log::info!(
                                "[dragon] TAKEOFF at ({:.0}, {:.0}, {:.0}) — {}-attuned",
                                cr.body.position[0], cr.body.position[1], cr.body.position[2],
                                unendlich_engine::sim::dragon_archetype::ELEMENT_NAMES
                                    [e % unendlich_engine::sim::dragon_archetype::NUM_ELEMENTS],
                            );
                        }
                    }
                    // r150: DRAGON ACTIVITY — the engine's daily-life state
                    // machine (sleep at night per its rhythm; noise wakes it;
                    // hunger pushes it to forage). Sleeping dragons LAND and
                    // do not bend; hunting dragons run the REAL dragon_hunt
                    // stalk -> stoop -> strike machine against the herd.
                    let ent_id_act = flight_phase.to_bits() as u64;
                    let dragon_asleep = {
                        use unendlich_engine::sim::dragon_activity as dact;
                        let pp3 = local_player.view.position;
                        let ddx = pp3.x - cr.body.position[0];
                        let ddz = pp3.z - cr.body.position[2];
                        let pd2 = ddx * ddx + ddz * ddz;
                        let mut bearing_p = ddz.atan2(ddx) - cr.yaw;
                        while bearing_p > std::f32::consts::PI { bearing_p -= std::f32::consts::TAU; }
                        while bearing_p < -std::f32::consts::PI { bearing_p += std::f32::consts::TAU; }
                        let prev_act = session.dragon_activities
                            .get(&ent_id_act)
                            .copied()
                            .unwrap_or_else(dact::ActivityState::fresh);
                        let was_asleep = matches!(prev_act.activity, dact::Activity::Sleeping);
                        let heard = unendlich_engine::sim::dragon_hearing::react(
                            unendlich_engine::sim::sound_masking::Ears {
                                base_range: 40.0,
                                sensitivity: 1.2,
                            },
                            &[unendlich_engine::sim::dragon_hearing::Noise {
                                sound: unendlich_engine::sim::sound_masking::Sound {
                                    loudness: player_footstep as f32
                                        * if player_hidden { 0.25 } else { 1.0 },
                                    freq01: 0.4,
                                },
                                bearing: bearing_p,
                                dist2: pd2,
                                occlusion: 0.0,
                            }],
                            6.0,
                            was_asleep,
                        );
                        let senses = dact::Senses {
                            threat: (1.0 - pd2.sqrt() / 48.0).clamp(0.0, 1.0),
                            aggression: 0.5,
                            health_frac: 1.0,
                            food_in_reach: false,
                            // activity convention: 0 = midnight (sky: 0 = sunrise)
                            day_phase: (sky_state.day_phase() + 0.25) % 1.0,
                            heard,
                        };
                        let next = dact::step(
                            prev_act,
                            unendlich_engine::sim::rest_cycle::rhythm_for(
                                unendlich_engine::item::CreatureKind::Dragon,
                            ),
                            &senses,
                            dt,
                        );
                        if matches!(next.activity, dact::Activity::Sleeping) && !was_asleep {
                            log::info!("[dragon] settling to SLEEP (day_phase {:.2})", sky_state.day_phase());
                        }
                        if was_asleep && heard.should_wake {
                            log::info!("[dragon] your noise WOKE the dragon");
                        }
                        session.dragon_activities.insert(ent_id_act, next);
                        matches!(next.activity, dact::Activity::Sleeping)
                    };
                    if !dragon_asleep && !*flying {
                        // r152: RELAUNCH — the engine's mode machine decides
                        // when a grounded dragon returns to the air (Alert +
                        // threat / out-of-reach target). Applied one-way:
                        // Ground->Air only, so soaring behavior is untouched.
                        use unendlich_engine::sim::dragon_mode as dmode;
                        let act = session.dragon_activities
                            .get(&ent_id_act)
                            .map(|a| a.activity)
                            .unwrap_or(unendlich_engine::sim::dragon_activity::Activity::Idle);
                        let pp4 = local_player.view.position;
                        let ddx = pp4.x - cr.body.position[0];
                        let ddz = pp4.z - cr.body.position[2];
                        let d2m = ddx * ddx + ddz * ddz;
                        let stepm = dmode::decide(
                            dmode::ModeState::ground(),
                            &dmode::ModeSenses {
                                activity: act,
                                threat: (1.0 - d2m.sqrt() / 32.0).clamp(0.0, 1.0),
                                target_above: pp4.y > cr.body.position[1] + 4.0,
                                target_below: false,
                                target_dist2: d2m,
                                airborne_threat: false,
                                stamina01: 0.8,
                            },
                            24.0,
                            0.0,
                            dt,
                        );
                        if matches!(stepm.state.mode, dmode::MovementMode::Air) {
                            *flying = true;
                            log::info!("[dragon] takes to the air again");
                        }
                    }
                    if dragon_asleep {
                        *flying = false; // a sleeping dragon lands
                    } else if vitality.energy < 0.6 && *flying {
                        // r150: DRAGON HUNT — stalk the nearest Fauna from the
                        // snapshot; a stoop dives the dragon onto it; a strike
                        // in range makes a kill (same deferred-removal path as
                        // predator_prey) and the dragon FEEDS.
                        use unendlich_engine::sim::dragon_hunt as dh;
                        let mypos = cr.body.position;
                        let mut nearest: Option<(usize, f32, [f32; 3])> = None;
                        for (oi, (ok, op, _)) in herd_snapshot.iter().enumerate() {
                            if *ok != 0 || prey_killed.contains(&oi) { continue; }
                            let dx2 = (op[0] - mypos[0]).powi(2) + (op[2] - mypos[2]).powi(2);
                            if dx2 < 64.0 * 64.0 && nearest.map_or(true, |(_, d, _)| dx2 < d) {
                                nearest = Some((oi, dx2, *op));
                            }
                        }
                        if let Some((pi, pd2, ppos)) = nearest {
                            let mut bearing = (ppos[2] - mypos[2]).atan2(ppos[0] - mypos[0]) - cr.yaw;
                            while bearing > std::f32::consts::PI { bearing -= std::f32::consts::TAU; }
                            while bearing < -std::f32::consts::PI { bearing += std::f32::consts::TAU; }
                            let rel_h = mypos[1] - ppos[1];
                            let prev_h = session.dragon_hunts
                                .get(&ent_id_act)
                                .copied()
                                .unwrap_or_else(dh::HuntState::idle);
                            let stepd = dh::step(
                                prev_h,
                                &dh::HuntSenses {
                                    prey: dh::Prey {
                                        detected: true,
                                        bearing,
                                        dist2: pd2,
                                        rel_height: -rel_h,
                                        fleeing: false,
                                    },
                                    hunger01: 1.0 - vitality.energy,
                                    stamina01: 0.8,
                                    airborne: true,
                                    in_strike_range: pd2 < 16.0 && rel_h < 5.0,
                                },
                                dt,
                            );
                            let entering_stoop = matches!(stepd.state.phase, dh::HuntPhase::Stoop)
                                && !matches!(prev_h.phase, dh::HuntPhase::Stoop);
                            if entering_stoop {
                                log::info!("[dragon] STOOPS on the herd");
                            }
                            if stepd.wants_stoop || stepd.wants_close {
                                // r154: the dive is steered by nav3d — the
                                // 27-cell kernel bends the glide around
                                // terrain; altitude error drives the descent.
                                let hd = pd2.sqrt().max(0.1);
                                let dcx = mypos[0].floor() as i32;
                                let dcy = mypos[1].floor() as i32;
                                let dcz = mypos[2].floor() as i32;
                                let mut blocked = [false; 27];
                                let mut bi = 0;
                                for dy in -1..=1 {
                                    for dz in -1..=1 {
                                        for dx in -1..=1 {
                                            blocked[bi] = !matches!(
                                                chunk_manager.block_at(dcx + dx, dcy + dy, dcz + dz),
                                                BlockId::Air
                                            );
                                            bi += 1;
                                        }
                                    }
                                }
                                let alt_err = if stepd.wants_stoop && rel_h > 2.0 {
                                    -1.0 // descend onto the prey
                                } else {
                                    0.0
                                };
                                let dirv = unendlich_engine::sim::nav3d::step3d(
                                    &blocked,
                                    [
                                        (ppos[0] - mypos[0]) / hd,
                                        0.0,
                                        (ppos[2] - mypos[2]) / hd,
                                    ],
                                    unendlich_engine::sim::nav3d::Medium::Air,
                                    alt_err,
                                );
                                let rate = if stepd.wants_stoop { 7.0 } else { 3.0 };
                                cr.body.position[0] += dirv[0] * rate * dt;
                                cr.body.position[2] += dirv[2] * rate * dt;
                                cr.body.position[1] += dirv[1] * rate * 0.65 * dt;
                            }
                            if stepd.wants_strike && pd2 < 9.0 {
                                prey_killed.push(pi);
                                vitality.feed(0.6);
                                log::info!("[dragon] STRIKE — it takes a Fauna from the herd and feeds");
                            }
                            session.dragon_hunts.insert(ent_id_act, stepd.state);
                        }
                    }
                    if !dragon_asleep {
                    // r138: DRAGON BENDING AI — the dragon charges a technique
                    // while its eyes hold the player (visible tell = the wind-up
                    // window), then releases a genetics-chosen arc with aim
                    // error. Runs every frame for flying dragons (budget-exempt).
                    {
                        use unendlich_engine::sim::dragon_bending_ai as dba;
                        let ent_id = flight_phase.to_bits() as u64;
                        let pp2 = local_player.view.position;
                        let dx = pp2.x - cr.body.position[0];
                        let dz = pp2.z - cr.body.position[2];
                        let d2 = dx * dx + dz * dz + (pp2.y - cr.body.position[1]).powi(2);
                        let bearing = {
                            let mut b = dz.atan2(dx) - cr.yaw;
                            while b > std::f32::consts::PI { b -= std::f32::consts::TAU; }
                            while b < -std::f32::consts::PI { b += std::f32::consts::TAU; }
                            b
                        };
                        // r147: alertness from the REAL dragon_hearing — a
                        // heard footstep startles and sharpens the aim; move
                        // quietly and its arcs go wide.
                        let hearing = unendlich_engine::sim::dragon_hearing::react(
                            unendlich_engine::sim::sound_masking::Ears {
                                base_range: 40.0,
                                sensitivity: 1.2,
                            },
                            &[unendlich_engine::sim::dragon_hearing::Noise {
                                sound: unendlich_engine::sim::sound_masking::Sound {
                                    // r166: the grass muffles you too
                                    loudness: player_footstep as f32
                                        * if player_hidden { 0.25 } else { 1.0 },
                                    freq01: 0.4,
                                },
                                bearing,
                                dist2: d2,
                                occlusion: 0.0,
                            }],
                            6.0,
                            false,
                        );
                        let perception = dba::Perception {
                            has_target: d2 < 32.0 * 32.0
                                && bearing.abs() < 1.2
                                && !player_hidden, // r166: the grass hides you
                            target_bearing: bearing,
                            target_dist2: d2,
                            willing: if survival_mode { 0.8 } else { 0.35 },
                            alertness01: (0.35 + hearing.startle01 * 0.6).clamp(0.0, 1.0),
                        };
                        let g2 = unendlich_engine::sim::dragon_archetype::Genome::from_seed(ent_id);
                        let fields2 = unendlich_engine::sim::affinity_access::Fields {
                            h: 0.5, r: 0.5, w: 0.4, p: 0.6, l: 0.3, s: 0.5, b: 0.6,
                        };
                        let prev = session.dragon_bend_intents
                            .get(&ent_id)
                            .copied()
                            .unwrap_or_else(dba::BendIntent::idle);
                        let stepd = dba::step(
                            prev, &g2, fields2, 0.5, 1,
                            &perception, ent_id ^ frame_count as u64, dt,
                        );
                        session.dragon_bend_intents.insert(ent_id, stepd.intent);
                        // r149: GLOW TELL — the wind-up is visible: the dragon
                        // brightens in its element's hue as charge builds.
                        {
                            let base = unendlich_engine::spawn::dragon_tint(&g2);
                            let dom = g2
                                .element
                                .iter()
                                .enumerate()
                                .max_by(|a, b| {
                                    a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal)
                                })
                                .map(|(i, _)| i)
                                .unwrap_or(0);
                            use unendlich_engine::sim::emissive::Element as EmEl;
                            let em_el = match dom {
                                0 => EmEl::Wood,
                                1 => EmEl::Water,
                                2 => EmEl::Air,
                                3 => EmEl::Dust,
                                4 => EmEl::Fire,
                                _ => EmEl::Storm,
                            };
                            let em = unendlich_engine::sim::emissive::emissive_for(
                                em_el,
                                stepd.intent.charge01,
                                0.9,
                            );
                            for k in 0..3 {
                                cr.tint[k] = base[k] * (1.0 + em.intensity * em.color[k] * 1.5);
                            }
                        }
                        if let Some(rel) = stepd.release {
                            // Hit check: aim bearing within the technique's
                            // spread of the true bearing -> the cast lands.
                            let miss = (rel.aim_bearing - bearing).abs();
                            let spread = 0.12 + 0.05 * rel.technique.scale;
                            if miss < spread {
                                let dmg01 = (rel.potency * 0.06).clamp(0.0, 0.18);
                                if survival_mode {
                                    fg_app.vitality_mut().tick(1.0, dmg01 * player_dmg_resist);
                                }
                                log::info!(
                                    "[dragon] {:?} BENDING hits you — potency {:.1} (vitality {:.0}%)",
                                    rel.element, rel.potency,
                                    fg_app.vitality().energy * 100.0
                                );
                            } else {
                                log::info!(
                                    "[dragon] {:?} bending arc misses (off by {:.2} rad)",
                                    rel.element, miss
                                );
                            }
                        }
                    }
                    } // r150: end !asleep gate around bending
                    // r129: DRAGON BREATH BENDING — once per ~6s of the flight
                    // cycle, a flying dragon in range breathes at the player.
                    // Cone/range/damage/element from the REAL breath_attack
                    // params over a per-individual archetype genome (stable
                    // seed = flight_phase bits); falloff via damage_at.
                    if *flying && (cycle % 6.0) < 0.02 {
                        let g = unendlich_engine::sim::dragon_archetype::Genome::from_seed(
                            flight_phase.to_bits() as u64,
                        );
                        let bp = unendlich_engine::sim::breath_attack::params_from(&g);
                        let pp = local_player.view.position;
                        let dx = pp.x - cr.body.position[0];
                        let dy = pp.y - cr.body.position[1];
                        let dz = pp.z - cr.body.position[2];
                        let dist = (dx * dx + dy * dy + dz * dz).sqrt();
                        if dist <= bp.max_range {
                            let angle = {
                                let fyaw = cr.yaw;
                                let to_yaw = dz.atan2(dx);
                                let mut d = (to_yaw - fyaw).abs();
                                if d > std::f32::consts::PI { d = std::f32::consts::TAU - d; }
                                d
                            };
                            let dmg = unendlich_engine::sim::breath_attack::damage_at(&bp, angle, dist);
                            let elem = unendlich_engine::sim::dragon_archetype::ELEMENT_NAMES
                                [bp.element as usize % unendlich_engine::sim::dragon_archetype::NUM_ELEMENTS];
                            if dmg > 0.5 {
                                if survival_mode {
                                    fg_app.vitality_mut().tick(1.0, dmg * 0.004 * player_dmg_resist);
                                }
                                log::info!(
                                    "[dragon] {} BREATH hits you for {:.0} at {:.0} blocks — vitality {:.0}%",
                                    elem, dmg, dist, fg_app.vitality().energy * 100.0
                                );
                            } else {
                                log::info!("[dragon] {} breath misses ({:.0} blocks, off-axis)", elem, dist);
                            }
                        }
                    }
                    if *flying {
                        let target = if want_air {
                            surf + 16.0 + 4.0 * (cr.anim_time * 0.4).sin()
                        } else {
                            surf // landing approach
                        };
                        let gap = target - cr.body.position[1];
                        let vy = gap.clamp(-6.0, 6.0);
                        cr.body.position[1] += vy * dt;
                        // Wide banked circle.
                        cr.yaw += 0.35 * dt;
                        let spd = 7.0_f32;
                        cr.body.position[0] += cr.yaw.cos() * spd * dt;
                        cr.body.position[2] += cr.yaw.sin() * spd * dt;
                        cr.body.velocity = [cr.yaw.cos() * spd, vy, cr.yaw.sin() * spd];
                        cr.pitch = (vy / 10.0).clamp(-0.35, 0.35);
                        cr.roll = 0.22; // bank into the turn
                        if !want_air && gap.abs() < 1.5 {
                            *flying = false;
                            cr.body.velocity = [0.0; 3];
                            cr.roll = 0.0;
                            cr.pitch = 0.0;
                            log::info!("[dragon] LANDED at ({:.0}, {:.0}, {:.0})",
                                cr.body.position[0], cr.body.position[1], cr.body.position[2]);
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
            if profile_on { prof.record("roamer_tick", _t_roamer.elapsed().as_secs_f64() * 1e3); }
            if profile_on {
                prof.record("roamer_blockscan", _t_blockscan_us as f64 / 1000.0);
                prof.record("roamer_herdscan", _t_herdscan_us as f64 / 1000.0);
            }
            // r284: the roamer loop has read this frame's witnessed attacks; clear
            // for next frame. (Harm point after the loop carries to next frame —
            // a one-frame reaction lag, which is imperceptible.)
            attack_events.clear();

            // ---- Starvation: the unfed die ----
            // Persistent creatures (dragons) are exempt; anyone else that burned
            // through its reserve in barren country is removed.
            roamers.retain(|r| r.persistent || r.vitality.energy > 0.0);

            // ---- Mating ----
            // Nearby, well-fed, opposite-sex pairs of the same species breed. The
            // child inherits a Mendelian cross of the parents' genomes and spawns
            // at a parent's spot with a modest reserve.
            // r292: gated to a low cadence (BREED_INTERVAL) — breeding is a slow
            // biological event, and evaluating it every fired frame made the
            // pair-scan ~2/3 of game_update. Within the pass, a spatial grid
            // (sim::spatial_grid) replaces the old O(n^2) all-pairs scan.
            if last_breed_check.elapsed() >= BREED_INTERVAL {
                last_breed_check = Instant::now();
                const MATE_DIST2: f32 = 9.0; // within 3 blocks
                let mut offspring: Vec<Roamer> = Vec::new();
                // r292: SPATIAL GRID — bin roamers by position (cell side = mate
                // distance = 3 blocks) and only consider neighbour-cell pairs, so
                // mate-finding is ~O(n) instead of O(n^2). The precise MATE_DIST2 +
                // kind/vitality/sex tests below still gate each candidate.
                let positions: Vec<[f32; 3]> =
                    roamers.iter().map(|r| r.creature.body.position).collect();
                let candidates =
                    unendlich_engine::sim::spatial_grid::neighbour_pairs(&positions, 3.0);
                'pairs: for (i, j) in candidates {
                    {
                        // r463 (T2): breeding must respect the land's CARRYING CAPACITY, not just the
                        // hard ROAMER_CAP. Natural spawns + the spawn director already gate on
                        // fg_colony.population() (floored 8); breeding ignored it and bred toward 96,
                        // so herds overshot to 25–40 vs capacity 8 and mass-starved every tick (117
                        // starvation events in the r462 log). Gate it the same way for a stable pop.
                        let breed_cap = fg_colony.population().max(8).min(ROAMER_CAP);
                        if roamers.len() + offspring.len() >= breed_cap {
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
                        // r139: land can't support more grazers — gate BEFORE the
                        // split borrow (capacity from the popdyn colony).
                        if roamers.len() + offspring.len()
                            >= fg_colony.population().max(8).min(ROAMER_CAP)
                        {
                            continue;
                        }
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
                                bond: unendlich_engine::sim::bond_tame::Bond::Wild,
                                trust: 0.0,
                                hp: 40,
                                flying: false,
                                flight_phase: (cseed % 30) as f32,
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
                                species: None, // r462: offspring inherit no roster species (could inherit parent later)
                                genome: child,
                                forage_memory: unendlich_engine::sim::forage_memory::ForageMemory::new(16),
                                leap: unendlich_engine::sim::leap::Leap::from_genes(ph.speed, ph.speed),
                                airborne: None,
                                posture: unendlich_engine::sim::posture::Posture::new(0.0),
                                stumble: unendlich_engine::sim::stumble::Stumble::new(),
                                last_tick_frame: 0,
                                aggro: 0.0,
                                prev_behavior: 0,
                                combat_habit: 0.0,
                                disposition_cache: None,
                                graze_target: None,
                                graze_cell: (i32::MIN, i32::MIN),
                                graze_scan_tick: 0,
                            });
                        }
                    }
                }
                roamers.extend(offspring);
            // r141: resolve predator kills (deferred — indices were into the
            // pre-loop snapshot; remove highest-first so swap_remove is safe,
            // and only while the index still points at a non-persistent prey).
            prey_killed.sort_unstable_by(|a, b| b.cmp(a));
            prey_killed.dedup();
            for pi in prey_killed {
                if pi < roamers.len() && !roamers[pi].persistent {
                    let p = roamers[pi].creature.body.position;
                    roamers.swap_remove(pi);
                    fg_app.drops_mut().spawn(
                        unendlich_engine::items::ItemStack {
                            item: unendlich_engine::items::ItemId(MEAT_ITEM),
                            count: 1,
                        },
                        [p[0], p[1] + 0.4, p[2]],
                        [0.0, 1.2, 0.0],
                    );
                }
            }
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
                    session.fire_positions.push([pos.x, foot_y, pos.z]); // r129: feeds the smoke field
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
                // r314: don't fire hotbar keys while typing — the Dvorak home-row
                // keys (a o e u i d h t n) double as Hotbar1..9, so 'e' etc. would
                // hijack text fields.
                if !renderer.ui_wants_keyboard
                    && bindings.is_action_just_pressed(*action, &input)
                {
                    selected_block = PLACE_PALETTE[i];
                    selected_palette_idx = i; // r122: keep scroll cursor in sync
                    renderer.creative_selected = i; // r554: move the hotbar UI highlight
                    fg_app.inventory_mut().set_selected(i); // r120: hotbar cursor follows
                    if survival_mode {
                        hand_item = fg_app.inventory().slot(i).map(|s| s.item);
                        if let Some(stack) = fg_app.inventory().slot(i) {
                            if let Some(b) = unendlich_engine::world::blocks::BlockId::ALL
                                .iter()
                                .copied()
                                .find(|b| *b as u32 == stack.item.0)
                            {
                                selected_block = b; // r125: hand = inventory item
                            }
                        }
                    }
                    log::info!("Selected place block: {:?}", selected_block);
                }
            }
            // r554: ALT + HOME-ROW chord — hold Left Alt and tap a labelled home-row
            // key (a o e u i d h t n) to jump straight to that hotbar slot. The slots
            // are already labelled with these keys; the modifier keeps the bare keys
            // free for movement/typing.
            if input.is_key_pressed(Keycode::LeftAlt) && !renderer.ui_wants_keyboard {
                use unendlich_engine::world::blocks::BlockId;
                const HOMEROW_KEYS: [Keycode; 9] = [
                    Keycode::A, Keycode::O, Keycode::E, Keycode::U, Keycode::I,
                    Keycode::D, Keycode::H, Keycode::T, Keycode::N,
                ];
                for (slot, &kc) in HOMEROW_KEYS.iter().enumerate() {
                    if slot < PLACE_PALETTE.len() && input.is_key_just_pressed(kc) {
                        selected_block = PLACE_PALETTE[slot];
                        selected_palette_idx = slot;
                        renderer.creative_selected = slot;
                        fg_app.inventory_mut().set_selected(slot);
                        if survival_mode {
                            hand_item = fg_app.inventory().slot(slot).map(|s| s.item);
                            if let Some(stack) = fg_app.inventory().slot(slot) {
                                if let Some(b) = BlockId::ALL
                                    .iter()
                                    .copied()
                                    .find(|b| *b as u32 == stack.item.0)
                                {
                                    selected_block = b;
                                }
                            }
                        }
                        log::info!("[hotbar] alt+home-row -> slot {}", slot + 1);
                        break;
                    }
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
        let mouse_left_held = input.is_mouse_button_pressed(MouseBtn::Left);
        let mouse_right_held = input.is_mouse_button_pressed(MouseBtn::Right);
        // r486: snapshot so we can detect when an action actually fires (any of
        // the break/place/use sites resets last_block_action) → swing the hand.
        let prev_block_action = last_block_action;
        let cooldown_ok = last_block_action.elapsed().as_millis() >= BLOCK_ACTION_COOLDOWN_MS;
        // Permission gate: a player without can_break_block / can_place_block
        // shouldn't even raycast (saves cycles). Single-player default
        // permissions grant both, so behavior is unchanged. Multiplayer
        // server pushes restrictive permissions to e.g. spectators.
        let break_allowed = mouse_left_held && local_player.permissions.can_break_block;
        let place_allowed = mouse_right_held && local_player.permissions.can_place_block;
        // r565: radius scales ~ceil((h−4)·¼) above a stature of 5 (1 at 5–8 blocks,
        // 2 at 12, 3 at 16, 4 at 20, 6 at 25), capped at 6. Drives the place-mound
        // size + the break fan below.
        let dig_radius: i32 = if player_stature.height > 5.0 {
            // r573: bigger mining volume + ceiling raised 8 → 16 (operator). Factor 0.36
            // so a 50-block titan reaches ~16; placement uses half this (below).
            (((player_stature.height - 4.0) * 0.36).ceil() as i32).clamp(1, 16)
        } else {
            0
        };
        // r573: PLACE sphere is HALF the mining sphere (operator) — building reads as a
        // mound, not a crater-sized blob.
        let place_radius: i32 = (dig_radius / 2).max(if dig_radius > 0 { 1 } else { 0 });
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
                // r569: reach grows ~LINEARLY with stature so a giant can actually reach
                // the ground in front of it. (The old √ scaling fell short: a 25-block eye
                // sits ~22 blocks up, but √ reach was only ~22 — it couldn't touch the floor.)
                // = 6.0 for a default character; slope 1.2 (> eye fraction) clears the eye
                // height so the ground a few blocks forward is always targetable.
                (6.0 + 1.2 * (player_stature.height
                    - unendlich_engine::sim::stature::BASE_HEIGHT).max(0.0)).max(6.0),
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
                // r143: FEED-TO-TAME — right-mouse with MEAT in hand while a
                // creature is in reach feeds the CREATURE (bond_tame Feed
                // event) rather than yourself. Tamed (trust crosses the
                // threshold) creatures follow you and never count as prey.
                let mut click_fed_creature = false;
                if place_allowed
                    && survival_mode
                    && hand_item.map_or(false, |h| h.0 == MEAT_ITEM)
                {
                    let eye = local_player.view.position;
                    let dirv = cam_forward;
                    let mut best: Option<(usize, f32)> = None;
                    for (ri, r) in roamers.iter().enumerate() {
                        let p = r.creature.body.position;
                        let to = glm::vec3(p[0] - eye.x, p[1] - eye.y, p[2] - eye.z);
                        let along = glm::dot(&to, &dirv);
                        if !(0.5..=3.5).contains(&along) {
                            continue;
                        }
                        let closest = eye + dirv * along;
                        if glm::distance(&closest, &glm::vec3(p[0], p[1] + 0.5, p[2])) < 1.4
                            && best.map_or(true, |(_, d)| along < d)
                        {
                            best = Some((ri, along));
                        }
                    }
                    if let Some((ri, _)) = best {
                        if fg_app.inventory_mut().remove_item(
                            unendlich_engine::items::ItemId(MEAT_ITEM), 1,
                        ) == 0 {
                            let r = &mut roamers[ri];
                            let (nb, nt) = unendlich_engine::sim::bond_tame::advance(
                                r.bond,
                                unendlich_engine::sim::bond_tame::Event::Feed,
                                r.trust,
                            );
                            let newly_tamed =
                                nb == unendlich_engine::sim::bond_tame::Bond::Tamed
                                    && r.bond != unendlich_engine::sim::bond_tame::Bond::Tamed;
                            r.bond = nb;
                            r.trust = nt;
                            r.vitality.feed(0.3);
                            log::info!(
                                "[tame] fed {:?} — {:?} (trust {:.2}){}",
                                r.kind, r.bond, r.trust,
                                if newly_tamed { "  ** TAMED — it follows you now **" } else { "" }
                            );
                            last_block_action = Instant::now();
                        }
                        // a creature took the click; combat/place skip below
                        click_fed_creature = true;
                    }
                }
                // r164: ESSENCE USE — drinking a slain dragon's essence:
                // right-click with essence in hand consumes one, fully
                // refills bending resonance, clears overdraw, and attunes
                // the imbue artifact to the essence's element.
                if place_allowed
                    && survival_mode
                    && !click_fed_creature
                    && hand_item.map_or(false, |h| (3000..3006).contains(&h.0))
                {
                    if let Some(h) = hand_item {
                        if fg_app.inventory_mut().remove_item(h, 1) == 0 {
                            bend_resonance =
                                unendlich_engine::sim::bending_charge::Resonance::full();
                            fg_imbue.element = (h.0 - 3000) as u8;
                            fg_imbue.counter = (fg_imbue.counter + 0.4).min(1.0);
                            click_fed_creature = true; // consume the click
                            log::info!(
                                "[essence] consumed element-{} essence — resonance FULL, artifact attunes",
                                h.0 - 3000
                            );
                            last_block_action = Instant::now();
                        }
                    }
                }
                // r127: COMBAT — a creature in punching range along the view
                // ray takes the hit instead of the block behind it. Damage runs
                // through the REAL sim::combat::resolve (no shield/armor, fresh
                // affliction set), so the elemental pipeline is live.
                let mut creature_hit = click_fed_creature; // feeding consumed the click
                if break_allowed && !click_fed_creature {
                    let eye = local_player.view.position;
                    let dir = cam_forward;
                    let mut best: Option<(usize, f32)> = None;
                    for (ri, r) in roamers.iter().enumerate() {
                        let p = r.creature.body.position;
                        let to = glm::vec3(p[0] - eye.x, p[1] - eye.y, p[2] - eye.z);
                        let along = glm::dot(&to, &dir);
                        if !(0.5..=4.0).contains(&along) {
                            continue;
                        }
                        let closest = eye + dir * along;
                        let lateral = glm::distance(
                            &closest,
                            &glm::vec3(p[0], p[1] + 0.5, p[2]),
                        );
                        if lateral < 1.2 && best.map_or(true, |(_, d)| along < d) {
                            best = Some((ri, along));
                        }
                    }
                    if let Some((ri, _)) = best {
                        let attack = unendlich_engine::sim::combat::Attack {
                            element: unendlich_engine::sim::elemental::Element::Earth,
                            magnitude: 12,
                            affliction_ticks: 0,
                        };
                        let mut aff = unendlich_engine::sim::affliction::Afflicted::new();
                        let hit = unendlich_engine::sim::combat::resolve(&attack, None, None, &mut aff);
                        // r268: crit + armor modifiers (off by default → flat damage).
                        let dmg_dealt: i32 = if combat_mods_enabled {
                            let raw = hit.health_damage as f32;
                            // deterministic crit roll from frame + target identity
                            let roll_seed = (frame_count as u64)
                                .wrapping_mul(0x9E37_79B9)
                                .wrapping_add(ri as u64);
                            let roll = ((roll_seed >> 40) & 0xFFFF) as f32 / 65535.0;
                            let crit_dmg = unendlich_engine::sim::crit_roll::crit(roll, 0.2, 1.8);
                            // natural armor from body mass (bigger = tougher hide)
                            let armor = (roamers[ri].creature.mass * 2.0).clamp(0.0, 30.0);
                            let mitig = unendlich_engine::sim::damage_after_armor::mitigated(
                                raw * crit_dmg, armor, 20.0,
                            );
                            let was_crit = crit_dmg > 1.0;
                            if was_crit {
                                log::info!("[combat] CRIT ×{:.1} (raw {} → {:.0} after armor {:.0})", crit_dmg, hit.health_damage, mitig, armor);
                            }
                            mitig.round().max(1.0) as i32
                        } else {
                            hit.health_damage as i32
                        };
                        {
                            // r143: violence is remembered — Harm event.
                            let (nb, nt) = unendlich_engine::sim::bond_tame::advance(
                                roamers[ri].bond,
                                unendlich_engine::sim::bond_tame::Event::Harm,
                                roamers[ri].trust,
                            );
                            roamers[ri].bond = nb;
                            roamers[ri].trust = nt;
                        }
                        roamers[ri].hp -= dmg_dealt;
                        // r509: the weapon wears with each swing; at 0 it breaks (survival only).
                        if survival_mode {
                            if let Some((wid, cur, max)) = equipped_weapon_dura {
                                let (nc, broke) = unendlich_engine::sim::item_durability::apply_wear(
                                    cur, max, WEAR_PER_ATTACK,
                                );
                                equipped_weapon_dura = Some((wid, nc, max));
                                if broke {
                                    let _ = fg_app.inventory_mut().remove_item(wid, 1);
                                    equipped_weapon_dura = None;
                                    log::info!("[durability] your weapon shattered from the blow");
                                }
                            }
                        }
                        // rNNN: LIGHTNING ARC — the strike chains to nearby foes
                        // via the REAL legendary_weapon swing resolver (r369). A
                        // Storm-element blade (element 5 ⇒ LightningArc proc) is
                        // forged once; resolve_weapon_swing picks the arc targets
                        // (greedy nearest, no repeats) and the per-arc damage.
                        // Gated default-OFF, additive — primary hit is untouched.
                        if lightning_arc_enabled {
                            let origin = roamers[ri].creature.body.position;
                            let mut cand_idx: Vec<usize> = Vec::new();
                            let mut cand_pos: Vec<[f32; 3]> = Vec::new();
                            for (rj, rr) in roamers.iter().enumerate() {
                                if rj == ri || rr.hp <= 0 { continue; }
                                let p = rr.creature.body.position;
                                let d2 = (p[0] - origin[0]).powi(2)
                                    + (p[1] - origin[1]).powi(2)
                                    + (p[2] - origin[2]).powi(2);
                                if d2 <= 12.0 * 12.0 {
                                    cand_idx.push(rj);
                                    cand_pos.push(p);
                                }
                            }
                            let sword = unendlich_engine::sim::legendary_weapon::forge(
                                0x5106_57A2,
                                unendlich_engine::sim::legendary_weapon::WeaponClass::Blade,
                                5, // Storm ⇒ LightningArc
                                3,
                            );
                            let roll_seed = (frame_count as u64)
                                .wrapping_mul(0x9E37_79B9)
                                .wrapping_add(ri as u64);
                            let roll = ((roll_seed >> 40) & 0xFFFF) as f32 / 65535.0;
                            let swing = unendlich_engine::sim::legendary_weapon::resolve_weapon_swing(
                                &sword, roll, origin, &cand_pos, 4, 6.0,
                            );
                            if !swing.arc_targets.is_empty() {
                                let arc_dmg = swing.arc_damage.round().max(1.0) as i32;
                                for &ci in &swing.arc_targets {
                                    let rj = cand_idx[ci];
                                    roamers[rj].hp -= arc_dmg;
                                    if witness_enabled {
                                        let vk = match roamers[rj].kind {
                                            unendlich_engine::item::CreatureKind::Dinosaur => 1u8,
                                            unendlich_engine::item::CreatureKind::Dragon => 2u8,
                                            _ => 0u8,
                                        };
                                        attack_events.push((roamers[rj].creature.body.position, vk));
                                    }
                                }
                                log::info!(
                                    "[lightning] arc chained to {} foe(s) for {} each",
                                    swing.arc_targets.len(), arc_dmg
                                );
                            }
                        }
                        // r284: record this attack so creatures that can see it can
                        // react (witnessed-attack reaction, below). Position + kind.
                        if witness_enabled {
                            let vk = match roamers[ri].kind {
                                unendlich_engine::item::CreatureKind::Dinosaur => 1u8,
                                unendlich_engine::item::CreatureKind::Dragon => 2u8,
                                _ => 0u8,
                            };
                            attack_events.push((roamers[ri].creature.body.position, vk));
                        }
                        // r268: being hit stokes aggression. The ceiling comes from
                        // aggression_level (health + 1v1 numbers); a hit pushes
                        // aggro toward it. High aggro → approach instead of flee.
                        if aggro_enabled {
                            let hp_frac = (roamers[ri].hp.max(0) as f32 / 40.0).clamp(0.0, 1.0);
                            let ceil = unendlich_engine::sim::aggression_level::aggression(hp_frac, 0, 1);
                            roamers[ri].aggro = (roamers[ri].aggro + 0.5).min(ceil.max(0.3));
                            log::info!("[aggro] {:?} provoked → aggro {:.2} (ceil {:.2})", roamers[ri].kind, roamers[ri].aggro, ceil);
                        }
                        // r129: swipe-claw recoil — the struck creature flinches
                        // (claw_curl curve drives a torso_bend impulse that the
                        // per-frame decay relaxes).
                        let curl = unendlich_engine::sim::claw_curl::claw_curl(
                            1.0, roamers[ri].creature.gait_phase, 3,
                        );
                        roamers[ri].creature.torso_bend =
                            curl.iter().sum::<f32>() / curl.len().max(1) as f32 * 0.6;
                        // r566: KNOCKBACK — the blow shoves the struck creature along the
                        // punch direction (+ a small upward pop), scaled by the attacker's
                        // stature (knockback_dealt = 1.0 default → a giant sends them flying).
                        {
                            let kb = 3.0 * player_stature.knockback_dealt;
                            let v = &mut roamers[ri].creature.body.velocity;
                            v[0] += cam_forward.x * kb;
                            v[1] += 1.2;
                            v[2] += cam_forward.z * kb;
                        }
                        let p = roamers[ri].creature.body.position;
                        log::info!(
                            "[combat] punched {:?} for {} (hp {})",
                            roamers[ri].kind, dmg_dealt, roamers[ri].hp.max(0)
                        );
                        if roamers[ri].hp <= 0 {
                            log::info!("[combat] {:?} slain — meat dropped", roamers[ri].kind);
                            // r146: DRAGON ESSENCE — dragons also yield
                            // elemental essence per the engine's material
                            // model (element from the archetype genome).
                            if matches!(
                                roamers[ri].kind,
                                unendlich_engine::item::CreatureKind::Dragon
                            ) {
                                use unendlich_engine::sim::dragon_material_yield as dmy;
                                let g2 = unendlich_engine::sim::dragon_archetype::Genome::from_seed(
                                    roamers[ri].flight_phase.to_bits() as u64,
                                );
                                let elem = g2
                                    .element
                                    .iter()
                                    .enumerate()
                                    .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
                                    .map(|(i, _)| i as u8)
                                    .unwrap_or(0);
                                let y = dmy::material_yield(&dmy::DragonMaterial {
                                    element: elem,
                                    attunement01: 0.6,
                                    rarity_tier: 3,
                                });
                                let total: u32 = y.essence.iter().sum();
                                let count = ((total / 20).clamp(1, 8)) as u16;
                                fg_app.drops_mut().spawn(
                                    unendlich_engine::items::ItemStack {
                                        item: unendlich_engine::items::ItemId(3000 + elem as u32),
                                        count,
                                    },
                                    [p[0], p[1] + 1.0, p[2]],
                                    [0.4, 2.4, -0.3],
                                );
                                log::info!(
                                    "[loot] dragon essence x{} (element {}) — a rare prize",
                                    count, elem
                                );
                            }
                            // r511: VARIED LOOT — meat count scales with the creature's bulk
                            // (tier) via sim::loot_quantity, and a luck-boosted sim::drop_chance
                            // roll adds an occasional egg. A small fish yields one scrap; a big
                            // beast yields a haul.
                            let tier = ((roamers[ri].creature.mass * 0.4) as u32).min(4);
                            let luck = 0.25;
                            let meat_n = unendlich_engine::sim::loot_quantity::quantity(tier, luck, 5)
                                .max(1) as u16;
                            fg_app.drops_mut().spawn(
                                unendlich_engine::items::ItemStack {
                                    item: unendlich_engine::items::ItemId(MEAT_ITEM),
                                    count: meat_n,
                                },
                                [p[0], p[1] + 0.6, p[2]],
                                [0.0, 2.0, 0.0],
                            );
                            let egg_p =
                                unendlich_engine::sim::drop_chance::drop_chance(0.12, luck, 0.0);
                            let roll = {
                                let s = (p[0].to_bits() as u64)
                                    ^ ((p[2].to_bits() as u64) << 17)
                                    ^ (frame_count as u64).wrapping_mul(0x9E37_79B1);
                                ((s >> 33) & 0xFFFF) as f32 / 65535.0
                            };
                            if roll < egg_p {
                                fg_app.drops_mut().spawn(
                                    unendlich_engine::items::ItemStack {
                                        item: unendlich_engine::items::ItemId(999),
                                        count: 1,
                                    },
                                    [p[0] + 0.3, p[1] + 0.7, p[2]],
                                    [0.5, 2.2, 0.2],
                                );
                                log::info!("[loot] …and an egg");
                            }
                            log::info!("[loot] {} meat (tier {})", meat_n, tier);
                            roamers.swap_remove(ri);
                        }
                        last_block_action = Instant::now();
                        creature_hit = true;
                    }
                }
                if break_allowed && !creature_hit {
                    // Break: replace hit block with air.
                    let [x, y, z] = hit_cell;
                    if chunk_manager.set_block(x, y, z, BlockId::Air) {
                        log::info!("Broke {:?} at ({},{},{})", hit_block_id, x, y, z);
                        // r512: SEEDS FROM GRASS — breaking wild ground cover sometimes yields a
                        // vegetable, so a survival player can find their first crop and start a
                        // farm (plant it on Grass/Dirt). Deterministic roll from cell + frame.
                        if matches!(hit_block_id, BlockId::TallGrass | BlockId::Fern | BlockId::Flower) {
                            let h = (x as u64).wrapping_mul(0x9E37_79B1)
                                ^ ((z as u64) << 21)
                                ^ (frame_count as u64).wrapping_mul(0x85EB_CA77);
                            if (h & 0xFF) < 38 {
                                // ~15%
                                let veg = [BlockId::Carrot, BlockId::Potato, BlockId::Cabbage]
                                    [((h >> 8) % 3) as usize];
                                fg_app.drops_mut().spawn(
                                    unendlich_engine::items::ItemStack {
                                        item: unendlich_engine::items::ItemId(veg as u32),
                                        count: 1,
                                    },
                                    [x as f32 + 0.5, y as f32 + 0.4, z as f32 + 0.5],
                                    [0.0, 1.4, 0.0],
                                );
                                log::info!("[farm] a {:?} was hidden in the grass — plant it on soil to farm", veg);
                            }
                        }
                        // r521: SAPLINGS FROM LEAVES — breaking foliage occasionally drops a tree
                        // sapling (a Sprout item) you can plant on soil under open sky to grow a tree.
                        if matches!(hit_block_id, BlockId::OakLeaves | BlockId::BirchLeaves) {
                            let h = (x as u64).wrapping_mul(0xD1B5_4A35)
                                ^ ((z as u64) << 19)
                                ^ (frame_count as u64).wrapping_mul(0xC2B2_AE3D);
                            if (h & 0xFF) < 28 {
                                // ~11%
                                fg_app.drops_mut().spawn(
                                    unendlich_engine::items::ItemStack {
                                        item: unendlich_engine::items::ItemId(BlockId::Sprout as u32),
                                        count: 1,
                                    },
                                    [x as f32 + 0.5, y as f32 + 0.4, z as f32 + 0.5],
                                    [0.0, 1.4, 0.0],
                                );
                                log::info!("[farm] a sapling fell from the leaves — plant it on soil under open sky");
                            }
                            // r579: FRUIT — leafy trees sometimes bear an apple you can eat
                            // (right-click) for a light vitality bump. Independent roll from
                            // the sapling drop above.
                            let hf = (x as u64).wrapping_mul(0x9E37_79B1)
                                ^ ((z as u64) << 17)
                                ^ (frame_count as u64).wrapping_mul(0x2545_F491);
                            if (hf & 0xFF) < 33 {
                                // ~13%
                                fg_app.drops_mut().spawn(
                                    unendlich_engine::items::ItemStack {
                                        item: unendlich_engine::items::ItemId(APPLE_ITEM),
                                        count: 1,
                                    },
                                    [x as f32 + 0.5, y as f32 + 0.5, z as f32 + 0.5],
                                    [0.0, 1.2, 0.0],
                                );
                                log::info!("[forage] an apple dropped from the tree");
                            }
                        }
                        water_dirty = true; // opt-1: terrain changed near possible water
                        last_block_action = Instant::now();
                        // r509: mining with a forged tool wears it (survival only); breaks at 0.
                        if survival_mode {
                            if let Some((wid, cur, max)) = equipped_weapon_dura {
                                let (nc, broke) = unendlich_engine::sim::item_durability::apply_wear(
                                    cur, max, WEAR_PER_MINE,
                                );
                                equipped_weapon_dura = Some((wid, nc, max));
                                if broke {
                                    let _ = fg_app.inventory_mut().remove_item(wid, 1);
                                    equipped_weapon_dura = None;
                                    log::info!("[durability] your tool wore out and broke");
                                }
                            }
                        }
                        // r512/r514: HARVEST — breaking a tracked crop yields a bonus when ripe
                        // (the farming payoff). The default block-break already drops 1; a ripe
                        // one drops several more (sim::loot_quantity). Pulled from the persistent
                        // flora store, so a crop you grew before saving still pays out after load.
                        if let Some(cell) = session.flora.remove([x, y, z]) {
                            // r519: only crops pay a harvest bonus; breaking an OakLog sapling
                            // just removes it from tracking (handled by the remove above).
                            // r520: uprooting a seedling (Sprout) returns the seed you sowed.
                            if cell.block == BlockId::Sprout as u8 {
                                let seed = match cell.element {
                                    1 => BlockId::Potato,
                                    2 => BlockId::Cabbage,
                                    _ => BlockId::Carrot,
                                };
                                fg_app.drops_mut().spawn(
                                    unendlich_engine::items::ItemStack {
                                        item: unendlich_engine::items::ItemId(seed as u32),
                                        count: 1,
                                    },
                                    [x as f32 + 0.5, y as f32 + 0.4, z as f32 + 0.5],
                                    [0.0, 1.5, 0.0],
                                );
                                log::info!("[farm] uprooted a {:?} seedling — seed recovered", seed);
                            }
                            let is_crop = matches!(
                                cell.block,
                                c if c == BlockId::Carrot as u8
                                    || c == BlockId::Potato as u8
                                    || c == BlockId::Cabbage as u8
                            );
                            let ripe = is_crop && cell.is_ripe();
                            // r522: SEED GENETICS — a fruitful genome yields more, and harvesting a
                            // ripe crop breeds its genome back into that crop's seed line, so the
                            // next planting inherits the improvement (selective breeding).
                            let g = unendlich_engine::sim::plant_tree_genetics::PlantGenome::from_array(cell.genome);
                            if ripe {
                                let idx = if cell.block == BlockId::Potato as u8 {
                                    1usize
                                } else if cell.block == BlockId::Cabbage as u8 {
                                    2usize
                                } else {
                                    0usize
                                };
                                seed_stock[idx] = unendlich_engine::sim::plant_tree_genetics::cross(
                                    &seed_stock[idx], &g,
                                    ((x as u64) << 24) ^ ((z as u64) << 8) ^ frame_count,
                                );
                            }
                            let luck = 0.3 + g.fruitfulness * 0.6;
                            let extra = if ripe {
                                unendlich_engine::sim::loot_quantity::quantity(2, luck, 5).max(1)
                            } else {
                                0
                            };
                            for k in 0..extra {
                                let a = (k as f32) * 1.7;
                                fg_app.drops_mut().spawn(
                                    unendlich_engine::items::ItemStack {
                                        item: unendlich_engine::items::ItemId(cell.block as u32),
                                        count: 1,
                                    },
                                    [x as f32 + 0.5, y as f32 + 0.4, z as f32 + 0.5],
                                    [a.cos() * 1.2, 1.6, a.sin() * 1.2],
                                );
                            }
                            if ripe {
                                log::info!("[farm] harvested ripe crop — +{} bonus yield", extra);
                            }
                        }
                        session.lan_outgoing_edits.push(
                            unendlich_engine::sim::world_authority::WorldCmd::Remove {
                                pos: (x, y, z),
                            },
                        );
                        // r122: THE item loop — broken blocks drop their item
                        // (mining::block_drop rules: stone->cobble, leaves->none,
                        // bare-hand harvest table) into the DropField; the r120
                        // cube renders it; proximity pickup banks it.
                        // r160: LEAF DECAY TRIGGER — felling a log orphans
                        // its canopy: leaves within 4 blocks get
                        // FLAG_DECAYING and the registered tick handler ages
                        // them to Air over the following minutes (faster in
                        // wind). Untouched forest is permanent.
                        // r451: FLORA BLOOM HARVEST — most plant breaks just drop the block, but a
                        // rare mature bloom yields elemental essence via sim::flora_harvest (the flora
                        // mirror of dragon essence). Deterministic per cell, so a given spot is stable.
                        if matches!(
                            hit_block_id,
                            BlockId::Flower | BlockId::TallGrass | BlockId::Fern | BlockId::Mushroom
                        ) {
                            use unendlich_engine::sim::flora_harvest as fh;
                            let cell_hash = unendlich_engine::util::hash_util::mix64(
                                ((x as i64 as u64) << 42)
                                    ^ ((y as i64 as u64) << 21)
                                    ^ (z as i64 as u64),
                            );
                            // rare: flowers bloom a touch more often than ground cover.
                            let bloom_pct: u64 =
                                if matches!(hit_block_id, BlockId::Flower) { 9 } else { 4 };
                            if (cell_hash % 100) < bloom_pct {
                                let element = ((cell_hash >> 8) % 6) as u8;
                                let strain = ((cell_hash >> 16) % 1000) as f32 / 1000.0;
                                let y_yield = fh::harvest(&fh::PlantState {
                                    element,
                                    strain_bias: strain,
                                    growth01: 1.0,
                                });
                                const PER_ITEM: u32 = 8; // reagent units per dropped essence item
                                let mut dropped = 0u32;
                                for i in 0..6usize {
                                    let count = (y_yield.reagent_units[i] / PER_ITEM).min(4) as u16;
                                    if count > 0 {
                                        fg_app.drops_mut().spawn(
                                            unendlich_engine::items::ItemStack {
                                                item: unendlich_engine::items::ItemId(
                                                    3000 + i as u32,
                                                ),
                                                count,
                                            },
                                            [x as f32 + 0.5, y as f32 + 0.6, z as f32 + 0.5],
                                            [0.3, 2.2, -0.2],
                                        );
                                        dropped += count as u32;
                                    }
                                }
                                if dropped > 0 {
                                    log::info!(
                                        "[flora] a rare bloom (element {}) — harvested {} elemental essence",
                                        element, dropped
                                    );
                                }
                            }
                        }
                        if matches!(hit_block_id, BlockId::OakLog | BlockId::BirchLog) {
                            let mut marked = 0u32;
                            for dy in -1..=5_i32 {
                                for dz in -4..=4_i32 {
                                    for dx in -4..=4_i32 {
                                        let (lx, ly, lz) = (x + dx, y + dy, z + dz);
                                        if matches!(
                                            chunk_manager.block_at(lx, ly, lz),
                                            BlockId::OakLeaves | BlockId::BirchLeaves
                                        ) {
                                            let mut st = unendlich_engine::world::chunk::BlockInstanceState::default();
                                            st.set_flag(
                                                unendlich_engine::world::chunk::BlockInstanceState::FLAG_DECAYING,
                                                true,
                                            );
                                            if chunk_manager.set_instance_state(lx, ly, lz, st) {
                                                marked += 1;
                                            }
                                        }
                                    }
                                }
                            }
                            if marked > 0 {
                                log::info!("[flora] {} leaves orphaned — the canopy will wither", marked);
                            }
                        }
                        // r142: SEED DISPERSAL — breaking leaves releases
                        // seeds carried by the REAL wind vector; each lands
                        // downwind and germinates (or not) per the REAL
                        // germination gate over local moisture/warmth/light.
                        if matches!(hit_block_id, BlockId::OakLeaves | BlockId::BirchLeaves) {
                            let (wsx, wsz, _wm) = unendlich_engine::wind::wind_at(
                                x as f64, y as f64, z as f64, sky_state.time_seconds,
                            );
                            let genes = unendlich_engine::sim::seed_dispersal::DispersalGenes {
                                spread_radius: 6.0,
                                count: 3,
                            };
                            let dseed = (x as u64) << 40 ^ (z as u64) << 8 ^ frame_count as u64;
                            let m = chunk_manager.model_arc();
                            let mut sprouted = 0;
                            for off in unendlich_engine::sim::seed_dispersal::disperse(
                                genes, [wsx as f32, wsz as f32], dseed,
                            ) {
                                let lx = x + off[0].round() as i32;
                                let lz = z + off[1].round() as i32;
                                let Some(ly) = chunk_manager.generator().cpu()
                                    .surface_y_at(lx, lz, probe_y_min, probe_y_max) else { continue };
                                let f = unendlich_engine::sim::germination::Fields {
                                    // r156: rain raises landing-spot moisture
                                    h: (m.humidity(lx as f64, ly as f64, lz as f64, 0.0) as f32
                                        + precip01 * 0.3).min(1.0),
                                    r: m.temperature(lx as f64, ly as f64, lz as f64, 0.0) as f32,
                                    l: 0.8, // surface = lit
                                    s: 0.5,
                                    b: 0.6,
                                };
                                let sg = unendlich_engine::sim::germination::SeedGenome {
                                    element: 0,
                                    vigor: 0.7,
                                    dormancy: 0.3,
                                };
                                let g = unendlich_engine::sim::germination::germinate(&sg, &f);
                                if g.will_germinate
                                    && chunk_manager.set_block(lx, ly + 1, lz, BlockId::Flower)
                                {
                                    sprouted += 1;
                                }
                            }
                            if sprouted > 0 {
                                log::info!("[flora] {} seed(s) took root downwind", sprouted);
                            }
                        }
                        // r149: ENGINE LOOT — the loot table rolls the drop
                        // (tool-aware), and resolve_drops scatters the spawns
                        // with real velocities. Unknown blocks fall back to
                        // the old direct mapping.
                        let loot = unendlich_engine::sim::item_drop_resolve::resolve_drops(
                            unendlich_engine::sim::item_drop_resolve::DropSource {
                                table_key: hit_block_id as u16,
                                origin: [x as f32 + 0.5, y as f32 + 0.5, z as f32 + 0.5],
                                has_tool: hand_item.is_some(),
                            },
                            &fg_loot_table,
                            (x as u64) << 40 ^ (y as u64) << 20 ^ (z as u64),
                        );
                        if !loot.is_empty() {
                            for sp in loot {
                                fg_app.drops_mut().spawn(sp.stack, sp.pos, sp.vel);
                            }
                        } else {
                            match unendlich_engine::mining::block_drop(hit_block_id, None) {
                                Some(unendlich_engine::item::ItemKind::Block(dropped)) => {
                                    let stack = unendlich_engine::items::ItemStack {
                                        item: unendlich_engine::items::ItemId(dropped as u32),
                                        count: 1,
                                    };
                                    fg_app.drops_mut().spawn(
                                        stack,
                                        [x as f32 + 0.5, y as f32 + 0.5, z as f32 + 0.5],
                                        [0.0, 1.5, 0.0],
                                    );
                                }
                                Some(other) => {
                                    log::info!("[items] non-block drop {:?} (no item entity yet)", other);
                                }
                                None => {}
                            }
                        }
                    }
                    // r571: SOLID DIG — a large character carves a FILLED SPHERE of solid
                    // blocks centred on the primary hit cell. (Replaces the old fan of
                    // diverging rays, which feathered out with gaps and couldn't punch a
                    // clean hole straight down at titan scale.) Bedrock (hardness < 0) is
                    // spared; everything else in the radius goes. The central cell was
                    // already cleared by the primary break above.
                    if dig_radius > 0 {
                        let [hx, hy, hz] = hit_cell;
                        let r = dig_radius;
                        let r2 = r * r;
                        let mut cleared = 0u32;
                        for dy in -r..=r {
                            for dz in -r..=r {
                                for dx in -r..=r {
                                    if dx * dx + dy * dy + dz * dz > r2 {
                                        continue;
                                    }
                                    let (cx, cy, cz) = (hx + dx, hy + dy, hz + dz);
                                    let blk = chunk_manager.block_at(cx, cy, cz);
                                    if blk.is_solid()
                                        && blk.hardness() >= 0.0
                                        && chunk_manager.set_block(cx, cy, cz, BlockId::Air)
                                    {
                                        cleared += 1;
                                    }
                                }
                            }
                        }
                        if cleared > 0 {
                            water_dirty = true;
                            last_block_action = Instant::now();
                            log::info!("[dig] carved {} blocks (solid sphere r={})", cleared, r);
                        }
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
                        } else if !click_fed_creature {
                            let go = if !survival_mode {
                                Some(selected_block)
                            } else if matches!(selected_block, BlockId::Carrot | BlockId::Potato | BlockId::Cabbage | BlockId::Sprout)
                                && matches!(chunk_manager.block_at(x, y - 1, z), BlockId::Grass | BlockId::Dirt)
                                && fg_app.inventory().count(unendlich_engine::items::ItemId(selected_block as u32)) > 0
                            {
                                // r512: PLANT on tilled soil — consume one and place it (registered below).
                                let _ = fg_app.inventory_mut().remove_item(unendlich_engine::items::ItemId(selected_block as u32), 1);
                                Some(selected_block)
                            } else {
                                survival_use_item(&mut fg_app, selected_block, hand_item)
                            };
                            if let Some(blk) = go {
                                if chunk_manager.set_block(x, y, z, blk) {
                                    log::info!("Placed {:?} at ({},{},{}) on {:?} face of {:?}",
                                        blk, x, y, z, hit_face, hit_block_id);
                                    water_dirty = true; // opt-1: terrain changed
                                    // r564: BIGGER MOUNDS — large characters place a sphere of
                                    // the block into surrounding air (radius from stature). In
                                    // survival each placed block costs one from inventory; it
                                    // stops when you run out. Crops/saplings are excluded.
                                    if place_radius > 0
                                        && !matches!(blk, BlockId::Carrot | BlockId::Potato | BlockId::Cabbage | BlockId::Sprout)
                                    {
                                        let r2 = place_radius * place_radius;
                                        let mut budget: i64 = if survival_mode {
                                            fg_app.inventory().count(unendlich_engine::items::ItemId(blk as u32)) as i64
                                        } else {
                                            i64::MAX
                                        };
                                        let mut placed_extra = 0u32;
                                        'mound: for dy in -place_radius..=place_radius {
                                            for dz in -place_radius..=place_radius {
                                                for dx in -place_radius..=place_radius {
                                                    if dx * dx + dy * dy + dz * dz > r2 {
                                                        continue;
                                                    }
                                                    if dx == 0 && dy == 0 && dz == 0 {
                                                        continue;
                                                    }
                                                    if budget <= 0 {
                                                        break 'mound;
                                                    }
                                                    let (bx, by, bz) = (x + dx, y + dy, z + dz);
                                                    if !matches!(chunk_manager.block_at(bx, by, bz), BlockId::Air) {
                                                        continue;
                                                    }
                                                    // never bury the player
                                                    let overlaps = body.as_ref().map_or(false, |b| {
                                                        let mn = b.aabb_min();
                                                        let mx = b.aabb_max();
                                                        (bx as f32) < mx[0] && (bx as f32 + 1.0) > mn[0]
                                                            && (by as f32) < mx[1] && (by as f32 + 1.0) > mn[1]
                                                            && (bz as f32) < mx[2] && (bz as f32 + 1.0) > mn[2]
                                                    });
                                                    if overlaps {
                                                        continue;
                                                    }
                                                    if chunk_manager.set_block(bx, by, bz, blk) {
                                                        placed_extra += 1;
                                                        if survival_mode {
                                                            let _ = fg_app.inventory_mut().remove_item(
                                                                unendlich_engine::items::ItemId(blk as u32), 1,
                                                            );
                                                            budget -= 1;
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                        if placed_extra > 0 {
                                            log::info!("[mound] raised {} blocks (r={})", placed_extra, place_radius);
                                        }
                                    }
                                    // r512/r520/r521: planting on soil. A crop becomes a SEEDLING
                                    // (Sprout) that grows into the full crop; a Sprout item (a tree
                                    // sapling, dropped by leaves) becomes an OakLog the sapling tick
                                    // matures into a tree (needs open sky above).
                                    if matches!(blk, BlockId::Carrot | BlockId::Potato | BlockId::Cabbage | BlockId::Sprout)
                                        && matches!(chunk_manager.block_at(x, y - 1, z), BlockId::Grass | BlockId::Dirt)
                                    {
                                        if blk == BlockId::Sprout {
                                            let clear = (1..=6).all(|u| {
                                                matches!(chunk_manager.block_at(x, y + u, z), BlockId::Air)
                                            });
                                            if clear {
                                                chunk_manager.set_block(x, y, z, BlockId::OakLog);
                                                session.flora.plant(
                                                    [x, y, z],
                                                    unendlich_engine::world::flora_store::FloraCell::sown(
                                                        BlockId::OakLog as u8, frame_count,
                                                    ),
                                                );
                                                log::info!("[farm] planted a tree sapling at ({},{},{})", x, y, z);
                                            } else {
                                                // not enough room — leave the sprout block as placed.
                                                log::info!("[farm] a tree needs open sky above to grow");
                                            }
                                        } else {
                                            let target = match blk {
                                                BlockId::Potato => 1u8,
                                                BlockId::Cabbage => 2u8,
                                                _ => 0u8,
                                            };
                                            chunk_manager.set_block(x, y, z, BlockId::Sprout);
                                            let mut fc = unendlich_engine::world::flora_store::FloraCell::sown(
                                                BlockId::Sprout as u8, frame_count,
                                            );
                                            fc.element = target;
                                            // r522: genome = this crop's seed line crossed with a fresh draw.
                                            let fresh = unendlich_engine::sim::plant_tree_genetics::PlantGenome::from_seed(
                                                fg_world_seed
                                                    ^ ((x as u64) << 20)
                                                    ^ ((z as u64) << 4)
                                                    ^ frame_count,
                                            );
                                            let g = unendlich_engine::sim::plant_tree_genetics::cross(
                                                &seed_stock[target as usize], &fresh,
                                                fg_world_seed ^ frame_count.wrapping_mul(0x9E37_79B9),
                                            );
                                            fc.genome = g.as_array();
                                            session.flora.plant([x, y, z], fc);
                                            log::info!(
                                                "[farm] sowed {:?} seedling at ({},{},{}) — genome g/{:.2} f/{:.2} h/{:.2}",
                                                blk, x, y, z, g.growth_rate, g.fruitfulness, g.hardiness
                                            );
                                        }
                                    }
                                    last_block_action = Instant::now();
                                    session.lan_outgoing_edits.push(
                                        unendlich_engine::sim::world_authority::WorldCmd::Place {
                                            pos: (x, y, z),
                                            block: blk as u8 as u16,
                                        },
                                    );
                                }
                            } else {
                                last_block_action = Instant::now(); // eat/deny still cools down
                            }
                        }
                    } else if !click_fed_creature {
                        let go = if !survival_mode {
                                Some(selected_block)
                            } else if matches!(selected_block, BlockId::Carrot | BlockId::Potato | BlockId::Cabbage | BlockId::Sprout)
                                && matches!(chunk_manager.block_at(x, y - 1, z), BlockId::Grass | BlockId::Dirt)
                                && fg_app.inventory().count(unendlich_engine::items::ItemId(selected_block as u32)) > 0
                            {
                                // r512: PLANT on tilled soil — consume one and place it (registered below).
                                let _ = fg_app.inventory_mut().remove_item(unendlich_engine::items::ItemId(selected_block as u32), 1);
                                Some(selected_block)
                            } else {
                                survival_use_item(&mut fg_app, selected_block, hand_item)
                            };
                        if let Some(blk) = go {
                            if chunk_manager.set_block(x, y, z, blk) {
                                log::info!("Placed {:?} at ({},{},{}) on {:?} face", blk, x, y, z, hit_face);
                                water_dirty = true; // opt-1: terrain changed
                                    // r565: BIGGER MOUNDS (creative/no-body place path).
                                    if place_radius > 0
                                        && !matches!(blk, BlockId::Carrot | BlockId::Potato | BlockId::Cabbage | BlockId::Sprout)
                                    {
                                        let r2 = place_radius * place_radius;
                                        let mut budget: i64 = if survival_mode {
                                            fg_app.inventory().count(unendlich_engine::items::ItemId(blk as u32)) as i64
                                        } else {
                                            i64::MAX
                                        };
                                        let mut placed_extra = 0u32;
                                        'mound2: for dy in -place_radius..=place_radius {
                                            for dz in -place_radius..=place_radius {
                                                for dx in -place_radius..=place_radius {
                                                    if dx * dx + dy * dy + dz * dz > r2 {
                                                        continue;
                                                    }
                                                    if dx == 0 && dy == 0 && dz == 0 {
                                                        continue;
                                                    }
                                                    if budget <= 0 {
                                                        break 'mound2;
                                                    }
                                                    let (bx, by, bz) = (x + dx, y + dy, z + dz);
                                                    if !matches!(chunk_manager.block_at(bx, by, bz), BlockId::Air) {
                                                        continue;
                                                    }
                                                    let overlaps = body.as_ref().map_or(false, |b| {
                                                        let mn = b.aabb_min();
                                                        let mx = b.aabb_max();
                                                        (bx as f32) < mx[0] && (bx as f32 + 1.0) > mn[0]
                                                            && (by as f32) < mx[1] && (by as f32 + 1.0) > mn[1]
                                                            && (bz as f32) < mx[2] && (bz as f32 + 1.0) > mn[2]
                                                    });
                                                    if overlaps {
                                                        continue;
                                                    }
                                                    if chunk_manager.set_block(bx, by, bz, blk) {
                                                        placed_extra += 1;
                                                        if survival_mode {
                                                            let _ = fg_app.inventory_mut().remove_item(
                                                                unendlich_engine::items::ItemId(blk as u32), 1,
                                                            );
                                                            budget -= 1;
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                        if placed_extra > 0 {
                                            log::info!("[mound] raised {} blocks (r={})", placed_extra, place_radius);
                                        }
                                    }
                                    // r512/r520/r521: planting on soil. A crop becomes a SEEDLING
                                    // (Sprout) that grows into the full crop; a Sprout item (a tree
                                    // sapling, dropped by leaves) becomes an OakLog the sapling tick
                                    // matures into a tree (needs open sky above).
                                    if matches!(blk, BlockId::Carrot | BlockId::Potato | BlockId::Cabbage | BlockId::Sprout)
                                        && matches!(chunk_manager.block_at(x, y - 1, z), BlockId::Grass | BlockId::Dirt)
                                    {
                                        if blk == BlockId::Sprout {
                                            let clear = (1..=6).all(|u| {
                                                matches!(chunk_manager.block_at(x, y + u, z), BlockId::Air)
                                            });
                                            if clear {
                                                chunk_manager.set_block(x, y, z, BlockId::OakLog);
                                                session.flora.plant(
                                                    [x, y, z],
                                                    unendlich_engine::world::flora_store::FloraCell::sown(
                                                        BlockId::OakLog as u8, frame_count,
                                                    ),
                                                );
                                                log::info!("[farm] planted a tree sapling at ({},{},{})", x, y, z);
                                            } else {
                                                // not enough room — leave the sprout block as placed.
                                                log::info!("[farm] a tree needs open sky above to grow");
                                            }
                                        } else {
                                            let target = match blk {
                                                BlockId::Potato => 1u8,
                                                BlockId::Cabbage => 2u8,
                                                _ => 0u8,
                                            };
                                            chunk_manager.set_block(x, y, z, BlockId::Sprout);
                                            let mut fc = unendlich_engine::world::flora_store::FloraCell::sown(
                                                BlockId::Sprout as u8, frame_count,
                                            );
                                            fc.element = target;
                                            // r522: genome = this crop's seed line crossed with a fresh draw.
                                            let fresh = unendlich_engine::sim::plant_tree_genetics::PlantGenome::from_seed(
                                                fg_world_seed
                                                    ^ ((x as u64) << 20)
                                                    ^ ((z as u64) << 4)
                                                    ^ frame_count,
                                            );
                                            let g = unendlich_engine::sim::plant_tree_genetics::cross(
                                                &seed_stock[target as usize], &fresh,
                                                fg_world_seed ^ frame_count.wrapping_mul(0x9E37_79B9),
                                            );
                                            fc.genome = g.as_array();
                                            session.flora.plant([x, y, z], fc);
                                            log::info!(
                                                "[farm] sowed {:?} seedling at ({},{},{}) — genome g/{:.2} f/{:.2} h/{:.2}",
                                                blk, x, y, z, g.growth_rate, g.fruitfulness, g.hardiness
                                            );
                                        }
                                    }
                                last_block_action = Instant::now();
                                session.lan_outgoing_edits.push(
                                    unendlich_engine::sim::world_authority::WorldCmd::Place {
                                        pos: (x, y, z),
                                        block: blk as u8 as u16,
                                    },
                                );
                            }
                        } else {
                            last_block_action = Instant::now();
                        }
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
                    if session.lava_registered.contains(&coord) { continue; }
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
                    session.lava_registered.insert(coord);
                    continue;
                }
                if let Some(entry) = chunks_arc.get(&coord) {
                    let chunk = entry.value();
                    let added = register_lava_heat_sources(chunk, &heat_sources);
                    if added > 0 {
                        log::debug!("Registered {} lava heat sources from chunk {:?}", added, coord);
                    }
                } // entry dropped here, shard lock released.
                session.lava_registered.insert(coord);
                registered_this_tick += 1;
            }
            // Step 2: tear down sources for evicted chunks. For any
            // coord in our set NOT in chunks_arc, dereg by bbox.
            let mut to_unregister: Vec<ChunkCoord> = Vec::new();
            for coord in &session.lava_registered {
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
                session.lava_registered.remove(&coord);
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
        // r200: on a chunk-boundary crossing, ENQUEUE out-of-range chunks
        // rather than evicting them all this frame (the movement-stutter
        // spike). The queue drains at a fixed budget below.
        if player_chunk_changed {
            let loaded: Vec<ChunkCoord> = renderer.loaded_chunk_coords().collect();
            for c in loaded.into_iter().filter(|c| !in_render_distance(*c)) {
                session.fg_evict_queue.push_back(c);
            }
        }
        // Drain up to the per-frame budget. Re-check range at drain time:
        // a chunk that came back into range (player reversed) is simply
        // dropped from the queue instead of evicted.
        if !session.fg_evict_queue.is_empty() {
            let mut batch: Vec<ChunkCoord> = Vec::with_capacity(EVICT_BUDGET_PER_FRAME);
            while batch.len() < EVICT_BUDGET_PER_FRAME {
                let Some(c) = session.fg_evict_queue.pop_front() else { break };
                if !in_render_distance(c) {
                    batch.push(c);
                }
            }
            if !batch.is_empty() {
                renderer.evict_chunks(&batch);
            }
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

            // r202: walking the WHOLE sphere every moving frame was the
            // dominant frame cost (log: submit=400-850ms in bursts — the
            // reported "stutter when moving"). The all-tiers-full early
            // exit never fired because outer tiers (Far/VeryFar at shells
            // 9+) are mostly UNGENERATED on a fresh/moving world, so their
            // budgets never fill, so the walk traversed every shell doing
            // a Vec alloc + desired_lod_for + DashMap read per coord.
            // Two cheap cutoffs that preserve correctness (unsubmitted
            // coords are picked up next frame): (a) stop after a run of
            // shells that yielded NO new candidate — the frontier of
            // generated chunks has been passed; (b) stop once total
            // candidates reach the queue capacity (can't submit more).
            const EMPTY_SHELL_CUTOFF: i32 = 3; // consecutive empty shells
            let mut empty_shell_run: i32 = 0;
            let total_cands = |n: &Vec<_>, m: &Vec<_>, f: &Vec<_>, v: &Vec<_>| {
                n.len() + m.len() + f.len() + v.len()
            };

            'shells: for shell in 0..=rd {
                // r293: time-budget cutoff — if the scan has used its per-frame
                // wall-time budget, stop here and resume from shell 0 next frame.
                // Shells are visited near→far (forward-biased), so the chunks left
                // unscanned are the farthest/least-urgent; they're picked up next
                // frame exactly like the existing empty-shell/candidate cutoffs.
                // (shell > 0 so the innermost ring is never starved by a stale clock.)
                if submit_budget_ms > 0.0
                    && shell > 0
                    && t_submit_start.elapsed().as_secs_f64() * 1e3 >= submit_budget_ms
                {
                    break 'shells;
                }
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
                // r202: total-candidate cap — the mesh queue can't take
                // more than its capacity this frame.
                if total_cands(&near_cands, &mid_cands, &far_cands, &vfar_cands)
                    >= MESH_QUEUE_CAPACITY
                {
                    break 'shells;
                }
                let cands_before_shell =
                    total_cands(&near_cands, &mid_cands, &far_cands, &vfar_cands);


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
                // r202: empty-shell cutoff. If this shell added nothing,
                // count it; after EMPTY_SHELL_CUTOFF consecutive empties we
                // have walked past the generated frontier and the rest of
                // the sphere is wasted DashMap reads — stop. Any shell that
                // yields a candidate resets the run. (Shells 0..1 are tiny,
                // so a brief early gap can't trip it prematurely; the cutoff
                // of 3 tolerates a sparse band.)
                let cands_after_shell =
                    total_cands(&near_cands, &mid_cands, &far_cands, &vfar_cands);
                if cands_after_shell == cands_before_shell {
                    empty_shell_run += 1;
                    if empty_shell_run >= EMPTY_SHELL_CUTOFF {
                        break 'shells;
                    }
                } else {
                    empty_shell_run = 0;
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
                // r293: the prefetch scan is part of the same submit budget — if
                // the shell walk already used it, defer prefetch to a later frame
                // (prefetch is the least-urgent work: not-yet-visible chunks).
                if submit_budget_ms > 0.0
                    && t_submit_start.elapsed().as_secs_f64() * 1e3 >= submit_budget_ms
                {
                    break;
                }
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
                            epoch: chunk_manager.world_epoch(), // r195: stamp at submit
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
            // r195: cross-WORLD staleness — a result meshed against the
            // previous seed can collide with a freshly-loaded coord at the
            // same version 0 in the new world. The version check can't see
            // that; the epoch can. This closes the "sticky/polluted mesher
            // after re-entering with a new seed" path.
            if !unendlich_engine::world::mesh_epoch::mesh_result_is_current(
                result.epoch, chunk_manager.world_epoch())
            {
                continue;
            }
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
        let upload_ms = upload_start.elapsed().as_secs_f64() * 1e3;
        t_upload_us += (upload_ms * 1000.0) as u64;
        // r242: upload is a sub-phase of the per-frame cost; profile its tail
        // so a chunk-upload spike is distinguishable from a meshing/apply one.
        if profile_on { prof.record("upload", upload_ms); }

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
            // r129: SMOKESCREEN visual — standing in dense smoke multiplies
            // fog density (up to 6x), so the screen actually screens.
            if in_smoke {
                fog_density *= 6.0;
            }
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
                wave_speed: water_wave_speed,
            }
        };

        // r420: [water] diagnostics — reveals what actually drives the surface so we can tell
        // a windspeed bug from a shader-tuning issue. tod_rate≈1.0 means the wave clock runs
        // real-time (not accelerated); speed_scale mirrors the shader's wind→speed mapping;
        // |wind| vs len(wind) disagreeing would point at a wind-magnitude bug.
        if renderer.ui_debug_master {
            let now_ms = start.elapsed().as_millis() as u64;
            let dt_ms = now_ms.saturating_sub(water_debug_last_ms);
            if dt_ms >= 1000 {
                let dt_s = (dt_ms.max(1) as f32) / 1000.0;
                water_debug_last_ms = now_ms;
                let wx: f32 = atmos.wind_xz[0];
                let wz: f32 = atmos.wind_xz[1];
                let wmag: f32 = atmos.wind_magnitude;
                let temp: f32 = atmos.temperature;
                let world_time: f32 = atmos.time_of_day;
                let wlen = (wx * wx + wz * wz).sqrt();
                let speed_scale = 0.60_f32 + 0.50_f32 * wmag.clamp(0.0_f32, 2.0_f32);
                let tod_rate = (world_time - water_dbg_prev_tod) / dt_s;
                water_dbg_prev_tod = world_time;
                let pp = local_player.view.position;
                let (pfx, pfz) = water_flow_tiles.sample(pp.x as i32, pp.z as i32);
                let pdepth = water_flow_tiles.sample_depth(pp.x as i32, pp.z as i32);
                if renderer.ui_debug_water {
                    log::info!(
                        "[water] wind=({wx:.2},{wz:.2}) |wind|={wmag:.2} len(wind)={wlen:.2} speed_scale={speed_scale:.2} wave_mult={water_wave_speed:.2} temp={temp:.2} tod={world_time:.1}s tod_rate={tod_rate:.2}x flow@player=({pfx:.3},{pfz:.3}) depth@player={pdepth:.2} | sim: cols={dbg_water_cols} moved={dbg_water_changes} flowspd={dbg_flow_speed:.3} active={water_active}"
                    );
                }
                if renderer.ui_debug_echo {
                    // r437: reverb + nearest-wall occlusion the listener's space implies — this is
                    // the live read of sim/voice_spatial, and it spikes when you drop into a cave.
                    let lp = [pp.x, pp.y, pp.z];
                    let solid = |x: i32, y: i32, z: i32| {
                        chunk_manager.block_at(x, y, z).is_solid()
                    };
                    let rv = unendlich_engine::sim::voice_spatial::reverb(lp, 32.0, &solid);
                    log::info!(
                        "[echo] enclosure={:.2} wet={:.2} decay={:.2}s @player=({:.1},{:.1},{:.1})",
                        rv.enclosure, rv.wet, rv.decay, pp.x, pp.y, pp.z
                    );
                }
            }
        }

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
            if let Some(ev) = unendlich_engine::input_codes::input_event_from_glfw(&event) {
                input.handle_event_late(&ev);
            }
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
            renderer.ui_pointer_down = input.is_mouse_button_pressed(MouseBtn::Left);
            renderer.ui_pointer2_down = input.is_mouse_button_pressed(MouseBtn::Right); // r186
            // Clicks whose press+release collapse into one frame (low fps)
            // produce no level edge; forward them as discrete clicks.
            renderer.ui_click_pending =
                (input.is_mouse_button_just_pressed(MouseBtn::Left)
                    && !input.is_mouse_button_pressed(MouseBtn::Left)) as u32;
            renderer.ui_pointer_active = ui_panel_open
                || renderer.settings_open
                || renderer.pause_menu_open
                || renderer.settings_menu_open
                || renderer.craft_menu_open
                || renderer.perf_window_open
                || renderer.menu_open
                || renderer.wizard_open
                || show_inventory; // r171: every UI surface gets pointer events
            // r181: text + edit keys ride alongside pointer events.
            // r314: always forward typed chars to egui — it ignores Text events when
            // no field is focused, so gating on pointer-active only dropped legit
            // keystrokes. Text now registers whenever a field has focus.
            renderer.ui_text_input = std::mem::take(&mut ui_text_frame);
            renderer.ui_backspace = renderer.ui_pointer_active
                && input.is_key_just_pressed(Keycode::Backspace);
            renderer.ui_enter = renderer.ui_pointer_active
                && input.is_key_just_pressed(Keycode::Enter);
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
            if frame_count % 30 == 0 {
                // r126: on-screen HUD (egui card top-left) — same data as the
                // [HUD] log line, visible without a terminal.
                // r545: survival meters for the on-screen bars (hunger + thermal now;
                // thirst placeholder until Roll 3). Hidden in creative mode.
                renderer.hud_survival = if survival_mode {
                    Some(unendlich_engine::render::world_renderer::SurvivalHud {
                        hunger: fg_app.vitality().energy,
                        thirst: fg_hydration.level,
                        stamina: fg_app.stamina().level,
                        comfort: fg_thermal.comfort,
                        body_c: fg_thermal.body_c,
                    })
                } else {
                    None
                };
                // r576: snapshot the real backpack (occupied slots) for the survival
                // inventory panel, resolved to swatch+name+count via item_display.
                renderer.backpack.clear();
                if survival_mode {
                    for slot in fg_app.inventory().slots() {
                        if let Some(st) = slot {
                            let d = unendlich_engine::item::item_display(st.item.0);
                            renderer.backpack.push(
                                unendlich_engine::render::world_renderer::InvSlotView {
                                    color: d.color,
                                    name: d.name,
                                    count: st.count as u32,
                                },
                            );
                        }
                    }
                }
                // r662: POSITIONAL snapshot (all slots incl. empties) for the
                // interactive 3×9 + hotbar panel, plus the held cursor stack.
                renderer.inv_slots.clear();
                if survival_mode {
                    let inv = fg_app.inventory();
                    renderer.inv_hotbar_size = inv.hotbar_size();
                    for slot in inv.slots() {
                        renderer.inv_slots.push(slot.map(|st| {
                            let d = unendlich_engine::item::item_display(st.item.0);
                            unendlich_engine::render::world_renderer::InvSlotView {
                                color: d.color,
                                name: d.name,
                                count: st.count as u32,
                            }
                        }));
                    }
                }
                renderer.inv_cursor = fg_inv_cursor.map(|st| {
                    let d = unendlich_engine::item::item_display(st.item.0);
                    unendlich_engine::render::world_renderer::InvSlotView {
                        color: d.color,
                        name: d.name,
                        count: st.count as u32,
                    }
                });
                let game_secs = (8 * 3600 + (world_clock_elapsed as u64) * 60) % 86_400;
                renderer.hud_status_lines = vec![
                    format!(
                        "{:02}:{:02}  {}{}",
                        game_secs / 3600,
                        (game_secs % 3600) / 60,
                        if survival_mode { "SURVIVAL" } else { "CREATIVE" },
                        if game_paused { "  (paused)" } else { "" },
                    ),
                    format!(
                        "stamina {:>3.0}%   vitality {:>3.0}%{}",
                        fg_app.stamina().level * 100.0,
                        fg_app.vitality().energy * 100.0,
                        if fg_app.swimming() { "  SWIM" } else { "" },
                    ),
                    // r670: Resonance Rank from lifetime chi (sim::player_rank).
                    format!(
                        "\u{2694} Rank {}   {} chi to next",
                        unendlich_engine::sim::player_rank::rank_for_chi(fg_lifetime_chi),
                        unendlich_engine::sim::player_rank::chi_to_next(fg_lifetime_chi),
                    ),
                    {
                        // r159: tamed companions are a stat you care about.
                        let companions = roamers
                            .iter()
                            .filter(|r| {
                                matches!(
                                    r.bond,
                                    unendlich_engine::sim::bond_tame::Bond::Tamed
                                )
                            })
                            .count();
                        format!(
                            "inv {} slot(s)   drops {}{}",
                            fg_app.inventory().slots().iter().filter(|s| s.is_some()).count(),
                            fg_app.drops().len(),
                            if companions > 0 {
                                format!("   companions {companions}")
                            } else {
                                String::new()
                            },
                        )
                    },
                    // r158: the survival senses line — body thermal state
                    // (r142), live weather (r156), bending resonance (r131).
                    format!(
                        "body {:>4.1}\u{00b0}C{}   resonance {:>3.0}%{}",
                        fg_thermal.body_c,
                        if precip01 > 0.6 {
                            "  STORM"
                        } else if precip01 > 0.15 {
                            "  rain"
                        } else {
                            ""
                        },
                        bend_resonance.charge01 * 100.0,
                        if bend_resonance.is_overdrawn() { "  OVERDRAWN" } else { "" },
                    ),
                ];
                // r670: announce the last-cast technique for ~1.6s (named via the
                // catalogue at cast time). Ties the HUD to the bending move set.
                if let Some((tname, when)) = fg_last_technique {
                    if when.elapsed().as_secs_f32() < 1.6 {
                        renderer.hud_status_lines.push(format!("\u{25c8} {}", tname));
                    } else {
                        fg_last_technique = None;
                    }
                }
                // r674: announce an environment-unlocked hybrid discipline for ~2s.
                if let Some((hname, when)) = fg_last_hybrid {
                    if when.elapsed().as_secs_f32() < 2.0 {
                        renderer.hud_status_lines.push(format!("\u{2735} {} bending available", hname));
                    } else {
                        fg_last_hybrid = None;
                    }
                }
                // r517: BUILD/MINE FEEDBACK — show what you're aiming at (block + cell), and
                // whether it's a mineable target in reach. Reuses the same octree raycast the
                // break/place path uses, so the readout matches exactly what an action would hit.
                {
                    let cf = local_player.forward();
                    if let Some(hit) = chunk_manager.raycast_octree(
                        [pos.x, pos.y, pos.z],
                        [cf.x, cf.y, cf.z],
                        6.0,
                        |b| b.is_solid(),
                    ) {
                        let (hx, hy, hz) = hit.world_cell;
                        let tier = unendlich_engine::mining::required_tier(hit.block);
                        let gate = if tier == unendlich_engine::mining::UNBREAKABLE {
                            "  [unbreakable]"
                        } else {
                            ""
                        };
                        renderer.hud_status_lines.push(format!(
                            "▸ {:?} ({},{},{}){}",
                            hit.block, hx, hy, hz, gate
                        ));
                    }
                }
                // r509: show the held weapon's durability when one is equipped.
                if let Some((_, cur, max)) = equipped_weapon_dura {
                    renderer.hud_status_lines.push(format!(
                        "weapon {}/{}{}",
                        cur.max(0),
                        max,
                        if cur * 5 <= max { "  (worn!)" } else { "" }
                    ));
                }
                // r510: cooking progress while roasting meat at a fire.
                if cook_progress > 0.0 {
                    renderer.hud_status_lines.push(format!(
                        "roasting meat… {:.0}%",
                        (cook_progress / COOK_TIME * 100.0).min(100.0)
                    ));
                }
                // r506: MULTIPLAYER PRESENCE — when LAN peers are connected, list who's
                // online on the HUD. Names come from each remote's chosen character
                // (CH_AVATAR appearance sync, r494); falls back to the entity id otherwise.
                if !lan_remote_creatures.is_empty() {
                    let mut names: Vec<String> = lan_remote_creatures
                        .keys()
                        .map(|id| {
                            lan_remote_appearance
                                .get(id)
                                .map(|p| p.name.clone())
                                .unwrap_or_else(|| format!("player#{id}"))
                        })
                        .collect();
                    names.sort();
                    renderer.hud_status_lines.push(format!(
                        "online ({}): you, {}",
                        names.len() + 1,
                        names.join(", ")
                    ));
                }
            }
            if frame_count % 120 == 0 && mouse_captured {
                let game_secs = (8 * 3600 + (world_clock_elapsed as u64) * 60) % 86_400;
                log::info!(
                    "[HUD] {:02}:{:02} | {} | {} | stamina {:>3.0}% | vitality {:>3.0}%{} | hotbar slot {} | inv {} occupied | ground drops {}",
                    game_secs / 3600,
                    (game_secs % 3600) / 60,
                    if survival_mode { "SURVIVAL" } else { "CREATIVE" },
                    if game_paused { "PAUSED" } else { "RUNNING" },
                    fg_app.stamina().level * 100.0,
                    fg_app.vitality().energy * 100.0,
                    if fg_app.swimming() { " (SWIM)" } else { "" },
                    fg_app.inventory().selected() + 1,
                    fg_app.inventory().slots().iter().filter(|s| s.is_some()).count(),
                    fg_app.drops().len(),
                );
            }
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
            // r247: time the per-frame creature mesh rebuild. The r245 profile
            // localized the stutter to game_update (not draw/streaming); the
            // microbenchmarks (tools/perf_benches) ruled out the AI scans and
            // implicated THIS block — it regenerates the full multi-box geometry
            // for all ~200 creatures into fresh Vecs + re-uploads every frame,
            // even when nothing moved. This zone confirms it directly: if
            // creature_mesh p95 tracks game_update's tail, the dirty-flag fix
            // (cache + rebuild-on-change, reuse the Vecs) is the win. See
            // docs/design/PERF_FINDING_creature_mesh.md.
            let _t_cmesh = Instant::now();
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
            // r400: remote-player avatars (LAN multiplayer), same bake path.
            for c in lan_remote_creatures.values() {
                let (cv, ci) = creature_to_vertices(c);
                let base = everts.len() as u32;
                everts.extend(cv);
                eindices.extend(ci.iter().map(|i| i + base));
            }
            // r490: the player's own character, shown only in third-person view.
            // Built from active_character (rebuilt when the pick changes); parked
            // at the player's feet and turned to face the look yaw.
            // r490/r515: the player's own character — shown in third-person and cinematic
            // views, and in first-person when `fp_body` is on (look down to see your body).
            if camera_mode != 0 || fp_body {
                if self_avatar_dirty || self_avatar.is_none() {
                    let p = local_player.view.position;
                    // r561: recompute the stature profile for the (possibly new)
                    // character, then place the avatar's feet at the player's TRUE feet
                    // (camera − the stature eye height) and resize the collider to match.
                    {
                        let bp = &active_character.build;
                        // r567: density tracks the muscle/bulk slider (muscle is dense →
                        // sinks; lean → floats), and strength too, so buoyancy/knockback get
                        // real variation without a new slider.
                        player_stature = unendlich_engine::sim::stature::derive(
                            bp.height, bp.bulk, bp.bulk, bp.bulk,
                        );
                        player_eye_height = player_stature.eye_height;
                        player_eye_offset =
                            player_stature.eye_height - player_stature.half_extents[1];
                        player_dmg_resist = unendlich_engine::sim::stature::BASE_HEALTH
                            / player_stature.max_health;
                        // resize the player collider to the stature, keeping the feet
                        // planted (shift the AABB centre by the half-height delta).
                        if let Some(b) = body.as_mut() {
                            let old_half = b.half_extents[1];
                            b.half_extents = player_stature.half_extents;
                            b.position[1] += player_stature.half_extents[1] - old_half;
                        }
                    }
                    let pos = [p.x, p.y - player_eye_height, p.z];
                    self_avatar = Some(
                        if matches!(
                            active_character.arche(),
                            unendlich_engine::creature_design::Arche::Dragon
                        ) {
                            // r531: actual dragon characters render as the forge dragon.
                            // The elemental "hero" types are humanoid (Arche::Biped) and
                            // take the blueprint path below — they are NOT dragons.
                            unendlich_engine::spawn::spawn_dragon_arche(active_character.seed, pos)
                        } else {
                            let bp = active_character.blueprint();
                            unendlich_engine::spawn::from_blueprint(&bp, pos)
                        },
                    );
                    self_avatar_dirty = false;
                }
                if let Some(av) = self_avatar.as_mut() {
                    let p = local_player.view.position;
                    // r561: feet at the player's true feet for this stature.
                    let cur = [p.x, p.y - player_eye_height, p.z];
                    // derive velocity from the per-frame move so the pose rig
                    // (legs/arms/wings) animates as the player walks/flies.
                    if let Some(prev) = self_avatar_prev {
                        let inv_dt = if frame_dt_s > 1e-4 { 1.0 / frame_dt_s } else { 0.0 };
                        av.body.velocity = [
                            (cur[0] - prev[0]) * inv_dt,
                            (cur[1] - prev[1]) * inv_dt,
                            (cur[2] - prev[2]) * inv_dt,
                        ];
                    }
                    self_avatar_prev = Some(cur);
                    av.body.position = cur;
                    // r545 (Roll 2 / B1): advance the avatar's animation clocks so the
                    // velocity-driven wing pose actually beats and the legs step. These
                    // were only ticked for roamers, so the playable dragon picked the
                    // Beating pose but held it frozen. anim_time drives the wing-beat/idle
                    // sway; gait_phase advances with horizontal speed for the leg cycle.
                    // (Forge heroes use their own hero_gait_phase, so this is dragon/
                    // blueprint-avatar only in effect.)
                    av.anim_time = (av.anim_time + frame_dt_s) % (std::f32::consts::TAU * 256.0);
                    let spd_h = av.body.velocity[0].hypot(av.body.velocity[2]).min(12.0);
                    av.gait_phase =
                        (av.gait_phase + frame_dt_s * (2.0 + spd_h)) % std::f32::consts::TAU;
                    av.yaw = local_player.view.yaw;
                    // r534: NECK TRACKING — tilt the avatar's head/neck up/down with the
                    // look pitch (gaze drives the head voxels in the vertex builder), so
                    // looking up or down moves the head out of the camera's way instead of
                    // the body squarely blocking the view. Clamped to a natural neck range.
                    av.gaze_yaw = 0.0; // body yaw already faces the look direction
                    av.gaze_pitch = local_player.view.pitch.clamp(-0.95, 0.95);
                    // r538: forge heroes (Heliographer, Windwalker, Tidecaller, Stormweaver,
                    // Gravitybender) render through the dedicated parameterized voxel hero
                    // builder, shaped by the character's BuildParams sliders. Everyone else
                    // uses the generic blueprint avatar. `av` is still updated above so its
                    // velocity/gaze are current (used for the non-hero path + future anim).
                    let (cv, ci) = if let Some(kind) =
                        unendlich_engine::hero_forge::HeroKind::from_name(&active_character.name)
                    {
                        // walk cycle: amplitude follows horizontal speed (settles to 0
                        // when standing), phase advances a touch faster as you move.
                        let spd = (av.body.velocity[0] * av.body.velocity[0]
                            + av.body.velocity[2] * av.body.velocity[2])
                            .sqrt();
                        let target_amp = (spd * 0.11).clamp(0.0, 0.5);
                        let k = (frame_dt_s * 8.0).min(1.0);
                        hero_gait_amp += (target_amp - hero_gait_amp) * k;
                        // r568: ANTI FOOT-SLIDE. The swing amplitude already scales with
                        // speed (target_amp = spd·0.11 above), so the planted foot's ground
                        // speed at mid-stance ≈ leg_len · amp · dφ/dt. For that to track body
                        // speed at ALL speeds, the phase RATE must be CONSTANT — cadence stays
                        // fixed while stride LENGTH grows with speed, exactly as in real gait.
                        // The rate is tuned to leg length (≈ ½·height), so a tall body strides
                        // at a slower cadence (and, with its longer legs, covers more ground per
                        // step). When stopped, amp→0 hides the still-advancing phase.
                        let leg_len = 0.5 * player_stature.height;
                        let gait_rate = 1.0 / (leg_len * 0.11).max(0.05);
                        hero_gait_phase += frame_dt_s * gait_rate;
                        if hero_gait_phase > std::f32::consts::TAU {
                            hero_gait_phase -= std::f32::consts::TAU;
                        }
                        // (walk_phase + walk_amp are passed to the hero rig, which
                        // derives the limb swing / bob / twist itself.)
                        // r545: equipped-item colour for the hero's hand (simple
                        // category map; None = empty-handed) + a continuous anim clock
                        // for idle breathing.
                        let held_color = hand_item.map(|h| match h.0 {
                            4000..=4007 => [0.72, 0.74, 0.80], // forged weapon — steel
                            1000 => [0.85, 0.72, 0.30],        // gold ingot
                            1001 => [0.78, 0.78, 0.82],        // iron ingot
                            2000 => [0.75, 0.30, 0.28],        // raw meat
                            2001 => [0.62, 0.40, 0.26],        // cooked meat
                            _ => [0.55, 0.45, 0.35],           // generic held item
                        });
                        // r570: airborne/landing pose drive. Ease `hero_airborne01` toward
                        // 1 while off the ground (body=None ⇒ creative flight ⇒ also airborne),
                        // and decay the landing crouch pulse over ~¼ s. vertical_vel comes from
                        // the real player body so the fall/rise lean tracks actual motion.
                        let (vvel, target_air) = match body.as_ref() {
                            Some(b) => (b.velocity[1], if b.on_ground { 0.0 } else { 1.0 }),
                            None => (0.0, 1.0), // flying
                        };
                        hero_airborne01 += (target_air - hero_airborne01)
                            * (frame_dt_s * 9.0).min(1.0);
                        hero_land_crouch = (hero_land_crouch - frame_dt_s * 4.0).max(0.0);
                        // r572: flight blend — go horizontal "superman" while actively flying
                        // (creative flight = no physics body) and moving; ease back upright when
                        // hovering or grounded.
                        let fly_target = if body.is_none() && spd > 1.5 { 1.0 } else { 0.0 };
                        hero_flight01 += (fly_target - hero_flight01) * (frame_dt_s * 5.0).min(1.0);
                        // r573: bank into flight turns — roll proportional to the view's yaw
                        // rate (wrapped), smoothed and clamped, so a heading change arcs.
                        let yaw_now = local_player.view.yaw;
                        let mut dyaw = yaw_now - prev_view_yaw;
                        while dyaw > std::f32::consts::PI {
                            dyaw -= std::f32::consts::TAU;
                        }
                        while dyaw < -std::f32::consts::PI {
                            dyaw += std::f32::consts::TAU;
                        }
                        prev_view_yaw = yaw_now;
                        let yaw_rate = if frame_dt_s > 1e-4 { dyaw / frame_dt_s } else { 0.0 };
                        let bank_target = (-yaw_rate * 0.22).clamp(-0.7, 0.7);
                        hero_bank += (bank_target - hero_bank) * (frame_dt_s * 6.0).min(1.0);
                        let pose = unendlich_engine::hero_forge::HeroPose {
                            yaw: local_player.view.yaw,
                            gaze_pitch: av.gaze_pitch,
                            walk_phase: hero_gait_phase,
                            walk_amp: hero_gait_amp,
                            anim_t: start.elapsed().as_secs_f32(),
                            held_color,
                            airborne: hero_airborne01,
                            vertical_vel: vvel,
                            land_crouch: hero_land_crouch,
                            flight01: hero_flight01,
                            bank: hero_bank,
                        };
                        unendlich_engine::hero_forge::build(
                            kind,
                            &active_character.build,
                            cur,
                            &pose,
                        )
                    } else {
                        creature_to_vertices(av)
                    };
                    let base = everts.len() as u32;
                    // r535: AVATAR FADE (real alpha). As the body obscures more of the
                    // camera, fade it out (down to 50% opacity) so it never squarely blocks
                    // the view. The entity pipeline now blends per-vertex alpha, so this is
                    // true transparency. Coverage = fraction of avatar triangles sitting
                    // close + central in front of the render camera. Applies in every view;
                    // in first-person it stacks on the r531 above-eye head clip.
                    let cam_pos = match camera_mode {
                        1 => local_player.view.position
                            - local_player.view.forward() * (4.0 * cam_zoom)
                            + glm::vec3(0.0, 0.6, 0.0),
                        2 => local_player.view.position
                            - local_player.view.forward() * (2.6 * cam_zoom)
                            + local_player.view.right() * 0.9
                            + glm::vec3(0.0, 0.45, 0.0),
                        _ => local_player.view.position,
                    };
                    let cam_fwd = local_player.view.forward();
                    // r562: taller bodies fill the FP view far more, so widen the
                    // coverage net AND deepen the fade with height (= 1.0 at default).
                    let fade_r = (player_stature.height
                        / unendlich_engine::sim::stature::BASE_HEIGHT).max(1.0);
                    let tri_count = ci.len() / 3;
                    let mut covered = 0usize;
                    for tri in ci.chunks_exact(3) {
                        let p0 = cv[tri[0] as usize].position;
                        let p1 = cv[tri[1] as usize].position;
                        let p2 = cv[tri[2] as usize].position;
                        let c = glm::vec3(
                            (p0[0] + p1[0] + p2[0]) / 3.0,
                            (p0[1] + p1[1] + p2[1]) / 3.0,
                            (p0[2] + p1[2] + p2[2]) / 3.0,
                        );
                        let rel = c - cam_pos;
                        let d = glm::dot(&rel, &cam_fwd);
                        if d > 0.0 && d < 2.6 * fade_r {
                            let lat = glm::length(&(rel - cam_fwd * d));
                            if lat < 1.3 * fade_r {
                                covered += 1;
                            }
                        }
                    }
                    let coverage01 = if tri_count > 0 {
                        (covered as f32 / tri_count as f32).clamp(0.0, 1.0)
                    } else {
                        0.0
                    };
                    // opacity 1.0 → floor as coverage rises — applied as real vertex alpha.
                    // r550: gentler fade for a normal body (~45% floor). r562: taller
                    // characters obscure far more, so the fade DEPTH grows with height
                    // (up to ~92% transparent) — a default character is unchanged.
                    let fade_depth = (0.55 * fade_r).min(0.92);
                    let opacity = 1.0 - fade_depth * coverage01;
                    let mut cv = cv;
                    if opacity < 0.999 {
                        for v in cv.iter_mut() {
                            v.alpha = opacity;
                        }
                    }
                    let clip_y = local_player.view.position.y - 0.25; // FP head clip
                    everts.extend(cv.iter().cloned());
                    for tri in ci.chunks_exact(3) {
                        // First-person: drop triangles at/above eye level (r531 head clip).
                        if camera_mode == 0
                            && !tri.iter().all(|&i| cv[i as usize].position[1] < clip_y)
                        {
                            continue;
                        }
                        for &i in tri {
                            eindices.push(i + base);
                        }
                    }
                }
            }
            // r492: falling precipitation (rain/snow) when the weather is wet.
            // Snow when the column is near/below freezing, else rain. Reuses the
            // entity vertex stream — no separate particle pipeline.
            if precip01 > 0.05 {
                let pp = local_player.view.position;
                let pt = sky_state.time_seconds as f32;
                let (pwx, pwz, _pwm) = unendlich_engine::wind::wind_at(
                    pp.x as f64, pp.y as f64, pp.z as f64, sky_state.time_seconds,
                );
                let snowing = last_climate_temp_c <= 1.0;
                let inten = precip01.clamp(0.0, 1.0);
                let (count, fall_speed, sway, half, pcolor) = if snowing {
                    ((140.0 + 360.0 * inten) as usize, 2.2_f32, 0.5_f32,
                     [0.06_f32, 0.06, 0.06], [0.95_f32, 0.97, 1.0])
                } else {
                    ((220.0 + 700.0 * inten) as usize, 14.0_f32, 0.04_f32,
                     [0.02_f32, 0.20, 0.02], [0.60_f32, 0.69, 0.85])
                };
                let drops = unendlich_engine::sim::precipitation::field(
                    [pp.x, pp.y, pp.z], pt, count, 16.0, 18.0, fall_speed, [pwx, pwz], sway,
                );
                let (hx, hy, hz) = (half[0], half[1], half[2]);
                for d in &drops {
                    let base = everts.len() as u32;
                    let p = d.pos;
                    for &(sx, sy, sz) in &[
                        (-hx, -hy, -hz), (hx, -hy, -hz), (hx, -hy, hz), (-hx, -hy, hz),
                        (-hx, hy, -hz), (hx, hy, -hz), (hx, hy, hz), (-hx, hy, hz),
                    ] {
                        everts.push(unendlich_engine::world::entity::EntityVertex {
                            position: [p[0] + sx, p[1] + sy, p[2] + sz],
                            color: pcolor,
                            alpha: 1.0,
                        });
                    }
                    for &i in &[
                        0u32, 1, 2, 0, 2, 3, 4, 6, 5, 4, 7, 6,
                        0, 4, 5, 0, 5, 1, 1, 5, 6, 1, 6, 2,
                        2, 6, 7, 2, 7, 3, 3, 7, 4, 3, 4, 0,
                    ] {
                        eindices.push(base + i);
                    }
                }
            }
            // r493: drifting ICE FLOES on cold water — procedural floes around the
            // player that drift downwind and only appear where the surface column is
            // water. Visual (not collidable yet). Reuses the entity vertex stream.
            if last_climate_temp_c <= 2.0 {
                let pp = local_player.view.position;
                let it = sky_state.time_seconds as f32;
                let (iwx, iwz, _iwm) = unendlich_engine::wind::wind_at(
                    pp.x as f64, pp.y as f64, pp.z as f64, sky_state.time_seconds,
                );
                let sea = unendlich_engine::constants::SEA_LEVEL;
                let cold = ((2.0 - last_climate_temp_c).clamp(0.0, 6.0)) / 6.0; // 0..1
                let n = (10.0 + 50.0 * cold) as usize;
                let radius = 48.0_f32;
                let ihash = |i: u32, s: u32| -> f32 {
                    let mut h = i.wrapping_mul(0x9E37_79B1) ^ s.wrapping_mul(0x85EB_CA77);
                    h ^= h >> 15; h = h.wrapping_mul(0x2C1B_3C6D); h ^= h >> 12;
                    (h & 0x00FF_FFFF) as f32 / 16_777_216.0
                };
                let driftx = iwx * it * 0.30;
                let driftz = iwz * it * 0.30;
                for i in 0..n as u32 {
                    let hx = ihash(i, 0x1); let hz = ihash(i, 0x2); let hs = ihash(i, 0x3);
                    let bx = (hx * 2.0 - 1.0) * radius + driftx;
                    let bz = (hz * 2.0 - 1.0) * radius + driftz;
                    let wx = pp.x + ((bx + radius).rem_euclid(2.0 * radius) - radius);
                    let wz = pp.z + ((bz + radius).rem_euclid(2.0 * radius) - radius);
                    let xi = wx.floor() as i32;
                    let zi = wz.floor() as i32;
                    let on_water = matches!(chunk_manager.block_at(xi, sea, zi), BlockId::Water)
                        && !chunk_manager.block_at(xi, sea + 1, zi).is_solid();
                    if !on_water { continue; }
                    let sx = 0.7 + hs * 1.6;          // floe half-width
                    let sz = sx * (0.7 + ihash(i, 0x4) * 0.5);
                    let y = sea as f32 + 0.95 + (it * 0.5 + hs * 6.28).sin() * 0.05;
                    let yaw = hs * 6.2831 + it * 0.05 * (hx - 0.5); // slow spin
                    let (cs, sn) = yaw.sin_cos();
                    let (cyaw, syaw) = (sn, cs);
                    let col = [0.82_f32, 0.90, 0.97];
                    let base = everts.len() as u32;
                    let hyt = 0.16_f32; // thin slab
                    for &(cx, cy, cz) in &[
                        (-sx, -hyt, -sz), (sx, -hyt, -sz), (sx, -hyt, sz), (-sx, -hyt, sz),
                        (-sx, hyt, -sz), (sx, hyt, -sz), (sx, hyt, sz), (-sx, hyt, sz),
                    ] {
                        let rx = cx * cyaw - cz * syaw;
                        let rz = cx * syaw + cz * cyaw;
                        everts.push(unendlich_engine::world::entity::EntityVertex {
                            position: [wx + rx, y + cy, wz + rz],
                            color: col,
                            alpha: 1.0,
                        });
                    }
                    for &k in &[
                        0u32, 1, 2, 0, 2, 3, 4, 6, 5, 4, 7, 6,
                        0, 4, 5, 0, 5, 1, 1, 5, 6, 1, 6, 2,
                        2, 6, 7, 2, 7, 3, 3, 7, 4, 3, 4, 0,
                    ] {
                        eindices.push(base + k);
                    }
                }
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
                // r120: ground item drops render as small colored cubes (swatch
                // color from the items debug palette; slow spin from anim time).
                {
                    let spin = (frame_count as f32) * 0.03;
                    let palette = unendlich_engine::items::default_palette();
                    // r147: drops afloat bob on the REAL water-wave field —
                    // params derived from the live wind + temperature.
                    let (wavep, surf_adv) = {
                        let ppw = local_player.view.position;
                        let (wx, wz, wm) = unendlich_engine::wind::wind_at(
                            ppw.x as f64, ppw.y as f64, ppw.z as f64, sky_state.time_seconds,
                        );
                        let wp = unendlich_engine::world::water_waves::water_wave_params(
                            wx as f32, wz as f32, wm as f32, 0.5,
                            sky_state.time_seconds as f32,
                        );
                        // r504: match chunk.frag's surface advection (always-on baseline
                        // + stronger wind) so drops ride the same lateral current the
                        // water surface now shows. Keep in lockstep with the shader's
                        // WW_BASE_DRIFT (0.60) / WW_WIND_DRIFT (0.55).
                        let wlen = ((wx * wx + wz * wz).sqrt()).max(1e-3);
                        let (dirx, dirz) = if wlen > 1e-3 {
                            (wx as f32 / wlen as f32, wz as f32 / wlen as f32)
                        } else {
                            (0.7071, 0.7071)
                        };
                        let adv = [
                            dirx * 0.60 + wx as f32 * 0.55,
                            dirz * 0.60 + wz as f32 * 0.55,
                        ];
                        (wp, adv)
                    };
                    let wave_t = sky_state.time_seconds as f32;
                    for d in fg_app.drops().drops() {
                        let c = palette
                            .iter()
                            .find(|e| e.item == d.stack.item)
                            .map(|e| [e.color[0] as f32 / 255.0, e.color[1] as f32 / 255.0, e.color[2] as f32 / 255.0])
                            .unwrap_or_else(|| {
                                // r157: dragon essence (3000+elem) glows in its
                                // element's hue via the REAL emissive model.
                                if (3000..3006).contains(&d.stack.item.0) {
                                    use unendlich_engine::sim::emissive::{emissive_for, Element};
                                    let el = match d.stack.item.0 - 3000 {
                                        0 => Element::Wood,
                                        1 => Element::Water,
                                        2 => Element::Air,
                                        3 => Element::Dust,
                                        4 => Element::Fire,
                                        _ => Element::Storm,
                                    };
                                    let em = emissive_for(el, 1.0, 1.0);
                                    [
                                        (0.3 + em.color[0]).min(1.0),
                                        (0.3 + em.color[1]).min(1.0),
                                        (0.3 + em.color[2]).min(1.0),
                                    ]
                                } else {
                                    // r159: known non-block items get real
                                    // hues; only true unknowns stay golden.
                                    match d.stack.item.0 {
                                        1000 => [0.78, 0.80, 0.86], // iron ingot
                                        1001 => [0.98, 0.84, 0.30], // gold ingot
                                        2000 => [0.82, 0.30, 0.26], // meat
                                        999 => [0.93, 0.89, 0.78],  // egg
                                        _ => [0.95, 0.78, 0.2],     // golden fallback
                                    }
                                }
                            });
                        let base = everts.len() as u32;
                        let h = 0.18_f32;
                        let (s, co) = spin.sin_cos();
                        // r147: floating? bob with the wave at this spot.
                        let bob = if matches!(
                            chunk_manager.block_at(
                                d.pos[0].floor() as i32,
                                (d.pos[1] - 0.25).floor() as i32,
                                d.pos[2].floor() as i32,
                            ),
                            BlockId::Water
                        ) {
                            unendlich_engine::world::water_waves::water_wave_height_grad(
                                d.pos[0] - surf_adv[0] * wave_t,
                                d.pos[2] - surf_adv[1] * wave_t,
                                &wavep,
                            )
                            .0 * 0.5
                        } else {
                            0.0
                        };
                        // 8 cube corners, rotated about Y.
                        for &(cx, cy, cz) in &[
                            (-h, -h, -h), (h, -h, -h), (h, -h, h), (-h, -h, h),
                            (-h, h, -h), (h, h, -h), (h, h, h), (-h, h, h),
                        ] {
                            let rx = cx * co - cz * s;
                            let rz = cx * s + cz * co;
                            everts.push(unendlich_engine::world::entity::EntityVertex {
                                position: [d.pos[0] + rx, d.pos[1] + cy + 0.25 + bob, d.pos[2] + rz],
                                color: c,
                                alpha: 1.0,
                            });
                        }
                        for &i in &[
                            0u32, 1, 2, 0, 2, 3, // bottom
                            4, 6, 5, 4, 7, 6, // top
                            0, 4, 5, 0, 5, 1, // -z
                            1, 5, 6, 1, 6, 2, // +x
                            2, 6, 7, 2, 7, 3, // +z
                            3, 7, 4, 3, 4, 0, // -x
                        ] {
                            eindices.push(base + i);
                        }
                    }
                }
                // held-item model with walk bob + per-form pose, placed in
                // camera space and pushed through the same entity stream.
                // r486: advance the held-item swing — fire on a block action, decay each frame.
                if last_block_action != prev_block_action {
                    swing_timer = SWING_DURATION;
                }
                swing_timer = (swing_timer - frame_dt_s).max(0.0);
                if mouse_captured && camera_mode == 0 && !fp_body && (hand_item.is_some() || debug_weapon_class.is_some()) {
                    use unendlich_engine::sim::viewmodel as vm;
                    use unendlich_engine::sim::elemental::ItemForm;
                    let fwd = local_player.forward();
                    let upw = glm::vec3(0.0f32, 1.0, 0.0);
                    let right = glm::normalize(&glm::cross(&fwd, &upw));
                    let upv = glm::cross(&right, &fwd);
                    let eyep = local_player.view.position;
                    let cam = vm::CameraBasis {
                        pos: [eyep.x, eyep.y, eyep.z],
                        forward: [fwd.x, fwd.y, fwd.z],
                        right: [right.x, right.y, right.z],
                        up: [upv.x, upv.y, upv.z],
                    };
                    let speed01 = (last_player_speed / 5.6).clamp(0.0, 1.0);
                    let walk_phase = (frame_count as f32) * 0.21 * (0.3 + speed01);
                    // r481: weapons take a dedicated model keyed by class; other
                    // hand items fall back to the id->form heuristic.
                    let hid = hand_item.map(|h| h.0);
                    let weapon = debug_weapon_class
                        .map(|i| vm::WEAPON_CLASSES[i])
                        .or_else(|| hid.and_then(vm::weapon_class_for_id));
                    // r485: a held BLOCK renders as a cube in the block's colour
                    // instead of the generic tool fallback.
                    let held_block = if weapon.is_none() {
                        hid.and_then(|id| {
                            unendlich_engine::world::blocks::BlockId::ALL
                                .iter()
                                .copied()
                                .find(|b| *b as u32 == id && id != 0)
                        })
                    } else {
                        None
                    };
                    let form = match (weapon, hid) {
                        (Some(c), _) => vm::weapon_to_form(c),
                        (None, Some(1000)) | (None, Some(1001)) => ItemForm::Blade, // ingots read as metal
                        (None, Some(id)) if id >= 3000 => ItemForm::Staff,          // essence: arcane
                        _ => ItemForm::Tool,
                    };
                    let mut pose = vm::viewmodel_pose(form, walk_phase, speed01, [0.0, 0.0]);
                    // r486: overlay the use-swing — a quick down-forward arc that
                    // peaks mid-swing, on top of the walk pose.
                    if swing_timer > 0.0 {
                        let prog = 1.0 - (swing_timer / SWING_DURATION).clamp(0.0, 1.0);
                        let arc = (prog * std::f32::consts::PI).sin();
                        pose.pitch += arc * 0.95;
                        pose.roll += arc * 0.15;
                        pose.offset[1] -= arc * 0.10;
                        pose.offset[2] -= arc * 0.06;
                    }
                    let hand_boxes = if let Some(c) = weapon {
                        vm::weapon_model(c)
                    } else if let Some(b) = held_block {
                        vm::block_model(b.base_color())
                    } else {
                        vm::item_model(form)
                    };
                    for b in hand_boxes {
                        let (corners, col) = vm::box_corners_in_view(&pose, &b);
                        let base = everts.len() as u32;
                        for cnr in corners {
                            let wp = vm::place_in_world(cnr, &cam);
                            everts.push(unendlich_engine::world::entity::EntityVertex {
                                position: wp,
                                color: [
                                    col[0] as f32 / 255.0,
                                    col[1] as f32 / 255.0,
                                    col[2] as f32 / 255.0,
                                ],
                                alpha: 1.0,
                            });
                        }
                        for &i in &[
                            0u32, 1, 2, 0, 2, 3,
                            4, 6, 5, 4, 7, 6,
                            0, 4, 5, 0, 5, 1,
                            1, 5, 6, 1, 6, 2,
                            2, 6, 7, 2, 7, 3,
                            3, 7, 4, 3, 4, 0,
                        ] {
                            eindices.push(base + i);
                        }
                    }
                }
                let (mv, mi) = mobs_to_vertices(&test_mobs);
                let base = everts.len() as u32;
                everts.extend(mv);
                eindices.extend(mi.iter().map(|i| i + base));
            }
            renderer.upload_entities(&ctx, &everts, &eindices);
            if profile_on {
                prof.record("creature_mesh", _t_cmesh.elapsed().as_secs_f64() * 1e3);
            }
        }
        // ── fullgame: drive title music from menu/app state ──
        {
            use unendlich_engine::sim::{game_flow::AppState, music_director};
            let dt_ms = fg_music_clock.elapsed().as_millis() as u32;
            fg_music_clock = std::time::Instant::now();
            // r166: pause ducks the music too (cue_for(Paused) = 0.35).
            let st = if renderer.menu_open || renderer.wizard_open {
                AppState::MainMenu
            } else if game_paused {
                AppState::Paused
            } else {
                AppState::InGame
            };
            let prev_asset = fg_music.asset();
            fg_music = fg_music.step(&music_director::cue_for(st), dt_ms);
            // r159: the director's decisions are observable — until the
            // audio-backend session lands (rodio/kira), a cue change logs.
            if fg_music.asset() != prev_asset {
                log::info!(
                    "[music] director cue change -> {:?} (vol {:.2})",
                    fg_music.asset(),
                    fg_music.volume01 * renderer.ui_music_vol // r169: settings slider
                );
            }
        }

        // ── fullgame: title-menu camera flythrough ──
        // While the menu/wizard is up, fly the camera along the App's cinematic
        // orbit, offset to the spawn so it sweeps the live world behind the menu.
        // Written HERE (just before the draw, after the body-follow camera update)
        // so it is the last write to the view and actually takes effect.
        if renderer.menu_open || renderer.wizard_open {
            // r167: TITLE CAMERA — the menu backdrop is a slow orbit around
            // the spawn monument, not the player's last in-world state.
            // r186: the camera PANS on navigation (each screen owns a yaw
            // offset) + cursor micro-parallax — frosted v1.5 step one.
            let screen_yaw = match renderer.menu_screen_kind {
                1 => 1.9,                      // confirm-quit looks away
                2 => -1.1,                     // multiplayer pans left
                _ if renderer.wizard_open => 0.8,
                _ => 0.0,
            };
            let (cx, cy) = window.get_cursor_pos();
            let (ww, wh) = window.get_size();
            let par_x = ((cx as f32 / ww.max(1) as f32) - 0.5) * 0.07;
            let par_y = ((cy as f32 / wh.max(1) as f32) - 0.5) * 0.03;
            let yaw_target = screen_yaw + par_x;
            // r191: distance-accelerated fly (operator: more distance
            // acceleration on click) — base 0.9 rad/s + 3.2/s per radian.
            ramp_approach(&mut fg_menu_yaw, yaw_target, dt, 0.9, 3.2);
            ramp_approach(&mut fg_menu_pitch_par, par_y, dt, 0.5, 2.0);
            let th = (frame_count as f32) * 0.0015 + fg_menu_yaw;
            let (ox, oz) = (th.cos() * 46.0, th.sin() * 46.0);
            local_player.view.position = glm::vec3(ox, 96.0, oz);
            local_player.view.yaw = th + std::f32::consts::PI; // face the center
            local_player.view.pitch = -0.35 + fg_menu_pitch_par;
            fg_app.advance(dt);
            let pose = fg_app.camera_pose();
            let eye = glm::vec3(
                pose.eye[0] + spawn_pos.x,
                pose.eye[1] + spawn_pos.y,
                pose.eye[2] + spawn_pos.z,
            );
            let tgt = glm::vec3(
                pose.target[0] + spawn_pos.x,
                pose.target[1] + spawn_pos.y,
                pose.target[2] + spawn_pos.z,
            );
            // r190: PARALLAX FOR REAL — fg_app.camera_pose() OVERWRITES the
            // pre-advance view (the r186 pan fed local_player.view BEFORE
            // this block, so it was discarded — operator: "no parallax").
            // Rotate the FINAL eye+target around the spawn pivot by the
            // smoothed pan, and tilt the final pitch by the cursor term.
            let (s, cgl) = fg_menu_yaw.sin_cos();
            let rot = |p: glm::Vec3| {
                let rx = p.x - spawn_pos.x;
                let rz = p.z - spawn_pos.z;
                glm::vec3(
                    spawn_pos.x + rx * cgl - rz * s,
                    p.y,
                    spawn_pos.z + rx * s + rz * cgl,
                )
            };
            let eye = rot(eye);
            let tgt = rot(tgt);
            let d = glm::normalize(&(tgt - eye));
            local_player.view.position = eye;
            local_player.view.pitch = (d.y.clamp(-1.0, 1.0)).asin() + fg_menu_pitch_par;
            local_player.view.yaw = d.z.atan2(d.x);
            local_player.view.fov_y_radians = pose.fov_deg.to_radians();
        }

        // r506: BODY PUSH-BACK — inject moving bodies' wakes into the flow field BEFORE it's
        // packed to the shader, so this frame's swimmers/mobs disturb this frame's surface.
        // Player wakes when swimming + moving; nearby mobs in water make wakes scaled by their
        // mass (body volume) × speed — a dragon crashing in makes a far bigger wave than a fish.
        if water_pushback && water_flow_shader {
            let now_ms = start.elapsed().as_millis() as u64;
            let pp = local_player.view.position;
            if fg_app.swimming() && last_player_speed > 0.4 {
                let f = local_player.forward();
                let amp = unendlich_engine::sim::water_react::wave_amplitude(
                    80.0, last_player_speed, 1.0, water_pushback_gain, water_pushback_max,
                );
                water_flow_tiles.splash(pp.x as i32, pp.z as i32, f.x, f.z, amp, now_ms);
            }
            for r in roamers.iter() {
                let bp = r.creature.body.position;
                let (dx, dz) = (bp[0] - pp.x, bp[2] - pp.z);
                if dx * dx + dz * dz > 80.0 * 80.0 {
                    continue; // outside the flow window
                }
                if !matches!(
                    chunk_manager.block_at(bp[0].floor() as i32, bp[1].floor() as i32, bp[2].floor() as i32),
                    unendlich_engine::world::blocks::BlockId::Water
                ) {
                    continue; // body isn't actually in water
                }
                let v = r.creature.body.velocity;
                let speed = (v[0] * v[0] + v[2] * v[2]).sqrt();
                if speed < 0.4 {
                    continue;
                }
                let he = r.creature.body.half_extents;
                let mass = (he[0] * he[1] * he[2] * 8.0).max(1.0) * 60.0; // volume × density proxy
                let area = (he[0] * he[2] * 4.0).max(1.0); // frontal cross-section
                let amp = unendlich_engine::sim::water_react::wave_amplitude(
                    mass, speed, area, water_pushback_gain, water_pushback_max,
                );
                water_flow_tiles.splash(bp[0] as i32, bp[2] as i32, v[0], v[2], amp, now_ms);
            }
        }

        // r419 (flow-as-shader): hand the persisted flow tiles to the renderer as a coarse
        // 16×16 grid (32 blocks/cell → ~512-block window) centred on the player. Off → the
        // renderer's enable flag stays 0 and water uses wind only.
        if water_flow_shader {
            let p = local_player.view.position;
            let (cov, grid) = water_flow_tiles.pack_grid(p.x as i32, p.z as i32, 16, 32);
            renderer.set_water_flow(cov, &grid);
        }
        // r432: persistent depth grid (16×16, 8 blocks/cell → 128-block window) from the same
        // tiles, uploaded every frame. Covers everywhere the sim has run (no lag/seam from the
        // sim throttle); never-simulated cells are -1 so the shader keeps its angle fallback.
        if water_depth_color {
            let p = local_player.view.position;
            let (cov, cells) = water_flow_tiles.pack_depth_grid(p.x as i32, p.z as i32, 16, 8);
            let mut grid256 = [-1.0f32; 256];
            grid256[..cells.len().min(256)].copy_from_slice(&cells[..cells.len().min(256)]);
            renderer.set_water_depth(cov, &grid256);
        }

        // r427: underwater screen FX — set submerged state from the eye block each frame.
        if water_underwater_fx {
            let e = local_player.view.position;
            let submerged = matches!(
                chunk_manager.block_at(e.x as i32, e.y as i32, e.z as i32),
                BlockId::Water
            );
            renderer.set_underwater(
                if submerged { 1.0 } else { 0.0 },
                start.elapsed().as_secs_f32(),
            );
        }
        renderer.set_season(current_season01); // r499: foliage seasonal tint

        // r476: timebend snapshot/scrub. While LIVE, record the player's vantage
        // on the capture cadence. While SCRUBBING, override the camera + world
        // clock to the selected past vantage so the live world re-renders exactly
        // as it looked from there. The fade veil (driven below) covers each cut.
        if timebend.active {
            if let Some(s) = timebend.current() {
                local_player.view.position = s.pos;
                local_player.view.yaw = s.yaw;
                local_player.view.pitch = s.pitch;
                sky_state.time_seconds = s.sky_time;
            }
        } else {
            timebend.maybe_capture(
                local_player.view.position,
                local_player.view.yaw,
                local_player.view.pitch,
                sky_state.time_seconds,
            );
        }
        timebend.tick_fade(dt);
        // Drive the existing veil from the scrub fade (max so we never undercut a
        // genuine loading fade). While scrubbing, hold a light floor so the HUD
        // label + a subtle vignette persist between cuts. r476.
        let veil = if timebend.active {
            timebend.fade.max(0.10)
        } else {
            timebend.fade
        };
        if veil > renderer.loading_fade {
            renderer.loading_fade = veil;
        }
        if timebend.active {
            renderer.loading_label = timebend.hud_label();
        }

        // r490: in third-person, render from a camera pulled back behind the eye
        // (the look direction is unchanged, so the avatar sits centered ahead).
        // Everything else — raycast, physics, culling, audio — still uses the eye.
        // r537: in the pulled-back views, march the camera offset against the world
        // and pinch it in so terrain can't wedge between the camera and the player
        // (tight corridors, walls at your back). Applies in every pulled-back view.
        // r562: chase cameras pull back, rise, and tilt down as the character grows,
        // so a tall hero stays framed. `hz` = height / 1.8 (≥1); = 1.0 (no change) for
        // a default character. Tilt is capped so it never looks straight down.
        let cam_hz = (player_stature.height / unendlich_engine::sim::stature::BASE_HEIGHT).max(1.0);
        let cam_tilt = (0.10 * (cam_hz - 1.0)).clamp(0.0, 0.45);
        // r564: raise the chase camera well above the head as the character grows, so
        // tilting down still frames the WHOLE body (the top no longer clips out). The
        // lift adds ~0.6 per extra block of stature on top of the base lift.
        let cam_lift_extra = 0.6 * (player_stature.height
            - unendlich_engine::sim::stature::BASE_HEIGHT).max(0.0);
        let mut render_view = match camera_mode {
            1 => {
                let mut v = local_player.view;
                let eye = local_player.view.position;
                let f = local_player.view.forward();
                // desired: straight back behind the look direction, lifted a little.
                let target = eye - f * (4.0 * cam_zoom * cam_hz)
                    + glm::vec3(0.0, 0.6 * cam_hz + cam_lift_extra, 0.0);
                let offset = target - eye;
                let dist = glm::length(&offset);
                let dir = if dist > 1e-4 { offset / dist } else { -f };
                let pinched = pinch_camera_distance(eye, dir, dist, |x, y, z| {
                    chunk_manager.block_at(x, y, z).is_solid()
                });
                v.position = eye + dir * pinched;
                v.pitch = (v.pitch - cam_tilt).clamp(-1.5, 1.5);
                v
            }
            2 => {
                // r515: cinematic over-the-shoulder — pulled back, raised, offset right.
                let mut v = local_player.view;
                let eye = local_player.view.position;
                let f = local_player.view.forward();
                let r = local_player.view.right();
                let target = eye - f * (2.6 * cam_zoom * cam_hz)
                    + r * (0.9 * cam_hz.sqrt())
                    + glm::vec3(0.0, 0.45 * cam_hz + cam_lift_extra, 0.0);
                let offset = target - eye;
                let dist = glm::length(&offset);
                let dir = if dist > 1e-4 { offset / dist } else { -f };
                let pinched = pinch_camera_distance(eye, dir, dist, |x, y, z| {
                    chunk_manager.block_at(x, y, z).is_solid()
                });
                v.position = eye + dir * pinched;
                v.pitch = (v.pitch - cam_tilt).clamp(-1.5, 1.5);
                v
            }
            _ => local_player.view,
        };
        // r540: CAMERA JUICE (render-only). First-person walk bob + chase-camera
        // smoothing. The logical eye (local_player.view) is untouched, so physics,
        // raycast, culling and audio are unaffected.
        {
            let speed01 = (last_player_speed / 5.6).clamp(0.0, 1.0);
            let grounded = !fg_app.swimming();
            let target_amp = if view_bob_on && grounded { speed01 } else { 0.0 };
            bob_amp_s += (target_amp - bob_amp_s) * (frame_dt_s * 6.0).min(1.0);
            bob_phase += frame_dt_s * (6.0 + speed01 * 6.0);
            if bob_phase > std::f32::consts::TAU {
                bob_phase -= std::f32::consts::TAU;
            }
            // r566: GROUND-SHAKE per footfall — a heavy (tall) body's steps jolt the
            // camera. The vertical bob is 2× stride, so a foot plants each time bob_phase
            // crosses π or wraps past 0. Scaled by the stature's ground_shake (= 0 for a
            // default character, so this never fires for them) and walking speed.
            {
                let pi = std::f32::consts::PI;
                let footfall = (prev_bob_phase < pi && bob_phase >= pi) || (bob_phase < prev_bob_phase);
                let walking = footfall && grounded && speed01 > 0.15 && body.is_some();
                if walking && cam_shake_on && player_stature.ground_shake > 0.05 {
                    let step_shake = (player_stature.ground_shake * 0.06).min(0.5) * speed01;
                    shake_mag = shake_mag.max(step_shake);
                }
                // r571: STOMP EROSION — each footfall of a heavy body crushes the blocks in
                // the foot's PATH. The stomp's crush capacity (≈ height/8, i.e. it grows with
                // the body's mass / ground_shake) is weighed against each block's hardness —
                // its density×fill resistance: blocks it can overpower are pulverised to air,
                // ones it can't are merely weathered a tier, and bedrock stops the foot. It
                // drives DOWN through a depth that grows with the crush capacity, so a titan
                // gouges a trench as it walks while a ~4-block ogre just packs the dirt.
                if walking && player_stature.ground_shake > 1.0 {
                    if let Some(b) = body.as_ref() {
                        let crush_cap = player_stature.height / 8.0; // max block hardness pulverised
                        // r573: curbed for mid titans (operator: too strong at ~15–35 blocks).
                        // Smaller footprint and a much shallower sink — the foot scuffs the
                        // surface rather than excavating a pit every step.
                        let er = ((player_stature.height / 12.0) as i32).clamp(1, 3); // foot footprint
                        let depth = (1 + (crush_cap * 0.4) as i32).clamp(1, 3); // how deep the foot drives
                        let fx = b.position[0].floor() as i32;
                        let fz = b.position[2].floor() as i32;
                        let fy = (b.aabb_min()[1] - 0.05).floor() as i32;
                        let mut crushed = 0u32;
                        for dz in -er..=er {
                            for dx in -er..=er {
                                if dx * dx + dz * dz > er * er {
                                    continue;
                                }
                                for dd in 0..depth {
                                    let (bx, by, bz) = (fx + dx, fy - dd, fz + dz);
                                    let blk = chunk_manager.block_at(bx, by, bz);
                                    if !blk.is_solid() {
                                        continue; // punch through air/water to reach buried solids
                                    }
                                    let h = blk.hardness();
                                    if h < 0.0 {
                                        break; // bedrock — the foot can't drive past it
                                    }
                                    if h <= crush_cap {
                                        if chunk_manager.set_block(bx, by, bz, BlockId::Air) {
                                            crushed += 1;
                                        }
                                    } else {
                                        // too dense to shatter — weather the surface tier and stop sinking
                                        let worn = match blk {
                                            BlockId::Grass => Some(BlockId::Dirt),
                                            BlockId::Stone => Some(BlockId::Cobblestone),
                                            BlockId::SnowBlock => Some(BlockId::Snow),
                                            _ => None,
                                        };
                                        if let Some(w) = worn {
                                            chunk_manager.set_block(bx, by, bz, w);
                                        }
                                        break;
                                    }
                                }
                            }
                        }
                        if crushed > 0 {
                            water_dirty = true;
                        }
                    }
                }
                prev_bob_phase = bob_phase;
            }
            if camera_mode == 0 && view_bob_on {
                // vertical at 2× stride frequency, lateral at 1× — classic, subtle.
                let up = (bob_phase * 2.0).sin() * 0.035 * bob_amp_s;
                let lat = bob_phase.sin() * 0.045 * bob_amp_s;
                let right = local_player.view.right();
                render_view.position =
                    render_view.position + glm::vec3(0.0, up, 0.0) + right * lat;
            }
            if cam_smooth_on && camera_mode != 0 {
                // ease the chase camera toward its target so pinch/zoom glide; snap
                // on big jumps (teleport/respawn/mode switch) to avoid a long slide.
                let target = render_view.position;
                let prev = cam_smoothed.unwrap_or(target);
                let lerp = if glm::length(&(target - prev)) > 6.0 {
                    1.0
                } else {
                    (frame_dt_s * 12.0).min(1.0)
                };
                let smoothed = prev + (target - prev) * lerp;
                cam_smoothed = Some(smoothed);
                render_view.position = smoothed;
            } else {
                cam_smoothed = None; // re-entering a chase view snaps the first frame
            }
            // r542: apply + decay the impulse shake (after bob/smoothing so it jolts
            // the final camera). Multi-frequency sine on the camera's right/up axes.
            if shake_mag > 0.001 {
                shake_t += frame_dt_s;
                let mag = shake_mag * 0.18; // world-unit scale of a full-strength jolt
                let sx = (shake_t * 37.0).sin() * 0.6 + (shake_t * 19.0).sin() * 0.4;
                let sy = (shake_t * 43.0).sin() * 0.6 + (shake_t * 23.0).sin() * 0.4;
                let right = local_player.view.right();
                render_view.position = render_view.position
                    + right * (sx * mag)
                    + glm::vec3(0.0, sy * mag, 0.0);
                // exponential decay (~0.25s to fade) — settles quickly.
                shake_mag *= (-frame_dt_s * 9.0).exp();
                if shake_mag < 0.001 {
                    shake_mag = 0.0;
                }
            }
        }
        let draw_result = renderer.draw_frame_with_view_and_atmos(
            &ctx, &render_view, &atmos,
        );
        _render_probe.set(unendlich_engine::thread_probe::Activity::Working);
        let draw_ms = t_draw_start.elapsed().as_secs_f64() * 1e3;
        t_draw_us += (draw_ms * 1000.0) as u64;
        if profile_on { prof.record("draw", draw_ms); }
        // r246: everything in the loop from frame top up to (not incl.) the
        // draw call — input/events, the roamer/dragon/fauna/drops/entity AI
        // updates, egui prepare, etc. The r245 profile showed frame_total p95
        // ~60ms while draw/tick_sim/manager/apply/upload were all near zero, so
        // the stutter is in THIS untimed remainder. This zone localizes it; if
        // game_update p95 is the ~55ms, the next step is to sub-time the AI
        // sections (roamers/dragon) inside it.
        if profile_on {
            let pre_draw_ms = t_draw_start.saturating_duration_since(t_frame_start).as_secs_f64() * 1e3;
            prof.record("game_update", pre_draw_ms);
        }
        match draw_result {
            Ok(true) => {
                if frame_count == 0 {
                    unendlich_engine::breadcrumb::drop_crumb("gpu: first frame presented OK");
                } else if frame_count % 600 == 0 {
                    // slow heartbeat (~every 10s @60fps) — a hang/power-off leaves this as the last line
                    unendlich_engine::breadcrumb::drop_crumb(&format!("alive: frame {frame_count}"));
                }
                frame_count += 1;
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

        // ── r412/r413/r415/r416: dynamic water sim (experimental, gated) ──
        // World-column water: each column's water rests on the real floor (so it falls),
        // spreads to lower-surface neighbours (downhill + fills holes); write-back flips
        // Air<->Water only (terrain never overwritten). Optimisations:
        //   opt-1  run only when dirty or still settling — idle settled water is free.
        //   opt-2  prefetch each region chunk ONCE (no per-cell DashMap lookup).
        //   opt-3  gather the columns in parallel via task_lanes (threshold-gated).
        // r447: WORLD-LOAD WARM-UP. For a short window after start, run the sim around spawn every
        // tick regardless of the player's exact position, so an ocean sitting next to a cave opening
        // begins DRAINING right away instead of waiting for the player to wade into it. The gather
        // covers the whole sim window, so anything within range of spawn settles/drains on load.
        if water_sim_enabled && start.elapsed().as_secs() < water_warmup_secs {
            water_dirty = true;
        }
        // r438: wake the sim for NATURAL (worldgen) water. The idle-skip previously only woke on
        // EDITS (place/break/rain/ice), so lakes and oceans you simply swim into were never
        // simulated — no depth, no flow, no settle, and the surface shader had no data (it read as
        // a flat overlay). If there's water near the player that we haven't simulated here yet
        // (no depth tile → -1), wake it. Once simulated, depth@player ≥ 0 and this stops firing.
        if water_sim_enabled && !water_dirty {
            let pp = local_player.view.position;
            if water_flow_tiles.sample_depth(pp.x as i32, pp.z as i32) < 0.0 {
                let (pxw, pyw, pzw) = (pp.x as i32, pp.y as i32, pp.z as i32);
                let mut near = false;
                'probe: for dy in -2..=1 {
                    for dz in -3..=3 {
                        for dx in -3..=3 {
                            if chunk_manager.block_at(pxw + dx, pyw + dy, pzw + dz)
                                == BlockId::Water
                            {
                                near = true;
                                break 'probe;
                            }
                        }
                    }
                }
                if near {
                    water_dirty = true;
                }
            }
        }
        // r424: rain wakes the sim so puddles form even without player edits.
        if water_rain && precip01 > 0.05 {
            water_dirty = true;
        }
        // r425: evaluate freeze/melt (hysteresis) and wake the sim on a transition.
        let mut freezing = false;
        if water_ice {
            let p = local_player.view.position;
            let t = {
                let m = chunk_manager.model_arc();
                m.temperature(p.x as f64, p.y as f64, p.z as f64, 0.0) as f32
            };
            let new_frozen = if water_ice_frozen {
                t < water_ice_melt
            } else {
                t < water_ice_freeze
            };
            if new_frozen != water_ice_frozen {
                water_ice_frozen = new_frozen;
                water_dirty = true; // apply the freeze/melt on the next tick
            }
            freezing = water_ice_frozen;
        }
        if water_sim_enabled && (water_dirty || water_active) {
            let now_ms = start.elapsed().as_millis() as u64;
            if now_ms.saturating_sub(water_sim_last_ms) >= water_sim_interval_ms {
                water_sim_last_ms = now_ms;
                use std::collections::HashMap;
                use std::sync::Arc;
                use unendlich_engine::world::chunk::Chunk;
                let cs = unendlich_engine::constants::CHUNK_SIZE;
                let p = local_player.view.position;
                let (px, py, pz) = (p.x as i32, p.y as i32, p.z as i32);
                let rh = (water_sim_radius * 8).max(8);
                let win_lo = py - water_drain_depth; // r426: deeper window → water finds cave floors farther down
                let win_hi = py + 24; // exclusive upper bound
                let side = (2 * rh + 1) as usize;
                let n = side * side;

                // opt-2: prefetch every chunk overlapping the region exactly once.
                let cx0 = (px - rh).div_euclid(cs);
                let cx1 = (px + rh).div_euclid(cs);
                let cy0 = win_lo.div_euclid(cs);
                let cy1 = (win_hi - 1).div_euclid(cs);
                let cz0 = (pz - rh).div_euclid(cs);
                let cz1 = (pz + rh).div_euclid(cs);
                let mut chunks: HashMap<ChunkCoord, Arc<Chunk>> = HashMap::new();
                for cyc in cy0..=cy1 {
                    for czc in cz0..=cz1 {
                        for cxc in cx0..=cx1 {
                            let coord = ChunkCoord::new(cxc, cyc, czc);
                            if let Some(c) = chunk_manager.get_chunk(coord) {
                                chunks.insert(coord, c);
                            }
                        }
                    }
                }
                // Read from the prefetched chunks (plain HashMap, no lock, no DashMap walk).
                let read = |wx: i32, wy: i32, wz: i32| -> BlockId {
                    let coord = ChunkCoord::new(
                        wx.div_euclid(cs),
                        wy.div_euclid(cs),
                        wz.div_euclid(cs),
                    );
                    match chunks.get(&coord) {
                        Some(c) => c.get(wx.rem_euclid(cs), wy.rem_euclid(cs), wz.rem_euclid(cs)),
                        None => BlockId::Air,
                    }
                };

                // opt-3: gather each column's (floor, water-count) — parallel above 2048 cols.
                let cols: Vec<usize> = (0..n).collect();
                let gathered: Vec<(i32, f32, bool)> =
                    unendlich_engine::util::task_lanes::map_pure_threshold(&cols, 2048, |&i| {
                        let cj = (i % side) as i32;
                        let ci = (i / side) as i32;
                        let wx = px - rh + cj;
                        let wz = pz - rh + ci;
                        // r445: the floor is the solid the water RESTS ON — the topmost solid
                        // BELOW the topmost water cell — NOT the topmost solid overall. The old
                        // code took the highest solid, so a column with a solid ABOVE the water (an
                        // overhang, a cave ceiling, a placed block, a terrain lip) set base above
                        // that solid and the write-back stacked the water on top of it → the
                        // "water rises occasionally" reports. Track the last solid seen scanning up
                        // and commit it as the floor each time water appears above it.
                        let mut floor_y = i32::MIN;
                        let mut last_solid = i32::MIN;
                        let mut last_solid_lava = false;
                        let mut wc = 0.0f32;
                        let mut lava = false;
                        for wy in win_lo..win_hi {
                            let b = read(wx, wy, wz);
                            if b == BlockId::Water || b == BlockId::Ice {
                                wc += 1.0;
                                floor_y = last_solid; // bed under the (highest-so-far) water
                                lava = last_solid_lava; // water boils if its BED is lava
                            } else if b.is_solid() || b == BlockId::Lava {
                                // r448: only SOLID blocks (and lava, which water rests on + boils)
                                // count as a floor. Non-solid decorations — grass, flowers, ferns,
                                // vines, mushrooms, … — fall through and are treated like air, so
                                // water flows into and floods them (waterlogging) instead of perching
                                // on top of them.
                                last_solid = wy;
                                last_solid_lava = b == BlockId::Lava;
                            }
                        }
                        (floor_y, wc, lava)
                    });
                let mut floor: Vec<i32> = Vec::with_capacity(n);
                let mut depth: Vec<f32> = Vec::with_capacity(n);
                let mut on_lava: Vec<bool> = Vec::with_capacity(n);
                for (t, d, l) in gathered {
                    floor.push(t);
                    depth.push(d);
                    on_lava.push(l);
                }
                // r424: rain adds shallow water to exposed columns before settling.
                let rain_added = if water_rain && precip01 > 0.05 {
                    unendlich_engine::world::water_sources::apply_rain(
                        &mut depth,
                        &floor,
                        precip01 * water_rain_rate,
                        water_rain_cap,
                    )
                } else {
                    0.0
                };
                // r428/r429: boil off water that's hot (near lava/fire/geothermal per the live
                // heat field) or sitting directly on lava, with a steam puff. r430: also build a
                // steam back-PRESSURE per column (raises its effective surface in the settle) so
                // water is repelled from hot deep sections — the counter-force that lets the sim
                // reach an equilibrium instead of draining oceans into caves indefinitely.
                let mut boil_removed = 0.0f32;
                let mut steam = vec![0.0f32; n];
                if water_boil {
                    let hsrc = heat_sources.snapshot(); // one lock, then pure per-column sums
                    for i in 0..n {
                        if depth[i] <= 0.0 || floor[i] == i32::MIN {
                            continue;
                        }
                        let cj = (i % side) as i32;
                        let ci = (i / side) as i32;
                        let wx = (px - rh + cj) as f32 + 0.5;
                        let wz = (pz - rh + ci) as f32 + 0.5;
                        let d0 = depth[i];
                        let wy = floor[i] as f32 + d0; // sample at the water surface
                        let heat_c: f32 =
                            hsrc.iter().map(|s| s.contribution_at(wx, wy, wz)).sum();
                        if on_lava[i] || heat_c >= water_boil_temp {
                            // hotter water boils faster (scaled a little past the threshold)
                            let over = ((heat_c - water_boil_temp) / 60.0).clamp(0.0, 2.0);
                            let rate = water_boil_rate * (1.0 + over);
                            depth[i] = (d0 - rate).max(0.0);
                            boil_removed += d0 - depth[i];
                            // steam pressure rises with how far past the boil point we are
                            steam[i] = (water_steam_pressure * (0.4 + over)).min(water_steam_pressure);
                            if let Some(ps) = particle_sys.as_mut() {
                                let puff = (4.0 + over * 6.0) as u32; // deeper/hotter vents harder
                                ps.emit(
                                    unendlich_engine::sim::particles::Preset::Smoke,
                                    [wx, wy, wz],
                                    puff,
                                );
                            }
                        }
                    }
                }
                let base: Vec<f32> = floor
                    .iter()
                    .map(|&t| {
                        let f = if t == i32::MIN { (win_lo - 1) as f32 } else { t as f32 };
                        f + 1.0
                    })
                    .collect();

                // Settle (gather-form: per-cell outflux then per-cell net; volume-conserving).
                // r417: also accumulate the per-column flow vectors for the surface shader.
                water_flow.reset(px - rh, pz - rh);
                let settle_iters = if freezing { 0 } else { water_settle_iters };
                // r442: cap total water a column may shed per TICK (summed across iters) so a dug
                // hole fills over a few ticks instead of in one — visible flow, still volume-safe.
                let mut sent_this_tick = vec![0.0f32; n];
                for _ in 0..settle_iters {
                    let mut send = vec![[0.0f32; 4]; n]; // +x, -x, +z, -z
                    for zz in 0..side {
                        for xx in 0..side {
                            let i = xx + zz * side;
                            let d = depth[i];
                            if d <= 0.0 {
                                continue;
                            }
                            // r680: a bottomless column (deep ocean, floor below the
                            // sim window) now acts as an INFINITE SOURCE: it may SEND
                            // downhill into a lower non-bottomless neighbour (so ocean
                            // water pours into an adjacent cave / cliff gap until that
                            // neighbour reaches sea level), but it never depletes (its
                            // depth is held constant in the write-back below) and never
                            // receives (neighbours skip `floor[j]==MIN` targets). Its
                            // gather surface `base+depth` already equals the true sea
                            // level, so it fills caves TO sea level and then stops
                            // (df→0) — no runaway rise (the r441 concern) and no ocean
                            // drain. Previously it was fully inert (`continue`), which is
                            // why cave/ocean borders never poured.
                            let is_source = floor[i] == i32::MIN;
                            let surf = base[i] + d + steam[i];
                            let mut want = [0.0f32; 4];
                            let mut wt = 0.0f32;
                            if xx + 1 < side {
                                let j = i + 1;
                                if floor[j] != i32::MIN {
                                    let df = surf - (base[j] + depth[j] + steam[j]);
                                    if df > 0.0 { want[0] = water_settle_f * df; wt += want[0]; }
                                }
                            }
                            if xx > 0 {
                                let j = i - 1;
                                if floor[j] != i32::MIN {
                                    let df = surf - (base[j] + depth[j] + steam[j]);
                                    if df > 0.0 { want[1] = water_settle_f * df; wt += want[1]; }
                                }
                            }
                            if zz + 1 < side {
                                let j = i + side;
                                if floor[j] != i32::MIN {
                                    let df = surf - (base[j] + depth[j] + steam[j]);
                                    if df > 0.0 { want[2] = water_settle_f * df; wt += want[2]; }
                                }
                            }
                            if zz > 0 {
                                let j = i - side;
                                if floor[j] != i32::MIN {
                                    let df = surf - (base[j] + depth[j] + steam[j]);
                                    if df > 0.0 { want[3] = water_settle_f * df; wt += want[3]; }
                                }
                            }
                            if wt <= 0.0 {
                                continue;
                            }
                            let scale = if wt > d && !is_source { d / wt } else { 1.0 };
                            // r442: also cap by remaining per-tick flow budget for this column.
                            let total = wt * scale;
                            let cap = (water_max_flow - sent_this_tick[i]).max(0.0);
                            let scale = if total > cap && total > 0.0 {
                                scale * (cap / total)
                            } else {
                                scale
                            };
                            sent_this_tick[i] += wt * scale;
                            send[i] = [want[0] * scale, want[1] * scale, want[2] * scale, want[3] * scale];
                        }
                    }
                    water_flow.accumulate(&send); // r417: fold this step's transfers into the flow field
                    for zz in 0..side {
                        for xx in 0..side {
                            let i = xx + zz * side;
                            let mut net = -(send[i][0] + send[i][1] + send[i][2] + send[i][3]);
                            if xx + 1 < side { net += send[i + 1][1]; }
                            if xx > 0 { net += send[i - 1][0]; }
                            if zz + 1 < side { net += send[i + side][3]; }
                            if zz > 0 { net += send[i - side][2]; }
                            // r680: a bottomless source holds its level (infinite ocean) —
                            // discard its own net so it never drains; real columns settle.
                            if floor[i] != i32::MIN {
                                depth[i] = (depth[i] + net).max(0.0).min((win_hi - win_lo) as f32);
                            }
                        }
                    }
                }

                // r423: dry isolated shallow puddles (guarded so lakes are never eroded).
                let evap_removed = if water_evap {
                    unendlich_engine::world::water_evaporate::evaporate(
                        &mut depth, side, 1.5, water_evap_rate,
                    )
                } else {
                    0.0
                };

                // Write back: water from floor+1 up to +depth (rounded); Air<->Water only.
                let mut budget = water_sim_budget;
                let mut changes: usize = 0;
                'wb: for ci in 0..side {
                    for cj in 0..side {
                        if budget == 0 {
                            break 'wb;
                        }
                        let i = cj + ci * side;
                        if floor[i] == i32::MIN {
                            continue;
                        }
                        let wx = px - rh + cj as i32;
                        let wz = pz - rh + ci as i32;
                        let b0 = floor[i] + 1;
                        let top = b0 + depth[i].round() as i32;
                        for wy in b0..win_hi {
                            if budget == 0 {
                                break;
                            }
                            let cur = read(wx, wy, wz);
                            let want = wy < top;
                            if want {
                                // r425: surface cell is Ice when frozen, else Water.
                                let desired = if freezing && wy == top - 1 {
                                    BlockId::Ice
                                } else {
                                    BlockId::Water
                                };
                                let replaceable = cur == BlockId::Air
                                    || cur == BlockId::Water
                                    || cur == BlockId::Ice
                                    // r448: flood non-solid decorations (grass, flowers, ferns,
                                    // vines, mushrooms, …) — waterlogging. Never lava (that's the
                                    // lava↔water reaction) or any solid.
                                    || (!cur.is_solid()
                                        && cur != BlockId::Lava
                                        && cur != BlockId::Air);
                                if cur != desired && replaceable {
                                    if chunk_manager.set_block(wx, wy, wz, desired) {
                                        budget -= 1;
                                        changes += 1;
                                    }
                                }
                            } else if cur == BlockId::Water || cur == BlockId::Ice {
                                if chunk_manager.set_block(wx, wy, wz, BlockId::Air) {
                                    budget -= 1;
                                    changes += 1;
                                }
                            }
                        }
                    }
                }
                // opt-1: keep simulating while water is still moving; otherwise go idle.
                // r433: shoreline erosion — waves wash exposed Grass/Dirt at the waterline into
                // Sand (beaches form over time). Conservative: same-volume swap, no holes; gated +
                // budgeted + slow. Deliberately the only place water touches terrain.
                let mut eroded = 0usize;
                if water_erosion {
                    let mut ebudget = water_erosion_budget;
                    // rolling start offset so we don't always scan the same cells first
                    let estart = (frame_count as usize).wrapping_mul(2654435761) % n.max(1);
                    for off in 0..n {
                        if ebudget == 0 { break; }
                        let i = (estart + off) % n;
                        if depth[i] <= 0.0 || floor[i] == i32::MIN { continue; }
                        let top_i = floor[i] + depth[i].round() as i32; // water surface level
                        let cj = (i % side) as i32;
                        let ci = (i / side) as i32;
                        // 4 neighbours; erode a land neighbour's surface block at the waterline
                        let nbrs = [(cj + 1, ci), (cj - 1, ci), (cj, ci + 1), (cj, ci - 1)];
                        for (nx, nz) in nbrs {
                            if nx < 0 || nz < 0 || nx >= side as i32 || nz >= side as i32 { continue; }
                            let j = (nx as usize) + (nz as usize) * side;
                            if floor[j] == i32::MIN { continue; }
                            // land that rises to about the waterline (±1)
                            if (floor[j] - top_i).abs() > 1 { continue; }
                            let wxj = px - rh + nx;
                            let wzj = pz - rh + nz;
                            let b = read(wxj, floor[j], wzj);
                            if b == BlockId::Grass || b == BlockId::Dirt {
                                if chunk_manager.set_block(wxj, floor[j], wzj, BlockId::Sand) {
                                    eroded += 1;
                                    ebudget -= 1;
                                    break;
                                }
                            }
                        }
                    }
                }
                let _ = eroded;

                // r444: LIVE FLOW EROSION + SEDIMENT. Fast current over a soft bed carves it out
                // (deepening the channel), lifting material into a suspended load that advects
                // downstream; where the current slows, the load drops back as bed. Budgeted.
                if water_flow_erosion {
                    let mut fbudget = flow_erosion_budget;
                    let fstart = (frame_count as usize).wrapping_mul(40503).wrapping_add(7) % n.max(1);
                    // pass 1: erode under fast flow
                    for off in 0..n {
                        if fbudget == 0 { break; }
                        let i = (fstart + off) % n;
                        if depth[i] <= 0.0 || floor[i] == i32::MIN { continue; }
                        let cx = i % side;
                        let cz = i / side;
                        let (fx, fz) = water_flow.at(cx, cz);
                        let fspeed = (fx * fx + fz * fz).sqrt();
                        if fspeed < flow_erosion_threshold { continue; }
                        let wx = px - rh + cx as i32;
                        let wz = pz - rh + cz as i32;
                        let by = floor[i]; // the bed cell (topmost solid under the water)
                        let bed = read(wx, by, wz);
                        let soft = matches!(
                            bed,
                            BlockId::Sand | BlockId::Gravel | BlockId::Dirt | BlockId::Grass
                        );
                        if !soft { continue; }
                        let rate = unendlich_engine::sim::current_force::erosion_rate(
                            fspeed,
                            flow_erosion_threshold,
                            unendlich_engine::sim::current_force::substrate_softness(true),
                            1.0,
                        );
                        if rate <= 0.0 { continue; }
                        // carve the bed away (it becomes water — channel deepens) and book the
                        // lifted material, then advect it one cell downstream along the current.
                        if chunk_manager.set_block(wx, by, wz, BlockId::Water) {
                            sediment.erode(wx, wz, 1.0);
                            let dx = if fx > 0.3 { 1 } else if fx < -0.3 { -1 } else { 0 };
                            let dz = if fz > 0.3 { 1 } else if fz < -0.3 { -1 } else { 0 };
                            if dx != 0 || dz != 0 {
                                sediment.advect((wx, wz), (wx + dx, wz + dz), 0.7);
                            }
                            changes += 1;
                            fbudget -= 1;
                        }
                    }
                    // pass 2: deposit suspended load where the current is too slow to hold it
                    let mut dbudget = flow_erosion_budget;
                    for off in 0..n {
                        if dbudget == 0 { break; }
                        let i = (fstart + off) % n;
                        if depth[i] <= 0.0 || floor[i] == i32::MIN { continue; }
                        let cx = i % side;
                        let cz = i / side;
                        let wx = px - rh + cx as i32;
                        let wz = pz - rh + cz as i32;
                        if sediment.suspended(wx, wz) < 1.0 { continue; }
                        let (fx, fz) = water_flow.at(cx, cz);
                        let fspeed = (fx * fx + fz * fz).sqrt();
                        // capacity the current can keep aloft scales with its speed
                        let capacity = fspeed * flow_carry_capacity;
                        let drop = sediment.deposit_excess(wx, wz, capacity);
                        if drop == 0 { continue; }
                        // raise the bed one cell (place Sand at the lowest water cell)
                        let place_y = floor[i] + 1;
                        if read(wx, place_y, wz) == BlockId::Water
                            && chunk_manager.set_block(wx, place_y, wz, BlockId::Sand)
                        {
                            changes += 1;
                            dbudget -= 1;
                        }
                    }
                }

                // r444: LAVA + WATER. Scan the floored columns; where a water cell sits face-
                // adjacent to a lava cell, solidify the lava (obsidian if it's a deep/source face,
                // else stone) and flash a steam puff. Budgeted via the erosion budget.
                if lava_water_react {
                    let mut lbudget = flow_erosion_budget * 2;
                    let lstart = (frame_count as usize).wrapping_mul(2246822519) % n.max(1);
                    for off in 0..n {
                        if lbudget == 0 { break; }
                        let i = (lstart + off) % n;
                        if depth[i] <= 0.0 || floor[i] == i32::MIN { continue; }
                        let cx = i % side;
                        let cz = i / side;
                        let wx = px - rh + cx as i32;
                        let wz = pz - rh + cz as i32;
                        // check a couple of water cells in this column against lava neighbours
                        let surf_y = floor[i] + depth[i].round() as i32;
                        let y_lo = (floor[i] + 1).max(surf_y - 3);
                        for wy in y_lo..=surf_y {
                            if read(wx, wy, wz) != BlockId::Water { continue; }
                            let faces = [
                                (wx + 1, wy, wz), (wx - 1, wy, wz),
                                (wx, wy, wz + 1), (wx, wy, wz - 1),
                                (wx, wy - 1, wz), (wx, wy + 1, wz),
                            ];
                            for (lx, ly, lz) in faces {
                                if read(lx, ly, lz) != BlockId::Lava { continue; }
                                // treat a lava cell with lava below it as a source (→ obsidian)
                                let is_source = read(lx, ly - 1, lz) == BlockId::Lava;
                                if let Some(res) = unendlich_engine::sim::water_react::lava_water_contact(
                                    true, true, is_source,
                                ) {
                                    let solid = if res.obsidian {
                                        BlockId::Obsidian
                                    } else {
                                        BlockId::Stone
                                    };
                                    if chunk_manager.set_block(lx, ly, lz, solid) {
                                        changes += 1;
                                        lbudget -= 1;
                                    }
                                    if res.water_consumed {
                                        let _ = chunk_manager.set_block(wx, wy, wz, BlockId::Air);
                                    }
                                    if res.steam {
                                        if let Some(ps) = particle_sys.as_mut() {
                                            ps.emit(
                                                unendlich_engine::sim::particles::Preset::Smoke,
                                                [wx as f32 + 0.5, wy as f32 + 1.0, wz as f32 + 0.5],
                                                6,
                                            );
                                        }
                                    }
                                    break;
                                }
                            }
                            if lbudget == 0 { break; }
                        }
                    }
                }
                water_active = changes > 0 || evap_removed > 0.0 || rain_added > 0.0 || boil_removed > 0.0;
                // r436: surface what the sim actually did this tick.
                dbg_water_changes = changes;
                dbg_water_cols = n;
                let (_ddx, _ddz, dsp) = water_flow.dominant(1.0);
                dbg_flow_speed = dsp;
                water_dirty = false;

                // r418 (phase 2): persist the window's flow into tiles, fade stale tiles,
                // and evict far/old ones. So a river you've travelled keeps its current
                // (gently fading where the sim no longer refreshes) instead of resetting.
                {
                    use unendlich_engine::world::water_flow_tiles::TILE_SIZE;
                    water_flow_tiles.ingest_blend(
                        water_flow.origin_x,
                        water_flow.origin_z,
                        side,
                        &water_flow.vx,
                        &water_flow.vz,
                        &depth, // r432: persist water depth alongside flow
                        now_ms,
                        water_flow_smooth,
                    );
                    water_flow_tiles.decay(now_ms, 0.97); // gentle fade for unrefreshed tiles
                    let ptx = px.div_euclid(TILE_SIZE);
                    let ptz = pz.div_euclid(TILE_SIZE);
                    water_flow_tiles.evict(now_ms, 60_000, ptx, ptz, 24);
                }

                // r417/r418 (phase 1/2): prove the flow field + persistence are working.
                if renderer.ui_debug_master && renderer.ui_debug_flow && now_ms.saturating_sub(water_flow_dbg_last_ms) >= 1000 {
                    water_flow_dbg_last_ms = now_ms;
                    let (dx, dz, sp) = water_flow.dominant(1.0);
                    let (tfx, tfz) = water_flow_tiles.sample(px, pz);
                    log::info!(
                        "[water-flow] window dir=({dx:.2},{dz:.2}) speed={sp:.3} | persisted@player=({tfx:.2},{tfz:.2}) tiles={} active={water_active} rain+={rain_added:.1} evap-={evap_removed:.1}",
                        water_flow_tiles.tile_count()
                    );
                }
            }
        }

        // ── fullgame: drive the title menu + new-game wizard through engine::app ──
        // (the wiring fix). The renderer panel still draws + emits menu_choice /
        // wizard_action; here we translate those into ONE AppInput, let `App` decide,
        // and reflect the resulting menu state back into the display-only panel.
        {
            use unendlich_engine::app::{AppInput, AppStep, LaunchPlan};
            use unendlich_engine::sim::command_parse::GameMode;
            use unendlich_engine::sim::game_flow::AppState;
            use unendlich_engine::sim::new_world_config::SeedChoice;
            use unendlich_engine::sim::title_flow::{MenuId, Screen};

            // r401: Host World opened the wizard; once it's actually open, fire the
            // proven one-click Create (wizard_action 9) so a single Host World click
            // launches a world without manual wizard steps.
            // r186: Create World spree — a single click walks the wizard to
            // launch via the NORMAL path (so Launch handling runs).
            if fg_create_spree > 0 {
                if !renderer.wizard_open {
                    fg_create_spree = 0;
                } else if renderer.menu_choice == 0 && renderer.wizard_action == 0 {
                    renderer.wizard_action = if renderer.wizard_at_confirm { 3 } else { 2 };
                    fg_create_spree -= 1;
                }
            }
            // r663: Bending Tutorial — once its wizard is open, pin the tutorial
            // seed/name and one-click launch through the FSM (drives MainMenu →
            // Loading → InGame properly), then arm the arena stamp at spawn.
            // r679: replaced by AppInput::StartTutorial (direct launch, no wizard).
            let input = if renderer.menu_world_pick >= 0 {
                // r385 LOAD FIX: a clicked saved-world row goes through the App FSM
                // (SelectSlot → resolve_load → Launch(StartLocal) AND the state machine
                // leaves the menu → InGame). The old bin bypass (fg_pending_load below)
                // rebuilt the terrain but never drove the App state, so it bounced back
                // to the menu with a fresh backdrop seed instead of entering the world.
                let p = renderer.menu_world_pick as u8;
                renderer.menu_world_pick = -1; // consume here so the bypass below can't double-fire
                AppInput::SelectSlot(p)
            } else if renderer.menu_choice != 0 {
                let c = renderer.menu_choice;
                renderer.menu_choice = 0;
                // r385 MP FIX: menu_choice is screen-dependent. On the Multiplayer
                // screen, 2 = "Host World" and 3 = "Join Server"; everywhere else
                // 2/3 mean "open the Multiplayer screen". The old code mapped 2|3 to
                // Select(Multiplayer) regardless, so on the MP screen those buttons hit
                // title_flow's Title-only guard and became silent no-ops.
                let on_mp = fg_app.menu().screen() == Screen::Multiplayer;
                match c {
                    1 => AppInput::Select(MenuId::NewGame),
                    // r401: Host World → open the new-game wizard and (next frame, once
                    // it's open) trigger the proven one-click Create path so it actually
                    // LAUNCHES a world via the real FSM (driving MainMenu→Loading→InGame,
                    // not the fg_pending_load bypass which bounces back to the menu). With
                    // LAN-MP on, the running world auto-announces for joiners.
                    2 if on_mp => {
                        // r661: Host World opens the create wizard so the player
                        // can choose a seed / name / mode before launching. (Was:
                        // set fg_host_pending → auto one-click-launch, which skipped
                        // seed creation entirely.) The running world auto-announces
                        // for LAN joiners (UNENDLICH_LAN_MP defaults on), so hosting
                        // still works once the world starts.
                        AppInput::Select(MenuId::NewGame)
                    }
                    3 if on_mp => {
                        // r401: joining is done by clicking a discovered host in the
                        // "LAN games" list below; this button just points the user there.
                        log::info!(
                            "[lan-mp] Join: pick a LAN game from the list below (hosts appear automatically)"
                        );
                        AppInput::None
                    }
                    2 | 3 => AppInput::Select(MenuId::Multiplayer), // from Title: open MP screen
                    4 => AppInput::Select(MenuId::Quit),
                    5 => AppInput::Confirm, // r186: ConfirmQuit -> Quit
                    6 => AppInput::Back,    // r186: screen back
                    7 => AppInput::Select(MenuId::Load), // r197: -> LoadSlots
                    8 => AppInput::Select(MenuId::Singleplayer), // r656: open SP submenu
                    9 => {
                        // r679: Bending Tutorial → launch DIRECTLY via the new
                        // AppInput::StartTutorial (builds a fixed tutorial spec +
                        // drives MainMenu → Loading → InGame) WITHOUT ever opening
                        // the create wizard, so the seed page never flashes. (r663
                        // walked the visible wizard, which flashed the seed page;
                        // r661's fg_pending_load bounced to the menu.) The arena is
                        // stamped once chunks load.
                        fg_tutorial_arena_pending = true;
                        fg_tutorial_launch_frame = frame_count;
                        AppInput::StartTutorial(TUTORIAL_SEED)
                    }
                    _ => AppInput::None,
                }
            } else if renderer.wizard_action != 0 {
                let a = renderer.wizard_action;
                renderer.wizard_action = 0;
                match a {
                    1 => AppInput::WizardBack,
                    2 => AppInput::WizardNext,
                    3 => AppInput::Confirm, // Create
                    4 => AppInput::Back,    // Cancel → title
                    5 => {
                        let next = match fg_app.menu().wizard().map(|w| w.config().mode) {
                            Some(GameMode::Survival) => GameMode::Creative,
                            Some(GameMode::Creative) => GameMode::Adventure,
                            Some(GameMode::Adventure) => GameMode::Spectator,
                            _ => GameMode::Survival,
                        };
                        AppInput::WizardSetMode(next)
                    }
                    6 => {
                        // r179: more seed options — Random -> classic 1337
                        // -> clock entropy -> name hash -> Random.
                        let name_hash = fg_app
                            .menu()
                            .wizard()
                            .map(|w| {
                                w.config().name.bytes().fold(0u64, |a, b| {
                                    a.wrapping_mul(131).wrapping_add(b as u64)
                                })
                            })
                            .unwrap_or(7);
                        let nanos = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_nanos() as u64 % 1_000_000)
                            .unwrap_or(42);
                        let next = match fg_app.menu().wizard().map(|w| w.config().seed.clone()) {
                            Some(SeedChoice::Random) => SeedChoice::Fixed(1337),
                            Some(SeedChoice::Fixed(1337)) => SeedChoice::Fixed(nanos),
                            Some(SeedChoice::Fixed(s)) if s == name_hash => SeedChoice::Random,
                            Some(SeedChoice::Fixed(_)) => SeedChoice::Fixed(name_hash),
                            _ => SeedChoice::Random,
                        };
                        AppInput::WizardSetSeed(next)
                    }
                    7 => {
                        // r179: name text from the wizard's edit box.
                        AppInput::WizardSetName(renderer.wizard_name_buf.clone())
                    }
                    8 => {
                        // r181: typed seed — number, or any text hashed.
                        let s = renderer.wizard_seed_buf.trim().to_string();
                        let v = s.parse::<u64>().unwrap_or_else(|_| {
                            s.bytes().fold(0u64, |a, b| a.wrapping_mul(131).wrapping_add(b as u64))
                        });
                        AppInput::WizardSetSeed(SeedChoice::Fixed(v))
                    }
                    9 => {
                        // r185: one-click Create World — blank names get a
                        // default (the blank-name dead end), Random gets the
                        // seed the preview already showed (no surprise world).
                        let nm = fg_app.menu().wizard().map(|w| w.config().name.clone()).unwrap_or_default();
                        if nm.trim().is_empty() {
                            let _ = fg_app.tick(AppInput::WizardSetName("New World".into()), 0);
                        }
                        if let Some(pv) = fg_last_preview_seed {
                            let is_random = fg_app.menu().wizard()
                                .map(|w| matches!(w.config().seed, SeedChoice::Random))
                                .unwrap_or(false);
                            if is_random {
                                let _ = fg_app.tick(AppInput::WizardSetSeed(SeedChoice::Fixed(pv)), 0);
                            }
                        }
                        fg_create_spree = 6; // r186: keep confirming to launch
                        AppInput::Confirm
                    }
                    _ => AppInput::None,
                }
            } else {
                AppInput::None
            };

            let entropy = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0);
            let in_dbg = format!("{input:?}");
            let mut step = fg_app.tick(input, entropy);
            // r197: a Load-screen selection overrides the step with a
            // synthetic StartLocal, reusing the ENTIRE launch handler below
            // (seed rebuild, mesh-epoch clear, spawn, r195 survival-flight
            // reset). Bypasses the menu's internal catalog (avoids the r131
            // dual-catalog desync) — the file catalog is the source of truth.
            if let Some((name, seed, surv)) = fg_pending_load.take() {
                use unendlich_engine::sim::command_parse::GameMode as GM;
                let init = unendlich_engine::sim::world_launch::WorldInit {
                    seed,
                    mode: if surv { GM::Survival } else { GM::Creative },
                    name,
                    multiplayer: false,
                };
                step = AppStep::Launch(LaunchPlan::StartLocal(init));
            }

            if in_dbg != "None" {
                log::info!("fullgame/app: {in_dbg} -> {step:?}"); // confirms App drives the menu
            }
            match step {
                AppStep::Menu => {}
                AppStep::Launch(plan) => {
                    // F4 wiring: a local launch rebuilds the world generator from the
                    // chosen seed and resets chunk streaming, then drops the player on
                    // the new surface. (The startup TerrainGenerator stays as the
                    // menu-backdrop world; this swaps it for the picked seed on Create.)
                    if let LaunchPlan::StartLocal(ref init) = plan {
                        // r125: MENU MODE PROPAGATION — the wizard's Creative/Survival
                        // choice finally reaches the ruleset (operator: "still creative,
                        // menu propagation thing"). Flight permission updates live.
                        let was = survival_mode;
                        // init.mode is the canonical command_parse::GameMode
                        // (4 variants; world_launch maps the catalog's 2 into it).
                        // Survival + Adventure both get the survival ruleset;
                        // Creative + Spectator stay creative.
                        survival_mode = matches!(
                            init.mode,
                            unendlich_engine::sim::command_parse::GameMode::Survival
                                | unendlich_engine::sim::command_parse::GameMode::Adventure
                        );
                        local_player.permissions.can_toggle_flight = !survival_mode;
                        // r195: launching SURVIVAL must also END active flight,
                        // not merely forbid toggling it. Previously you could
                        // spawn into survival still flying (carried from the
                        // title world / a prior creative session) and, because
                        // toggling was now denied, be unable to land (operator:
                        // "start in survival with fly enabled and cannot turn
                        // it off"). Mirror the F6 path: attach a physics body so
                        // gravity applies immediately. Creative leaves body as-is
                        // (None = free flight), matching its ruleset.
                        if survival_mode && body.is_none() {
                            body = Some(unendlich_engine::physics::PhysicsBody::new_player([
                                local_player.view.position.x,
                                local_player.view.position.y,
                                local_player.view.position.z,
                            ]));
                            self_avatar_dirty = true; // r561: size collider to stature
                            log::info!("[mode] survival launch -> flight ended, gravity on");
                        }
                        if was != survival_mode {
                            log::info!("[mode] menu launch set {} mode",
                                if survival_mode { "SURVIVAL" } else { "CREATIVE" });
                        }
                        let seed = init.seed;
                        // r163: PER-WORLD PERSISTENCE — save the outgoing
                        // world's edited chunks, then re-root storage under
                        // worlds/<name>/ so each world keeps its OWN edits
                        // (previously every world shared worlds/default and
                        // bled edits into each other at the same coords).
                        {
                            let saved = chunk_manager.save_all();
                            if saved > 0 {
                                log::info!("[saves] {} edited chunk(s) saved before world switch", saved);
                            }
                            let safe: String = init
                                .name
                                .chars()
                                .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
                                .collect();
                            // r291: identity is name+seed, not name alone — a world's
                            // title is just a label and may collide, but the seed
                            // disambiguates the storage dir so two same-titled worlds
                            // keep their OWN edits (a lightweight stand-in for a full
                            // world-id until that refactor lands).
                            let stem = if safe.is_empty() { "world".to_string() } else { safe };
                            let new_world_dir = std::path::PathBuf::from(format!(
                                "worlds/{}-{:016x}",
                                stem, seed
                            ));
                            chunk_manager.set_world_root(new_world_dir.clone());
                            // r580 FIX (world merging): the flora/crop store and whitelist
                            // were bound to the STARTUP root (worlds/default) and never
                            // rebound, so a new world inherited the previous world's crops &
                            // saplings at stale positions — plants "floating over air" where
                            // the old terrain used to be. Persist the old store, then rebind
                            // BOTH roots to this world and load THIS world's flora (empty on
                            // first visit ⇒ a clean, freshly-generated world).
                            let _ = session.flora.save(&flora_root);
                            let _ = whitelist.save(&flora_root);
                            flora_root = new_world_dir;
                            session.flora =
                                unendlich_engine::world::flora_store::FloraStore::load(&flora_root);
                            whitelist =
                                unendlich_engine::sim::whitelist::Whitelist::load(&flora_root);
                        }
                        match chunk_manager.rebuild_for_seed(seed, Backend::Auto, gen_batch) {
                            Ok(()) => {
                                // r346 SEED FIX: adopt the launched seed as the single
                                // live world seed and REBUILD the field overlays for it.
                                // rebuild_for_seed just swapped the terrain generator, but
                                // the moisture/biome/wind overlays sample their OWN
                                // WorldModel; without rebuilding it the overlays stay on
                                // the old seed while terrain uses the new one (two seeds in
                                // one world). Creature spawns + colony also key off
                                // fg_world_seed now, so the whole world is one seed.
                                fg_world_seed = seed;
                                // r350 BISECT: r346's on-load set_fields swap DISABLED to
                                // test whether it is the source of the giant-geometry regression.
                                // (rebuild_for_seed above still swaps the terrain generator.)
                                // chunk_manager.set_fields(...);  // <-- reverted for bisect
                                // r346 SEED FIX: weather was a fixed env/default seed
                                // shared by every world; derive it (salted) from the live
                                // world seed so each world has its own coherent weather.
                                weather_state = unendlich_engine::weather::Weather::new(
                                    fg_world_seed ^ 0x5EED_0000_0000_0000,
                                );
                                // r126: record/refresh this world in the catalog and
                                // persist, so it shows up in Load after a restart.
                                {
                                    // r131: record FILE-ONLY — mutating the live
                                    // catalog post-construction desyncs the menu's
                                    // GameFlow (the r126 menu breakage). The file is
                                    // re-read into a fresh catalog at next boot.
                                    use unendlich_engine::sim::world_catalog::{GameMode as CatMode, WorldEntry};
                                    let tick = world_clock_elapsed as u64;
                                    let mut entries = load_catalog_entries();
                                    // r577: identity is NAME (was name+seed, r291) —
                                    // re-creating a world under an existing name REPLACES
                                    // its row rather than leaving a duplicate behind. This
                                    // matches WorldCatalog::add and the deduped loader, so
                                    // the menu never shows two worlds of the same name.
                                    entries.retain(|e| e.name != init.name);
                                    entries.push(WorldEntry {
                                        name: init.name.clone(),
                                        seed,
                                        mode: if survival_mode { CatMode::Survival } else { CatMode::Creative },
                                        created_tick: tick,
                                        last_played_tick: tick,
                                    });
                                    save_catalog(&entries);
                                    log::info!("[saves] recorded world '{}' (seed {seed})", init.name);
                            current_world_name = Some(init.name.clone()); // r165
                                }
                                // Drop the old (titlescreen) world's uploaded meshes so
                                // the new seed's world doesn't render merged on top of it.
                                renderer.clear_all_chunk_meshes();
                                let (sx, sy, sz) =
                                    chunk_manager.pick_spawn(0, 0, probe_y_min, probe_y_max);
                                let new_spawn =
                                    glm::vec3(sx as f32 + 0.5, (sy + 4) as f32, sz as f32 + 0.5);
                                local_player.view.position = new_spawn;
                                // Re-arm the one-shot so the monument + flora are
                                // (re)placed in this freshly launched world too.
                                landmarks_placed = false;
                                // r162: WORLD-SWITCH RESET — the old world's
                                // life does not follow you into the new one.
                                roamers.clear();
                                fg_app.drops_mut().clear();
                                fg_colony = unendlich_engine::sim::popdyn::Colony::seeded(24, fg_world_seed ^ 0xFA1A);
                                fg_thermal = unendlich_engine::sim::survival_exposure::Thermal {
                                    body_c: 36.5,
                                    comfort: 0.0,
                                };
                                fg_imbue = unendlich_engine::sim::artifact_imbue::ImbueState {
                                    counter: 0.0,
                                    element: 2,
                                    tier: 2,
                                };
                                bend_resonance =
                                    unendlich_engine::sim::bending_charge::Resonance::full();
                                fg_app.stamina_mut().level = 1.0;
                                prev_fall_vy = 0.0;
                                precip01 = 0.0;
                                world_clock_elapsed = 0.0; // a new world starts at 08:00
                                // r453: MULTIWORLD LEAK SWEEP — persistent SIM state added since the
                                // r162 reset is keyed to the OLD world's coordinates and must NOT bleed
                                // into the new world (it would be two worlds' data coexisting): the
                                // sapling registry, the water flow field, the suspended sediment load,
                                // and the persistent flow/depth tiles. Clear/re-seed them all here.
                                water_dirty = water_sim_enabled;
                                water_flow.reset(0, 0);
                                sediment =
                                    unendlich_engine::world::sediment_map::SedimentMap::new();
                                water_flow_tiles =
                                    unendlich_engine::world::water_flow_tiles::FlowTileStore::new();
                                // r453: more coordinate-keyed state that must not carry over —
                                // session.lava_registered (a `contains` skip-guard: stale coords would stop
                                // the new world's lava at those chunks from ever registering), the
                                // chunk eviction queue (stale coords could evict freshly-loaded new
                                // chunks), and any not-yet-sent LAN edits from the old world.
                                // r461: named-location discoveries are per-world.
                                session.reset(fg_world_seed);
                                // r453: keep the WorldList in sync with the ACTUAL live world. Before
                                // this, only the startup spawn world was tracked; menu switches bypassed
                                // the gate, so loaded() stayed pinned to a world no longer loaded and the
                                // NoConcurrent guard was stale. Unload the outgoing world and load the
                                // incoming one through the same gate (advisory — the real load already
                                // happened; this makes the bookkeeping/audit-log truthful).
                                {
                                    use unendlich_engine::world::world_list::WorldEntry;
                                    let new_wid: u64 = seed;
                                    world_list.unload();
                                    if !world_list.is_known(new_wid) {
                                        world_list.register(WorldEntry {
                                            id: new_wid,
                                            name: init.name.clone(),
                                            seed,
                                        });
                                    }
                                    match world_list.try_load(new_wid) {
                                        Ok(id) => log::info!(
                                            "[worldlist] switched to world id={id}; exactly one world loaded"
                                        ),
                                        Err(e) => log::warn!(
                                            "[worldlist] switch load denied (advisory): {e:?}"
                                        ),
                                    }
                                }
                                log::info!("[launch] gameplay state reset for the new world");
                                renderer.loading_fade = 1.0; // r178: open on black
                                renderer.loading_label = format!("entering {}", init.name);
                                // r176: UI state too — no window or latch
                                // survives into the new world.
                                renderer.pause_menu_open = false;
                                renderer.settings_menu_open = false;
                                renderer.craft_menu_open = false;
                                renderer.settings_open = false;
                                renderer.pause_action = 0;
                                renderer.craft_pick = 0;
                                log::info!(
                                    "fullgame: launch seed {seed} -> rebuilt world, surface spawn {new_spawn:?}"
                                );
                            }
                            Err(e) => log::error!(
                                "fullgame: rebuild_for_seed({seed}) failed: {e:?} — staying in spawn world"
                            ),
                        }
                    }
                    fg_app.enter_game();
                    renderer.ui_pointer_active = false;
                    window.set_cursor_mode(CursorMode::Disabled);
                    log::info!("fullgame: launch {plan:?} — entering game");
                }
                AppStep::Quit => {
                    save_settings(&settings_path, &renderer); // r187 sticky
                    window.set_should_close(true);
                }
            }

            // Reflect app menu state into the display-only panel. Once the phase
            // leaves MainMenu (Loading/InGame) the menu + wizard are hidden.
            let in_menu = fg_app.state() == AppState::MainMenu;
            let screen = fg_app.menu().screen();

            // r394: LAN multiplayer discovery tick (gated; throttled ~5/sec).
            if let Some(beacon) = &lan_beacon {
                let now_ms = lan_start.elapsed().as_millis() as u64;

                // r397/r399: session-socket I/O, run EVERY frame. Datagrams are
                // channel-muxed by a leading byte so the handshake and entity
                // replication share one socket.
                if let Some(sock) = &lan_session_sock {
                    use unendlich_engine::sim::fixed::Fixed;
                    use unendlich_engine::sim::lan_session_wire::LanJoin;
                    use unendlich_engine::sim::net::entity_replication::{receive, seal};
                    use unendlich_engine::sim::net::entity_snapshot::{EntityKind, EntityState};
                    const CH_HANDSHAKE: u8 = 1;
                    const CH_ENTITY: u8 = 2;
                    const CH_WORLDCMD: u8 = 3;
                    const CH_AVATAR: u8 = 4; // r494: peer character appearance
                    use unendlich_engine::sim::net::world_cmd_wire::{decode_batch, encode_batch};

                    // Inbound: drain pending datagrams (non-blocking), route by channel.
                    let mut buf = [0u8; 1500];
                    loop {
                        match sock.recv_from(&mut buf) {
                            Ok((n, src)) if n >= 1 => {
                                let payload = &buf[1..n];
                                match buf[0] {
                                    CH_HANDSHAKE => match LanJoin::parse(payload) {
                                        Some(LanJoin::Join { name }) => {
                                            // r518: enforce the access policy. Open hosts admit
                                            // anyone; closed hosts admit only whitelisted names.
                                            if !whitelist.admits(&name) {
                                                log::info!("[lan-mp] join DENIED for '{name}' ({src}) — not on whitelist");
                                                let deny = LanJoin::Deny {
                                                    reason: "not on this server's whitelist".into(),
                                                }
                                                .encode();
                                                let mut dg = vec![CH_HANDSHAKE];
                                                dg.extend_from_slice(&deny);
                                                let _ = sock.send_to(&dg, src);
                                                continue;
                                            }
                                            log::info!("[lan-mp] join request from '{name}' ({src}) — admitted");
                                            whitelist.allow(&name); // remember joiners so the host can later close + keep them
                                            if let std::net::SocketAddr::V4(a) = src {
                                                lan_peer_addr = Some(a); // host learns where to replicate
                                            }
                                            let reply = LanJoin::Accept {
                                                world_name: current_world_name
                                                    .clone()
                                                    .unwrap_or_else(|| "World".into()),
                                                world_seed: fg_world_seed,
                                            }
                                            .encode();
                                            let mut dg = vec![CH_HANDSHAKE];
                                            dg.extend_from_slice(&reply);
                                            let _ = sock.send_to(&dg, src);
                                        }
                                        Some(LanJoin::Accept { world_name, world_seed }) => {
                                            log::info!(
                                                "[lan-mp] accepted by host: joining world '{world_name}' seed {world_seed}"
                                            );
                                            // Adopt the host's seed and load via the existing
                                            // synthetic-StartLocal launch path (rebuild_for_seed →
                                            // MainMenu→Loading→InGame). Same seed ⇒ same terrain.
                                            fg_pending_load = Some((world_name, world_seed, false));
                                        }
                                        Some(LanJoin::Deny { reason }) => {
                                            log::info!("[lan-mp] join denied: {reason}");
                                        }
                                        None => {}
                                    },
                                    CH_ENTITY => {
                                        if let Some(states) = receive(&mut lan_gate, payload) {
                                            for s in &states {
                                                let pos =
                                                    [s.x.to_f32(), s.y.to_f32(), s.z.to_f32()];
                                                let yaw = s.yaw as f32 / 10000.0;
                                                lan_remote_last_seen.insert(s.id, now_ms);
                                                match lan_remote_creatures.get_mut(&s.id) {
                                                    Some(cr) => {
                                                        cr.body.position = pos;
                                                        cr.yaw = yaw;
                                                    }
                                                    None => {
                                                        // r494: build from the peer's known
                                                        // appearance if we've received it, else
                                                        // a default until CH_AVATAR arrives.
                                                        let mut cr = match lan_remote_appearance.get(&s.id) {
                                                            Some(profile) => unendlich_engine::spawn::from_blueprint(
                                                                &profile.blueprint(), pos,
                                                            ),
                                                            None => unendlich_engine::spawn::spawn_beta_dragon(pos),
                                                        };
                                                        cr.body.position = pos;
                                                        cr.yaw = yaw;
                                                        lan_remote_creatures.insert(s.id, cr);
                                                        log::info!(
                                                            "[lan-mp] spawned remote avatar for entity {} at ({:.1},{:.1},{:.1})",
                                                            s.id, pos[0], pos[1], pos[2]
                                                        );
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    CH_WORLDCMD => {
                                        // r498: payload is packet_compress-framed.
                                        if let Some(raw) = unendlich_engine::sim::net::packet_compress::unpack(payload, 64 * 1024) {
                                        if let Some(cmds) = decode_batch(&raw) {
                                            use unendlich_engine::sim::world_authority::WorldCmd;
                                            for (_player, cmd) in &cmds {
                                                match cmd {
                                                    WorldCmd::Place { pos, block } => {
                                                        if let Ok(bid) =
                                                            BlockId::try_from(*block as u8)
                                                        {
                                                            chunk_manager
                                                                .set_block(pos.0, pos.1, pos.2, bid);
                                                        }
                                                    }
                                                    WorldCmd::Remove { pos } => {
                                                        chunk_manager.set_block(
                                                            pos.0, pos.1, pos.2, BlockId::Air,
                                                        );
                                                    }
                                                    _ => {}
                                                }
                                            }
                                        }
                                        }
                                    }
                                    CH_AVATAR => {
                                        // r494: [id u64 LE][appearance 12B]. Store the
                                        // peer's character; if their avatar already exists,
                                        // rebuild it from the new look (keep pos/yaw).
                                        if payload.len() >= 20 {
                                            let mut idb = [0u8; 8];
                                            idb.copy_from_slice(&payload[0..8]);
                                            let id = u64::from_le_bytes(idb);
                                            if let Some(profile) =
                                                unendlich_engine::player_character::CharacterProfile::from_appearance_bytes(&payload[8..20])
                                            {
                                                let changed = lan_remote_appearance
                                                    .get(&id)
                                                    .map(|p| p != &profile)
                                                    .unwrap_or(true);
                                                if changed {
                                                    if let Some(cr) = lan_remote_creatures.get(&id) {
                                                        let (pos, yaw) = (cr.body.position, cr.yaw);
                                                        let mut nc = unendlich_engine::spawn::from_blueprint(
                                                            &profile.blueprint(), pos,
                                                        );
                                                        nc.body.position = pos;
                                                        nc.yaw = yaw;
                                                        lan_remote_creatures.insert(id, nc);
                                                    }
                                                    lan_remote_appearance.insert(id, profile);
                                                    log::info!("[lan-mp] learned appearance for entity {id}");
                                                }
                                            }
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            Ok(_) => {} // empty datagram
                            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                            Err(_) => break,
                        }
                    }

                    // r404: liveness — drop avatars we haven't heard from in 5s, so a
                    // departed player's avatar doesn't linger forever.
                    const PEER_TIMEOUT_MS: u64 = 5000;
                    let stale: Vec<u64> = lan_remote_last_seen
                        .iter()
                        .filter(|(_, &t)| now_ms.saturating_sub(t) > PEER_TIMEOUT_MS)
                        .map(|(&id, _)| id)
                        .collect();
                    for id in stale {
                        lan_remote_creatures.remove(&id);
                        lan_remote_appearance.remove(&id); // r494
                        lan_remote_last_seen.remove(&id);
                        log::info!("[lan-mp] remote entity {id} timed out — avatar removed");
                    }

                    // Outbound handshake: a clicked host → send a JOIN to its candidate.
                    if renderer.menu_mp_pick >= 0 {
                        let pick = renderer.menu_mp_pick as usize;
                        renderer.menu_mp_pick = -1;
                        if let Some(addr) = lan_host_addrs.get(pick).copied() {
                            let join = LanJoin::Join { name: "joiner".to_string() }.encode();
                            let mut dg = vec![CH_HANDSHAKE];
                            dg.extend_from_slice(&join);
                            match sock.send_to(&dg, addr) {
                                Ok(_) => {
                                    log::info!("[lan-mp] sent JOIN to {addr}");
                                    lan_peer_addr = Some(addr); // joiner replicates to the host
                                }
                                Err(e) => log::warn!("[lan-mp] JOIN send failed: {e}"),
                            }
                        }
                    }

                    // r405: direct-connect — retry a JOIN to UNENDLICH_LAN_HOST every 2s
                    // until accepted (peer set), so the join path can be tested without
                    // multicast discovery.
                    if let Some(host) = lan_direct_host {
                        if lan_peer_addr.is_none() && now_ms.saturating_sub(lan_direct_last_ms) >= 2000 {
                            lan_direct_last_ms = now_ms;
                            let join = LanJoin::Join { name: "joiner".to_string() }.encode();
                            let mut dg = vec![CH_HANDSHAKE];
                            dg.extend_from_slice(&join);
                            match sock.send_to(&dg, host) {
                                Ok(_) => log::info!("[lan-mp] direct JOIN -> {host}"),
                                Err(e) => log::warn!("[lan-mp] direct JOIN failed: {e}"),
                            }
                        }
                    }

                    // Outbound replication: in-game with a peer → send our player transform
                    // ~15/sec (host-authoritative for our own entity; seal() stamps the seq).
                    if fg_app.state() == AppState::InGame {
                        if let Some(peer) = lan_peer_addr {
                            if now_ms.saturating_sub(lan_repl_last_ms) >= 66 {
                                lan_repl_last_ms = now_ms;
                                lan_repl_tick += 1;
                                let p = local_player.view.position;
                                let me = EntityState {
                                    id: lan_session_port as u64,
                                    kind: EntityKind::Player,
                                    x: Fixed::from_f64(p.x as f64),
                                    y: Fixed::from_f64(p.y as f64),
                                    z: Fixed::from_f64(p.z as f64),
                                    // yaw in radians × 10000, fits i16 (±π·1e4 ≈ ±31416)
                                    yaw: (local_player.view.yaw * 10000.0).clamp(-32767.0, 32767.0) as i16,
                                    pitch: 0,
                                    seq: 0, // seal() assigns the authoritative seq
                                };
                                let bytes = seal(&mut lan_stamper, lan_repl_tick, &[me]);
                                let mut dg = vec![CH_ENTITY];
                                dg.extend_from_slice(&bytes);
                                let _ = sock.send_to(&dg, std::net::SocketAddr::V4(peer));
                            }

                            // r494: broadcast our chosen-character appearance ~every 2s so
                            // the peer renders us as our character, not a default avatar.
                            if now_ms.saturating_sub(lan_avatar_last_sent_ms) >= 2000 {
                                lan_avatar_last_sent_ms = now_ms;
                                let mut dg = vec![CH_AVATAR];
                                dg.extend_from_slice(&(lan_session_port as u64).to_le_bytes());
                                dg.extend_from_slice(&active_character.to_appearance_bytes());
                                let _ = sock.send_to(&dg, std::net::SocketAddr::V4(peer));
                            }

                            // Outbound block edits: drain this frame's place/break edits
                            // to the peer so building/mining propagates.
                            if !session.lan_outgoing_edits.is_empty() {
                                let pid = lan_session_port as u64;
                                let batch: Vec<_> = session.lan_outgoing_edits
                                    .drain(..)
                                    .map(|cmd| (pid, cmd))
                                    .collect();
                                let bytes = encode_batch(&batch);
                                // r498: compress bulk edit batches (raw header for
                                // small ones — single byte, never expands).
                                let mut dg = vec![CH_WORLDCMD];
                                dg.extend_from_slice(
                                    &unendlich_engine::sim::net::packet_compress::pack(&bytes, 48),
                                );
                                let _ = sock.send_to(&dg, std::net::SocketAddr::V4(peer));
                            }
                        }
                    } else {
                        // Not in game / no peer: don't let edits accumulate unbounded.
                        if lan_peer_addr.is_none() {
                            session.lan_outgoing_edits.clear();
                        }
                    }
                }

                if now_ms.saturating_sub(lan_last_ms) >= 200 {
                    lan_last_ms = now_ms;
                    // Drain inbound beacons → browser; log newly-seen hosts once.
                    for adv in beacon.poll(now_ms) {
                        let key = adv.key();
                        if lan_seen_keys.insert(key) {
                            log::info!(
                                "[lan-mp] discovered host '{}' world '{}' seed {} ({} player(s))",
                                adv.host_username, adv.world_name, adv.world_seed, adv.players
                            );
                        }
                        lan_browser.observe(adv);
                    }
                    lan_browser.prune(now_ms, 45_000);
                    // While in-game we are a host: announce the live world on cadence.
                    if fg_app.state() == AppState::InGame {
                        let advert = unendlich_engine::sim::session_mode::WorldAdvert {
                            host_username: "host".to_string(),
                            world_name: current_world_name.clone().unwrap_or_else(|| "World".into()),
                            players: 1,
                            max_players: 8,
                            last_seen_ms: now_ms,
                            candidates: match lan_local_ip {
                                Some(ip) => vec![unendlich_engine::sim::lan_discovery::lan_candidate(
                                    ip,
                                    lan_session_port,
                                )],
                                None => Vec::new(),
                            },
                            world_seed: fg_world_seed,
                        };
                        if let Some(refreshed) = lan_announcer.poll(&advert, now_ms) {
                            match beacon.announce(&refreshed) {
                                Ok(()) => log::info!(
                                    "[lan-mp] announced world '{}' seed {}",
                                    refreshed.world_name, refreshed.world_seed
                                ),
                                Err(e) => log::warn!("[lan-mp] announce failed: {e}"),
                            }
                        }
                    }
                    // Feed the Multiplayer screen's discovered-host list.
                    // Build the display list and the parallel address list from ONE
                    // filtered pass (only hosts that carry a candidate are joinable),
                    // so a clicked index always resolves to the right address.
                    let recent = lan_browser.by_recent();
                    let joinable: Vec<(String, std::net::SocketAddrV4)> = recent
                        .iter()
                        .filter_map(|a| {
                            let c = a.candidates.first()?;
                            let addr = std::net::SocketAddrV4::new(
                                std::net::Ipv4Addr::new(c.host[0], c.host[1], c.host[2], c.host[3]),
                                c.port,
                            );
                            let label = format!(
                                "{} — {} (seed {}) @ {}",
                                a.host_username, a.world_name, a.world_seed, addr
                            );
                            Some((label, addr))
                        })
                        .collect();
                    renderer.menu_mp_list = joinable.iter().map(|(l, _)| l.clone()).collect();
                    lan_host_addrs = joinable.iter().map(|(_, a)| *a).collect();
                }
            }
            renderer.menu_open = in_menu
                && matches!(
                    screen,
                    Screen::Title
                        | Screen::Singleplayer // r660: was missing → SP submenu hid the overlay
                        | Screen::LoadSlots
                        | Screen::Multiplayer
                        | Screen::Settings
                        | Screen::ConfirmQuit
                );
            renderer.wizard_open = in_menu && screen == Screen::NewGame;
            if let Some(w) = fg_app.menu().wizard() {
                renderer.wizard_title = w.step().title().to_string();
                renderer.wizard_summary = w.summary();
                renderer.wizard_can_next = w.can_advance();
                renderer.wizard_at_confirm = w.at_confirm();
            }
            {
                // r186: tell the renderer WHICH menu screen to draw.
                renderer.menu_screen_kind = match fg_app.menu().screen() {
                    Screen::ConfirmQuit => 1,
                    Screen::Multiplayer => 2,
                    Screen::LoadSlots => 3, // r197
                    Screen::Singleplayer => 4, // r656: SP submenu
                    _ => 0,
                };
                // r197: feed the saved-world list to the renderer while the
                // Load screen is up (cheap: only when on LoadSlots).
                if matches!(fg_app.menu().screen(), Screen::LoadSlots) {
                    let entries = load_catalog_entries();
                    renderer.menu_world_list = entries
                        .iter()
                        .map(|e| {
                            let mode = match e.mode {
                                unendlich_engine::sim::world_catalog::GameMode::Survival => "survival",
                                _ => "creative",
                            };
                            format!("{}  ·  seed {}  ·  {}", e.name, e.seed, mode)
                        })
                        .collect();
                    // r197: a clicked row -> launch that world via the same
                    // StartLocal path the wizard uses (seed/name/mode all set).
                    if renderer.menu_world_pick >= 0 {
                        let idx = renderer.menu_world_pick as usize;
                        renderer.menu_world_pick = -1;
                        if let Some(e) = entries.get(idx) {
                            fg_pending_load = Some((e.name.clone(), e.seed, matches!(
                                e.mode,
                                unendlich_engine::sim::world_catalog::GameMode::Survival
                            )));
                            log::info!("[load] selected world '{}' (seed {})", e.name, e.seed);
                        }
                    }
                    // r203: ROLLBACK BROWSE. "history" on a world row loads its
                    // restore points (newest first) into the renderer so the
                    // player can step backward through them. Reads the .point
                    // markers written during play (worlds/<name>/rollbacks/).
                    if renderer.menu_rollback_peek >= 0 {
                        let idx = renderer.menu_rollback_peek as usize;
                        renderer.menu_rollback_peek = -1;
                        if let Some(e) = entries.get(idx) {
                            let pts = list_rollback_points(&e.name);
                            renderer.menu_rollback_list = pts
                                .iter()
                                .map(|p| {
                                    // human time-ago from the unix-second ts
                                    let ago = std::time::SystemTime::now()
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .map(|d| d.as_secs().saturating_sub(p.ts))
                                        .unwrap_or(0);
                                    let when = if ago < 90 { format!("{ago}s ago") }
                                        else if ago < 5400 { format!("{}m ago", ago / 60) }
                                        else { format!("{}h ago", ago / 3600) };
                                    format!("{}  ·  {}  ·  {} chunks", when, p.reason, p.chunks)
                                })
                                .collect();
                            renderer.menu_show_rollbacks = true;
                            fg_rollback_world = Some(e.name.clone());
                            log::info!("[rollback] {} point(s) for '{}'", pts.len(), e.name);
                        }
                    }
                    // r203: restore a chosen point. Full chunk-snapshot restore
                    // is staged separately; for now we record the intent + the
                    // marker so the player sees the selection take effect and
                    // the (future) restore loads from this exact point.
                    if renderer.menu_rollback_restore >= 0 {
                        let idx = renderer.menu_rollback_restore as usize;
                        renderer.menu_rollback_restore = -1;
                        if let Some(name) = &fg_rollback_world {
                            let pts = list_rollback_points(name);
                            if let Some(p) = pts.get(idx) {
                                log::info!(
                                    "[rollback] restore requested: '{}' point ts={} ({})",
                                    name, p.ts, p.reason
                                );
                                // Stage the restore: the world will reload from
                                // this point's saved chunks on next launch.
                                std::fs::write(
                                    format!("worlds/{}/rollbacks/RESTORE",
                                        name.replace(['/', '\\', ' '], "_")),
                                    format!("{}", p.ts),
                                ).ok();
                            }
                        }
                    }
                    // r206: delete a single rollback point (operator: "deleting
                    // bad points in the world history would be nice"). Removes
                    // that .point marker from disk.
                    if renderer.menu_rollback_delete >= 0 {
                        let idx = renderer.menu_rollback_delete as usize;
                        renderer.menu_rollback_delete = -1;
                        if let Some(name) = &fg_rollback_world {
                            let pts = list_rollback_points(name);
                            if let Some(p) = pts.get(idx) {
                                let path = format!(
                                    "worlds/{}/rollbacks/{}.point",
                                    name.replace(['/', '\\', ' '], "_"), p.ts
                                );
                                if let Err(e) = std::fs::remove_file(&path) {
                                    log::warn!("[rollback] delete point {}: {e}", p.ts);
                                } else {
                                    log::info!("[rollback] deleted point ts={}", p.ts);
                                }
                                // refresh the displayed list
                                let fresh = list_rollback_points(name);
                                renderer.menu_rollback_list = fresh
                                    .iter()
                                    .map(|q| {
                                        let ago = std::time::SystemTime::now()
                                            .duration_since(std::time::UNIX_EPOCH)
                                            .map(|d| d.as_secs().saturating_sub(q.ts))
                                            .unwrap_or(0);
                                        let when = if ago < 90 { format!("{ago}s ago") }
                                            else if ago < 5400 { format!("{}m ago", ago / 60) }
                                            else { format!("{}h ago", ago / 3600) };
                                        format!("{}  ·  {}  ·  {} chunks", when, q.reason, q.chunks)
                                    })
                                    .collect();
                            }
                        }
                    }
                    // r205: DELETE a world (confirmed in the menu). Remove its

                    // catalog entry AND its on-disk world dir so it stops
                    // showing in Load. Guarded by the confirm row in the UI.
                    if renderer.menu_world_delete_confirm >= 0 {
                        let idx = renderer.menu_world_delete_confirm as usize;
                        renderer.menu_world_delete_confirm = -1;
                        if let Some(e) = entries.get(idx) {
                            let name = e.name.clone();
                            // drop from catalog
                            let mut remaining = entries.clone();
                            remaining.retain(|w| w.name != name);
                            save_catalog(&remaining);
                            // delete the world's directory (best-effort)
                            let dir = format!("worlds/{}", name.replace(['/', '\\', ' '], "_"));
                            if let Err(err) = std::fs::remove_dir_all(&dir) {
                                log::warn!("[saves] delete '{}' dir: {err}", name);
                            }
                            log::info!("[saves] deleted world '{}'", name);
                        }
                    }
                }
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
            let p75 = if n > 0 { sorted[(n * 75 / 100).min(n - 1)] } else { 0.0 };
            let p99 = if n > 0 { sorted[(n * 99 / 100).min(n - 1)] } else { 0.0 };
            let max = sorted.last().copied().unwrap_or(0.0);
            let mean = if n > 0 { sorted.iter().sum::<f64>() / n as f64 } else { 0.0 };

            log::info!(
                "FPS: {:>4}  frame: p50={:.1}ms p75={:.1}ms p99={:.1}ms mean={:.1}ms max={:.1}ms  pos: ({:>6.0},{:>6.0},{:>6.0})  loaded: {}  gen_pending: {}  rendering: {}  upload_fails: {}  mesh_q: {}/{}  fires: {}",
                frame_count,
                p50, p75, p99, mean, max,
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

        // r237: close out the frame for the profiler, and periodically dump
        // the per-zone percentile table (env-gated). The dump shows, per zone,
        // n/mean/p50/p95/p99/max sorted worst-p99-first — so a chug shows up
        // as a high p99/max on whichever phase is the culprit (manager_update
        // = chunk streaming/upload, tick_sim = world sim, draw = GPU submit/
        // present), which the averaged sums can't reveal.
        if profile_on {
            prof.record("frame_total", t_frame_start.elapsed().as_secs_f64() * 1e3);
            if last_profile_dump.elapsed().as_secs_f32() >= PROFILE_DUMP_SECS {
                log::info!("[profile] per-zone percentiles (ms):\n{}", prof.dump());
                last_profile_dump = Instant::now();
            }
        }
    }

    let elapsed = start.elapsed();
    let saved = chunk_manager.save_all();
    if saved > 0 {
        log::info!("Saved {} edited chunks before shutdown", saved);
    }
    // r514: flush the persistent flora/crop store so growth survives the session.
    if let Err(e) = session.flora.save(&flora_root) {
        log::warn!("[farm] failed to save flora state on shutdown: {}", e);
    } else if !session.flora.is_empty() {
        log::info!("Saved {} crop(s) before shutdown", session.flora.len());    }
    // r518: persist the server access policy + allow-list.
    let _ = whitelist.save(&flora_root);
    // r671: persist lifetime chi (Resonance Rank) onto the GLOBAL profile — reload
    // it and update only this field so we never clobber the custom character's
    // name/build even if the player is currently wearing a preset hero.
    {
        let mut g = unendlich_engine::player_character::load_global();
        g.lifetime_chi = fg_lifetime_chi;
        if let Err(e) = unendlich_engine::player_character::save_global(&g) {
            log::warn!("[rank] failed to persist lifetime chi: {}", e);
        } else {
            log::info!("[rank] persisted lifetime chi = {}", fg_lifetime_chi);
        }
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

    // r172: AGGREGATE — a lava lake used to register one source PER BLOCK
    // (operator log: fires: 11372, FPS 1). One centroid source per chunk
    // carries the same gameplay heat at a fraction of the cost: strength
    // scales with surface count (capped), radius widened to cover the pool.
    let n = to_add.len();
    if n > 0 {
        let inv = 1.0 / n as f32;
        let mut cx = 0.0f32;
        let mut cy = 0.0f32;
        let mut cz = 0.0f32;
        for s in &to_add {
            cx += s.position[0] * inv;
            cy += s.position[1] * inv;
            cz += s.position[2] * inv;
        }
        // r178: cap lower — 2000-strength centroids saturated the body
        // model at depth (282 deg C log). Warmth, not a furnace.
        let strength = 300.0 + 8.0 * (n.min(60) as f32);
        heat_sources.add_many(vec![HeatSource::new([cx, cy, cz], strength, 9.0)]);
    }
    n.min(1)
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

