//! Game-wide constants and configuration values.

// Window
pub const WINDOW_WIDTH: u32 = 1920;
pub const WINDOW_HEIGHT: u32 = 1080;
pub const WINDOW_NAME: &str = "Unendlich";

// World
//
// Chunks are 32×32×32 cubes. The world is unbounded vertically — a
// `ChunkCoord` has y in addition to x,z, and chunks stack arbitrarily.
// At i32 max for y, that's ~66 billion blocks of nominal vertical range,
// which is "effectively unbounded" for any plausible gameplay.
//
// We dropped CHUNK_HEIGHT as a constant because it no longer means anything:
// surface y is derived from density, not stored, and the world has no fixed
// vertical extent.
pub const CHUNK_SIZE: i32 = 32;
pub const CHUNK_VOLUME: usize = (CHUNK_SIZE * CHUNK_SIZE * CHUNK_SIZE) as usize;

/// Approximate sea level y, used by the density function as the "neutral"
/// elevation. Density is biased toward solid below this and air above.
pub const SEA_LEVEL: i32 = 64;

/// 3D render distance in chunks (Euclidean sphere). At 6, the loaded set is
/// up to ~(4/3)π·6³ ≈ 905 chunks. Each 32^3 chunk is 32 KB of block data, so
/// the loaded set is ~29 MB of block storage. Comfortable on M2 Max with
/// CPU-only generation (~600 cps observed = ~1.5s initial load).
///
/// IMPORTANT: this value is shared between the chunk_manager (what to
/// generate) and renderer-driver (what to upload). Bumping it requires
/// either GPU world gen working or LOD for far chunks; otherwise generation
/// can't keep up with player movement.
/// Default render distance in chunks. Env var
/// `UNENDLICH_RENDER_DISTANCE` overrides at runtime via
/// [`runtime_render_distance`]. The const remains for code paths
/// that need a compile-time constant (mostly old tests); production
/// call sites should use the function.
///
/// IMPORTANT: this value is shared between the chunk_manager (what to
/// generate) and renderer-driver (what to upload). Bumping it requires
/// either GPU world gen working or LOD for far chunks; otherwise generation
/// can't keep up with player movement.
pub const RENDER_DISTANCE: i32 = 6;

/// Resolve the render distance: env var if set and valid, else
/// [`RENDER_DISTANCE`]. Clamped to [1, 256] to prevent silly values
/// (RD=512 with cubic chunk-iteration is 1.3 BILLION coords per sweep).
///
/// Read once at startup; env-var changes mid-run aren't supported
/// because the value is also baked into the chunk manager's eviction
/// radius at construction time.
/// r197: live override set by the settings slider. 0 = unset (fall back
/// to env var / const). Takes precedence over the env var when > 0 so a
/// player can change render distance from the menu without a restart.
pub static RENDER_DISTANCE_OVERRIDE: std::sync::atomic::AtomicI32 =
    std::sync::atomic::AtomicI32::new(0);

/// Set the live render-distance override (clamped to [1,64]; 0 clears).
pub fn set_render_distance_override(n: i32) {
    let v = if n == 0 { 0 } else { n.clamp(1, 64) };
    RENDER_DISTANCE_OVERRIDE.store(v, std::sync::atomic::Ordering::Relaxed);
}

/// r476: live GPU-mesher throttle, in percent. After each octree dispatch the
/// dispatcher sleeps `dispatch_time * pct/100`, leaving the shared GPU queue
/// idle so the render pass can land a frame. Settable live from the Performance
/// page (and seeded from UNENDLICH_GPU_MESH_YIELD_PCT at startup). 0 = off.
pub static GPU_MESH_YIELD_PCT: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

pub fn set_gpu_mesh_yield_pct(n: u64) {
    GPU_MESH_YIELD_PCT.store(n.min(1000), std::sync::atomic::Ordering::Relaxed);
}

pub fn gpu_mesh_yield_pct() -> u64 {
    GPU_MESH_YIELD_PCT.load(std::sync::atomic::Ordering::Relaxed)
}

/// r480: live toggle for the tiled shared-memory octree mesher path
/// (chunk_mesh_octree.comp, only relevant when GPU_MESHER=2). Seeded from
/// UNENDLICH_OCTREE_TILE; settable live from the Performance page.
pub static OCTREE_TILE_ENABLED: std::sync::atomic::AtomicU32 =
    std::sync::atomic::AtomicU32::new(0);

pub fn set_octree_tile(on: bool) {
    OCTREE_TILE_ENABLED.store(on as u32, std::sync::atomic::Ordering::Relaxed);
}

pub fn octree_tile_enabled() -> u32 {
    OCTREE_TILE_ENABLED.load(std::sync::atomic::Ordering::Relaxed)
}

pub fn runtime_render_distance() -> i32 {
    let ovr = RENDER_DISTANCE_OVERRIDE.load(std::sync::atomic::Ordering::Relaxed);
    if ovr > 0 {
        return ovr;
    }
    match std::env::var("UNENDLICH_RENDER_DISTANCE") {
        Ok(v) => match v.parse::<i32>() {
            Ok(n) if (1..=256).contains(&n) => n,
            Ok(n) => {
                log::warn!("UNENDLICH_RENDER_DISTANCE={} out of [1,256]; using {}", n, RENDER_DISTANCE);
                RENDER_DISTANCE
            }
            Err(_) => {
                log::warn!("UNENDLICH_RENDER_DISTANCE={} not a valid i32; using {}", v, RENDER_DISTANCE);
                RENDER_DISTANCE
            }
        },
        Err(_) => RENDER_DISTANCE,
    }
}

pub const WORLD_SEED_DEFAULT: u64 = 0xDEAD_BEEF_CAFE_F00D;

// Physics
pub const PHYSICS_TICKRATE: f32 = 60.0;
pub const GRAVITY: f32 = 32.0;
pub const TERMINAL_VELOCITY: f32 = 78.4;
pub const HORIZONTAL_ACCELERATION: f32 = 60.0;
pub const HORIZONTAL_DAMPING: f32 = 12.0;

// JUMP_IMPULSE = sqrt(2 * GRAVITY * JUMP_HEIGHT)
//              = sqrt(2 * 32.0 * 1.25) = sqrt(80) ≈ 8.944
pub const JUMP_IMPULSE: f32 = 8.944_272;

// Player
pub const PLAYER_HEIGHT: f32 = 1.8;
pub const PLAYER_WIDTH: f32 = 0.6;
pub const PLAYER_EYE_HEIGHT: f32 = 1.62;
pub const REACH_DISTANCE: f32 = 5.0;

// GPU Generation
pub const GPU_BATCH_SIZE: usize = 16;
pub const GPU_MAX_QUEUED_CHUNKS: usize = 64;

/// Default chunk-loader worker thread count.
pub fn default_worker_count() -> usize {
    let cpus = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    (cpus.saturating_sub(2)).max(2)
}
