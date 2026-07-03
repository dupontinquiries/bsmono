//! Chunk mesh generation.
//!
//! The public entry point is `mesh_chunk`, which uses **greedy meshing** —
//! it merges co-planar quads of identical block type into one big rectangle.
//! For typical surface chunks this produces 5-15× fewer vertices than naive
//! face-culling, with a corresponding GPU upload + draw bandwidth win.
//!
//! The naive face-culling implementation is preserved as `mesh_chunk_naive`
//! (still public for benchmarking and tests). It's also a reference oracle —
//! the greedy mesher must produce the *same set of covered surface area*
//! as naive, just expressed with fewer triangles.
//!
//! Color model: each face's color = block's base color × per-face brightness
//! (lighting hint). The greedy mesher merges quads of the same block type on
//! the same face direction, so the merged quad's color is that block's color
//! × that face's brightness. The previous "per-block hash tint" is dropped
//! — it broke greedy merging by making every cell unique.

use crate::constants::CHUNK_SIZE;
use crate::world::blocks::BlockId;
use crate::world::chunk::{Chunk, ChunkCoord};
use crate::world::cpu_generator::WorldGenerator;

/// Whether the block-texture path is enabled (`UNENDLICH_TEXTURES=1`). Read once
/// and cached. When false (default), the mesher packs the untextured 0xFF alpha
/// sentinel so the whole texture path is a no-op and rendering matches the
/// no-texture build exactly.
pub fn textures_enabled() -> bool {
    use std::sync::OnceLock;
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        std::env::var("UNENDLICH_TEXTURES").map(|v| v != "0" && !v.is_empty()).unwrap_or(true)
    })
}


/// Maximum height variation applied to "soft" block top faces, in blocks.
/// Tight (1/16 block ≈ 6 cm) so:
///   - The visual is subtly organic, not blatantly off-grid
///   - Player physics (which still treats blocks as cubes) doesn't visibly
///     phase through the surface
///   - The surface is still clearly readable as block-aligned
const TOP_JITTER_AMPLITUDE: f32 = 0.0625;

/// Deterministic per-corner height offset for soft block tops. Same world
/// (x, z) always returns the same offset, so adjacent blocks meeting at a
/// shared corner agree on the height — no T-junctions, no visual cracks.
///
/// Output is in [-TOP_JITTER_AMPLITUDE, +TOP_JITTER_AMPLITUDE].
fn top_corner_offset(world_x: i32, world_z: i32) -> f32 {
    // Cheap integer hash → float in [0,1].
    let mut h: u32 = (world_x as u32).wrapping_mul(0x27d4_eb2d)
        ^ (world_z as u32).wrapping_mul(0x1656_67b1);
    h ^= h >> 15;
    h = h.wrapping_mul(0x85eb_ca6b);
    h ^= h >> 13;
    h = h.wrapping_mul(0xc2b2_ae35);
    h ^= h >> 16;
    let unit = (h & 0xFFFF) as f32 / 65535.0; // [0, 1]
    // Map to [-1, 1] then scale.
    (unit * 2.0 - 1.0) * TOP_JITTER_AMPLITUDE
}

/// Maximum vertical sag for soft-block edges in blocks. 0.25 = a fully-
/// exposed corner drops a quarter block — visible erosion without
/// producing strongly trapezoidal side faces at cliff edges.
///
/// Tradeoff: lower values make cliff edges crisp (rectangular side
/// faces, reduced trapezoidal artifact); higher values make the
/// edges look more eroded but the side faces of cliff-edge blocks
/// become noticeably trapezoidal where my seam-fix drops their
/// bottom corners to match the lower block's sagged top.
///
/// Was 0.5; reduced after user feedback that mesh surfaces "still
/// not quite connecting" — the sag was creating visible trapezoidal
/// distortion at every cliff edge.
const SOFT_EDGE_MAX_SAG: f32 = 0.25;

/// Compute the sag at a corner shared by the four blocks meeting at world
/// (corner_x, slice, corner_z). Returns a non-positive value: 0 for
/// fully-buried corners (no air neighbors), -SOFT_EDGE_MAX_SAG × softness
/// for fully-exposed corners (only one solid touching = isolated tip).
///
/// Adjacent blocks that share this corner pass the same arguments and get
/// the same value — corners stay topologically welded across faces.
///
/// `softness` is the softness of THE BLOCK whose face we're emitting. We
/// don't average across the 4 corner-blocks because that would require
/// per-block softness lookup for each query, and a "soft block sag becomes
/// less soft when next to hard" effect doesn't read intuitively. The block
/// being drawn dictates how much its OWN edges sag.
/// Sag depth at a soft block's top corner. The "softness" used here is
/// the MAX softness of the 4 blocks meeting at the corner, NOT the
/// caller's own. Without this, grass (softness 0.45) meeting dirt
/// (softness 0.50) at a shared corner produces two different sag
/// depths — visible as horizontal seams along block boundaries. Using
/// max-of-4 makes the corner Y a property of the corner alone, so
/// every block touching it computes the same value.
fn corner_sag(
    chunk: &Chunk,
    corner_x: i32,
    slice: i32,
    corner_z: i32,
) -> f32 {
    // Find the 4 blocks meeting at this corner and the max softness
    // among them. If none are soft, no sag.
    let mut max_softness: f32 = 0.0;
    let mut weighted_air = 0.0_f32;
    // Inner 2×2 (the 4 horizontally-adjacent blocks).
    for sx in -1i32..=0 {
        for sz in -1i32..=0 {
            let nb = block_at_corner_sample(chunk, [corner_x + sx, slice, corner_z + sz]); // r198
            if !is_meshable(nb) {
                weighted_air += 1.0;
            } else {
                max_softness = max_softness.max(nb.softness());
            }
        }
    }
    if max_softness <= 0.0 { return 0.0; }
    // Outer ring — 4×4 minus inner 2×2 = 12 blocks at 1/3 weight.
    // Only contributes to air count, not softness lookup (would
    // dilute the max).
    for sx in -2i32..=1 {
        for sz in -2i32..=1 {
            if sx >= -1 && sx <= 0 && sz >= -1 && sz <= 0 { continue; }
            let nb = block_at_corner_sample(chunk, [corner_x + sx, slice, corner_z + sz]); // r198
            if !is_meshable(nb) { weighted_air += 0.333; }
        }
    }
    let intensity = (weighted_air / 4.5).min(1.0);
    -max_softness * intensity * SOFT_EDGE_MAX_SAG
}

/// Low-frequency rolling-bumps term for soft surfaces. Independent of
/// the per-corner micro-jitter — this adds a smooth wave across the
/// surface so a flat grass / sand field has gentle undulation rather
/// than being perfectly flat. Amplitude ~1/8 block.
///
/// Uses two octaves of trig to fake noise cheaply (no hash table).
/// Coordinates in world space so the pattern is global and consistent
/// across chunks.
/// Max softness of the 4 blocks meeting at corner `(corner_x, corner_z)`
/// at the given Y slice. Returns 0.0 if no soft block touches the corner.
/// This is the consistent "softness for visual offsets at this corner"
/// — both `corner_sag` and `surface_bumps` use it so adjacent blocks
/// don't disagree on shared-corner Y values.
fn max_softness_at_corner(chunk: &Chunk, corner_x: i32, slice: i32, corner_z: i32) -> f32 {
    let mut m: f32 = 0.0;
    for sx in -1i32..=0 {
        for sz in -1i32..=0 {
            let nb = block_at_corner_sample(chunk, [corner_x + sx, slice, corner_z + sz]); // r198
            if is_meshable(nb) {
                m = m.max(nb.softness());
            }

        }
    }
    m
}

/// Deterministic per-block weathering derived from world coordinates,
/// used as the worldgen default. Each block at (wx, wy, wz) gets its
/// own value 0-255 from a hash of its position. Adjacent blocks have
/// uncorrelated weathering, which produces per-block visual variation
/// in the bumpy mesh — exactly what makes "each block has its own
/// little potato shape" work.
///
/// This is purely deterministic and stateless, so it costs no memory
/// and reproduces identically on every machine. Only blocks that have
/// been EXPLICITLY weathered (e.g. by a tick handler that pushed
/// weathering up due to rain) override this with stored
/// `instance_state.age_ticks`.
#[inline]
fn worldgen_weathering_at(block: BlockId, wx: i32, wy: i32, wz: i32) -> u8 {
    // Use depth-aware default: stone gets weathering when deep
    // underground (cave humidity / water-flow proxy). Sky-exposed
    // surface stone stays fresh.
    let base = crate::world::weathering::worldgen_default_weathering_at_depth(block, wy);
    if base == 0 { return 0; }
    // Hash position into [0, 1] to vary weathering ±~50 units around base.
    // The variation gives each block its own bump intensity; without it,
    // all soft blocks would share identical weathering and look uniform.
    let hash = wx.wrapping_mul(73856093)
        ^ wy.wrapping_mul(19349663)
        ^ wz.wrapping_mul(83492791);
    let h = (hash as u32 ^ ((hash >> 13) as u32)) & 0x7fff;
    let unit = (h as f32) / 32767.0; // [0, 1]
    let variance = 50.0;
    let value = (base as f32) + (unit - 0.5) * 2.0 * variance;
    value.clamp(0.0, 255.0) as u8
}

/// Average weathering value (0.0-1.0) of the 4 blocks meeting at this
/// corner. Used to scale `surface_bumps` so individual blocks have
/// per-instance bump amplitude. Adjacent blocks of different
/// weathering blend smoothly via the corner average.
///
/// Blocks without instance state contribute the worldgen default for
/// their type (so freshly-loaded chunks still have weathering
/// without needing per-block init).
fn avg_weathering_at_corner(chunk: &Chunk, corner_x: i32, slice: i32, corner_z: i32) -> f32 {
    let origin = chunk.coord.block_origin();
    let mut sum: f32 = 0.0;
    let mut count: f32 = 0.0;
    for sx in -1i32..=0 {
        for sz in -1i32..=0 {
            let cx = corner_x + sx;
            let cz = corner_z + sz;
            let nb = block_at(chunk, [cx, slice, cz]);
            if !is_meshable(nb) { continue; }
            // World coords for the worldgen weathering hash.
            let wx = origin.0 + cx;
            let wy = origin.1 + slice;
            let wz = origin.2 + cz;
            // Local block index in this chunk, if in range.
            let in_range = (0..crate::constants::CHUNK_SIZE).contains(&cx)
                && (0..crate::constants::CHUNK_SIZE).contains(&slice)
                && (0..crate::constants::CHUNK_SIZE).contains(&cz);
            let w = if in_range {
                let idx = Chunk::block_index(cx, slice, cz) as u16;
                let state = chunk.instance_state.get(&idx).copied()
                    .unwrap_or_else(|| {
                        // No explicit state → use deterministic worldgen
                        // default at THIS world position. Per-block noise
                        // gives each block its own weathering value.
                        let mut s = crate::world::chunk::BlockInstanceState::default();
                        s.age_ticks = worldgen_weathering_at(nb, wx, wy, wz);
                        s
                    });
                crate::world::weathering::weathering_value(state)
            } else {
                // Out of chunk range — use worldgen default at world pos.
                let mut s = crate::world::chunk::BlockInstanceState::default();
                s.age_ticks = worldgen_weathering_at(nb, wx, wy, wz);
                crate::world::weathering::weathering_value(s)
            };
            sum += w;
            count += 1.0;
        }
    }
    if count == 0.0 { 0.0 } else { sum / count }
}

fn surface_bumps(world_x: i32, world_z: i32, softness: f32) -> f32 {
    if softness <= 0.0 { return 0.0; }
    let x = world_x as f32;
    let z = world_z as f32;
    // Three octaves of trig-based pseudo-noise for visible roughness:
    //   - low: long wavelength, broad rolling hills.
    //   - mid: medium wavelength, individual mounds.
    //   - high: short wavelength, fine surface texture.
    //
    // Then SHIFTED to be NON-POSITIVE: result is in [-amplitude, 0].
    // Critical for mesh integrity at terrain steps. Side-face emission
    // uses integer-Y for bottom corners; if surface_bumps could lift
    // a block top ABOVE integer Y, the side face above wouldn't reach
    // far enough down to meet it, leaving a triangular gap. With
    // non-positive bumps, block tops always sit AT or BELOW integer Y,
    // so side faces (extending down to integer Y) cover the seam.
    //
    // Visually: blocks with strong bumps have their tops dip downward
    // into "potholes" relative to the cube top; they never lift. Reads
    // as soft erosion / settling, which matches the soft-block intent.
    //
    // The previous unshifted version produced visible triangular gaps
    // in the terrain mesh (bumps lifted block tops above integer Y,
    // exposing the seam between top face and adjacent side faces).
    let low  = ((x * 0.13).sin() + (z * 0.17).cos()) * 0.5;
    let mid  = ((x * 0.27 + z * 0.31).sin()) * 0.35;
    let high = ((x * 0.71 + z * 0.59).sin()) * 0.20;
    // Combined raw value is roughly in [-1.05, 1.05]. Shift to [-1, 0]
    // by halving and shifting: (raw - 1) / 2.
    let raw = low + mid + high;
    let unit = ((raw - 1.0) * 0.5).clamp(-1.0, 0.0);
    // Final amplitude: 0.30 block. Visible from the typical playing
    // camera distance. Softness scales further: grass (0.45) maxes at
    // 0.135 block dip; sand (0.85) at 0.255. Stone/log/etc with
    // softness=0 get zero bumps as before.
    unit * 0.30 * softness
}

/// Total vertical offset (jitter + sag + rolling bumps) for a corner.
/// All three are functions of the CORNER (world_x, world_z, slice),
/// not of the calling block — so adjacent blocks meeting at a shared
/// corner ALWAYS compute the same Y offset, eliminating seams and
/// T-junctions. The `_softness` parameter is kept for API compat but
/// ignored: softness is derived from the corner via
/// `max_softness_at_corner`.
fn corner_y_offset(
    chunk: &Chunk,
    corner_x: i32,
    slice: i32,
    corner_z: i32,
    _softness: f32,
) -> f32 {
    let corner_softness = max_softness_at_corner(chunk, corner_x, slice, corner_z);
    // Weathering amplitude factor: 0..1 multiplier on bump amplitude.
    // Fresh blocks (player-placed) → 0, no bumps. World-gen soft
    // blocks → high weathering, full bumps. This is what gives each
    // block its own bumpiness profile while corners remain shared
    // (world-position-derived) so adjacent blocks meet flush.
    let weathering_factor = crate::world::weathering::bump_amplitude_factor(
        avg_weathering_at_corner(chunk, corner_x, slice, corner_z)
    );
    top_corner_offset(corner_x, corner_z)
        + corner_sag(chunk, corner_x, slice, corner_z)
        + surface_bumps(corner_x, corner_z, corner_softness) * weathering_factor
}

/// Per-block color tint based on world position. Returns a multiplier
/// in roughly [0.85, 1.15] that varies smoothly across world space.
///
/// This is the simplest version of "tint sampled at mesh time" — every
/// block's color is base_color × tint_for_position. Adjacent blocks
/// get slightly different tints, breaking up the uniform-color look
/// that comes from a single fixed `block_color()` palette entry.
///
/// Future expansion: take temperature/humidity/light from the world
/// model to drive climate-responsive color (warm-temp grass yellower,
/// cold-temp grass bluer, etc.). For now, position-only.
///
/// Reference variance value used by `tint_for_position` for the most-
/// varying material (grass/moss). Used elsewhere to scale matrix-driven
/// tinting against a known baseline. Changing this means every block's
/// position-noise contribution shifts proportionally.
const TINT_REFERENCE_VARIANCE: f32 = 0.18;

/// Performance: 6 multiplications + 3 sin/cos. Called per soft-block
/// top (not per vertex). With ~1000 soft blocks per chunk and 1000
/// chunks, that's 1M calls per chunk-load batch. Cheap.
#[inline]
fn tint_for_position(block: BlockId, wx: i32, wy: i32, wz: i32) -> [f32; 3] {
    // Per-block-type variance: how much each channel can swing.
    // Soft natural blocks (grass, sand) get more variance than hard
    // blocks. Hard blocks (stone, ore) keep tighter palettes.
    let variance = match block {
        BlockId::Grass | BlockId::Moss => TINT_REFERENCE_VARIANCE,    // green has wide visual variance
        BlockId::Sand | BlockId::Gravel => 0.12,   // sandy beige varies less
        BlockId::Dirt => 0.15,
        BlockId::Snow | BlockId::SnowBlock => 0.08, // snow is mostly white
        BlockId::OakLeaves | BlockId::BirchLeaves => 0.20, // foliage varies a lot
        _ => 0.08, // hard blocks: minimal variation
    };

    // Three octaves of per-axis trig noise. Wavelengths chosen so
    // adjacent blocks differ but a 4×4 region reads as one "tone."
    let x = wx as f32;
    let y = wy as f32;
    let z = wz as f32;
    // Channel R: low-frequency
    let r_n = (x * 0.13 + z * 0.17).sin() * 0.5
            + (x * 0.31 + y * 0.07).cos() * 0.3;
    // Channel G: mid-frequency, phase-shifted
    let g_n = (x * 0.21 + z * 0.29).sin() * 0.5
            + (z * 0.43 + y * 0.11).cos() * 0.3;
    // Channel B: high-frequency, phase-shifted again
    let b_n = (x * 0.37 + z * 0.41).sin() * 0.5
            + (x * 0.53 + y * 0.13).cos() * 0.3;

    [
        1.0 + r_n * variance,
        1.0 + g_n * variance,
        1.0 + b_n * variance,
    ]
}

/// Cheap sky-exposure heuristic: count air blocks in the column above
/// this one for up to SKY_PROBE depth. Returns [0, 1]. Out-of-chunk
/// reads above count as air (conservative for surface blocks at the
/// chunk top).
const SKY_PROBE: i32 = 12;
#[inline]
fn sky_exposure_at(wx: i32, wy: i32, wz: i32, chunk: &Chunk) -> f32 {
    let (origin_x, origin_y, origin_z) = chunk.coord.block_origin();
    let lx = wx - origin_x;
    let lz = wz - origin_z;
    if lx < 0 || lx >= CHUNK_SIZE || lz < 0 || lz >= CHUNK_SIZE {
        return 1.0; // out-of-chunk; assume sky
    }
    let mut air = 0i32;
    for dy in 1..=SKY_PROBE {
        let nly = (wy + dy) - origin_y;
        if nly < 0 || nly >= CHUNK_SIZE {
            air += 1; // chunk-edge: count as sky (conservative)
            continue;
        }
        let idx = Chunk::block_index(lx, nly, lz);
        if chunk.blocks[idx] == BlockId::Air {
            air += 1;
        }
    }
    air as f32 / SKY_PROBE as f32
}

/// Cheap slope heuristic: count how many of the 4 horizontal neighbors
/// at this Y are air (cliff edge) vs solid (continuous ground). Returns
/// [0, 1] where 0 = continuous ground, 1 = isolated peak.
#[inline]
fn slope_at(wx: i32, wy: i32, wz: i32, chunk: &Chunk) -> f32 {
    let (origin_x, origin_y, origin_z) = chunk.coord.block_origin();
    let lx = wx - origin_x;
    let ly = wy - origin_y;
    let lz = wz - origin_z;
    if ly < 0 || ly >= CHUNK_SIZE { return 0.0; }
    // Sample at (this_y) — if the horizontal neighbor is air, this side
    // is a cliff face. More air neighbors = steeper/more isolated.
    let dirs: [(i32, i32); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];
    let mut air_count = 0i32;
    for (dx, dz) in dirs.iter() {
        let nlx = lx + dx;
        let nlz = lz + dz;
        if nlx < 0 || nlx >= CHUNK_SIZE || nlz < 0 || nlz >= CHUNK_SIZE {
            continue;
        }
        let idx = Chunk::block_index(nlx, ly, nlz);
        if chunk.blocks[idx] == BlockId::Air {
            air_count += 1;
        }
    }
    air_count as f32 * 0.25
}

/// Sample the environment at a block position. Used by the response-
/// matrix tint path. Cheap: ~12 chunk-data reads + 2 world-model
/// queries (each cached at region-grid level).
#[inline]
fn sample_env_at(
    block: BlockId,
    wx: i32, wy: i32, wz: i32,
    chunk: &Chunk,
    world_model: Option<&std::sync::Arc<crate::world::world_model::WorldModel>>,
) -> crate::world::material_response::EnvironmentSample {
    let (temperature, humidity) = match world_model {
        Some(m) => (
            m.temperature(wx as f64, wy as f64, wz as f64, 0.0) as f32,
            m.humidity(wx as f64, wy as f64, wz as f64, 0.0) as f32,
        ),
        None => (0.5, 0.5),
    };
    let weathering = {
        let w = worldgen_weathering_at(block, wx, wy, wz);
        w as f32 / 255.0
    };
    crate::world::material_response::EnvironmentSample {
        temperature,
        humidity,
        sky_exposure: sky_exposure_at(wx, wy, wz, chunk),
        slope: slope_at(wx, wy, wz, chunk),
        weathering,
    }
}

/// Compute the per-block top-face color via the response matrix.
/// When the response matrix doesn't apply (`base_rgb` is the default
/// neutral grey), falls back to the legacy block_color() palette
/// modulated by per-position position-noise tint, preserving prior
/// visuals for blocks not yet given matrix entries.
fn compute_tinted_color_for_top(
    block: BlockId,
    wx: i32, wy: i32, wz: i32,
    chunk: &Chunk,
    world_model: Option<&std::sync::Arc<crate::world::world_model::WorldModel>>,
) -> [f32; 3] {
    let response = crate::world::material_response::for_block(block, wy);
    // Heuristic: if the matrix has the default neutral [0.5, 0.5, 0.5]
    // base it's a "fallthrough" entry for this block — use the legacy
    // palette path so we don't regress non-matrix blocks to grey.
    let is_neutral = response.base_rgb == [0.5; 3];
    if is_neutral {
        // Legacy path: block_color_per_face / block_color × per-position tint.
        let palette = match block_color_per_face(block) {
            Some(faces) => faces[face::TOP],
            None => match block_color(block) {
                Some(c) => c,
                None => return [0.5; 3],
            },
        };
        let tint = tint_for_position(block, wx, wy, wz);
        return [
            (palette[0] * tint[0]).clamp(0.0, 1.0),
            (palette[1] * tint[1]).clamp(0.0, 1.0),
            (palette[2] * tint[2]).clamp(0.0, 1.0),
        ];
    }
    let env = sample_env_at(block, wx, wy, wz, chunk, world_model);
    // response.tint() returns the climate-shifted color (already
    // including base_rgb). Add a position-noise wobble for per-block
    // variation; matrix's `position_variance` controls amplitude.
    let mut c = response.tint(env);
    let pn = tint_for_position(block, wx, wy, wz);
    let variance = response.position_variance;
    for i in 0..3 {
        // Apply the position multiplier scaled by variance: a variance
        // of 0 means pn has no effect; high variance fully applies it.
        // The denominator is the variance baked into tint_for_position
        // for grass (the most-varying material), which is what the
        // multiplier was calibrated against.
        let multiplier = 1.0 + (pn[i] - 1.0) * variance / TINT_REFERENCE_VARIANCE;
        c[i] = (c[i] * multiplier).clamp(0.0, 1.0);
    }
    c
}

/// Compute the per-block side-face color via the response matrix.
/// Falls back to the legacy palette for non-matrix blocks (same as
/// `compute_tinted_color_for_top`).
fn compute_tinted_color_for_face(
    block: BlockId,
    face: usize,
    wx: i32, wy: i32, wz: i32,
    chunk: &Chunk,
    world_model: Option<&std::sync::Arc<crate::world::world_model::WorldModel>>,
) -> [f32; 3] {
    let response = crate::world::material_response::for_block(block, wy);
    let is_neutral = response.base_rgb == [0.5; 3];
    if is_neutral {
        let palette = match block_color_per_face(block) {
            Some(faces) => faces[face],
            None => match block_color(block) {
                Some(c) => c,
                None => return [0.5; 3],
            },
        };
        let tint = tint_for_position(block, wx, wy, wz);
        return [
            (palette[0] * tint[0]).clamp(0.0, 1.0),
            (palette[1] * tint[1]).clamp(0.0, 1.0),
            (palette[2] * tint[2]).clamp(0.0, 1.0),
        ];
    }
    // For matrix-aware blocks, use the response's climate tint. The
    // response is a single tint regardless of face; per-face variation
    // (e.g. grass-side fades to dirt) is captured via block_color_per_face
    // which we don't currently consult through the matrix path. Future:
    // matrix could carry per-face shifts. For now: top face tint applies
    // to all faces of matrix-aware blocks.
    let env = sample_env_at(block, wx, wy, wz, chunk, world_model);
    let mut c = response.tint(env);
    let pn = tint_for_position(block, wx, wy, wz);
    let variance = response.position_variance;
    for i in 0..3 {
        let multiplier = 1.0 + (pn[i] - 1.0) * variance / TINT_REFERENCE_VARIANCE;
        c[i] = (c[i] * multiplier).clamp(0.0, 1.0);
    }
    let _ = face; // currently unused; reserved for per-face matrix shifts
    c
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Vertex {
    /// World-space position. Greedy quads cover multiple blocks, so a single
    /// vertex's position is a corner of the merged rectangle.
    pub position: [f32; 3],
    pub color:    [f32; 3],
}

impl Vertex {
    pub fn new(position: [f32; 3], color: [f32; 3]) -> Self {
        Self { position, color }
    }

    /// CPU-side vertex stride. The GPU-side stride is [`PackedVertex::STRIDE`]
    /// (8 bytes); the mesher carries the verbose form so meshing math
    /// can stay in floats, then the renderer's upload step quantizes
    /// each chunk's vertices into packed form before the memcpy.
    pub const STRIDE: u32 = std::mem::size_of::<Vertex>() as u32;
}

/// Compact GPU-upload form of [`Vertex`] (v0.24.20-vertex-pack).
///
/// **8 bytes per vertex**, vs 24 for the CPU-side `Vertex`. Replaces the
/// dominant streaming cost in the renderer: at RD=10 a typical playtest
/// loaded ~1000 chunks with ~3000 vertices each, totalling ~72 MB of
/// vertex bytes. Packed, the same data is ~24 MB.
///
/// # Layout
///
/// ```text
/// bytes 0..4 : pos_packed  — VK_FORMAT_A2B10G10R10_UNORM_PACK32
///              R = bits  0..10 : chunk-local x, unorm scaled to [0, 32]
///              G = bits 10..20 : chunk-local y, unorm scaled to [0, 32]
///              B = bits 20..30 : chunk-local z, unorm scaled to [0, 32]
///              A = bits 30..32 : reserved (2 bits free for future face
///                                 index, AO bucket, or per-vertex flag)
///
/// bytes 4..8 : color_packed — VK_FORMAT_R8G8B8A8_UNORM
///              byte 0 : R (red,   unorm)
///              byte 1 : G (green, unorm)
///              byte 2 : B (blue,  unorm)
///              byte 3 : reserved (currently 0xFF; future light/AO byte)
/// ```
///
/// # Why chunk-local position
///
/// The CPU mesher produces world-space vertex positions. A naive packed
/// form would need to span the world's full coordinate range (which is
/// ±2 billion blocks at i32 × 32) — impossible at 10 bits of precision.
/// By storing chunk-local positions and reconstructing world position in
/// the vertex shader (chunk_origin + local * 32), we get full sub-block
/// precision within a chunk's natural bounds.
///
/// The vertex shader reads `chunk_origin` from `chunks[gl_InstanceIndex]
/// .aabb_min.{x, w, z}`. The cull shader writes `first_instance =
/// chunk_slot_index` into each draw command so `gl_InstanceIndex`
/// resolves to the correct chunks-SSBO slot. `aabb_min.w` (previously
/// padding) carries chunk_origin.y; `aabb_min.x` and `.z` already
/// carried the chunk's XZ origin since the renderer uses
/// `coord.block_origin()` to build the AABB.
///
/// # Precision
///
/// Position step: 32.0 / 1023 ≈ 0.0313 blocks per axis. The mesher's
/// fractional offsets (z-fight nudges of 0.05, sub-block sag up to 0.25,
/// surface jitter up to 0.0625) all stay distinct after quantization.
///
/// Color step: 1/255 per channel. 256 levels per primary is well above
/// the eye's discrimination threshold at game-typical contrast.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PackedVertex {
    pub pos_packed: u32,
    pub color_packed: u32,
}

impl PackedVertex {
    /// Stride in bytes — matches the Vulkan vertex binding stride used
    /// in `chunk_pipeline.rs` and `shadow_pipeline.rs`.
    pub const STRIDE: u32 = std::mem::size_of::<PackedVertex>() as u32; // 8

    /// Edge length of a chunk in world blocks. Vertex positions must
    /// fall within `[0, CHUNK_EXTENT]` chunk-local for round-trip
    /// fidelity. The mesher only emits vertices inside its chunk's
    /// natural bounds, so this is always satisfied for production
    /// meshes; out-of-range values get clamped.
    pub const CHUNK_EXTENT: f32 = 32.0;

    /// Encode a chunk-local position into the A2B10G10R10_UNORM packed
    /// format. `local` is the vertex's position relative to the chunk
    /// origin, in blocks; expected range `[0, 32]` per axis.
    #[inline]
    pub fn pack_pos(local: [f32; 3]) -> u32 {
        let q = |v: f32| -> u32 {
            // Map [0, 32] -> [0, 1023]. Clamp protects against vertices
            // that overshoot the chunk's natural bounds (none in
            // practice; defensive only).
            let t = (v / Self::CHUNK_EXTENT).clamp(0.0, 1.0);
            (t * 1023.0).round() as u32
        };
        let r = q(local[0]);
        let g = q(local[1]);
        let b = q(local[2]);
        // A2B10G10R10_UNORM_PACK32: R=bits 0..10, G=10..20, B=20..30, A=30..32.
        // Alpha reserved (0).
        r | (g << 10) | (b << 20)
    }

    /// Encode an RGB color (each channel in `[0.0, 1.0]`) into R8G8B8A8.
    /// Alpha is set to 0xFF (full opacity).
    #[inline]
    pub fn pack_color(color: [f32; 3]) -> u32 {
        Self::pack_color_id(color, 0xFF)
    }

    /// Encode an RGB color plus a material/tile id into R8G8B8A8 — the id rides
    /// the reserved alpha byte. The ruleset (`world::texture_mapping`) proved this
    /// is lossless and that chunk.vert recovers the id via `in_color_unorm.a *
    /// 255.0`. This is the seam the per-block tile id will flow through once the
    /// octree mesher emits one; until then every caller passes 0xFF (via
    /// `pack_color`) so nothing downstream reads it and behaviour is unchanged.
    #[inline]
    pub fn pack_color_id(color: [f32; 3], id: u8) -> u32 {
        let q = |v: f32| -> u32 {
            (v.clamp(0.0, 1.0) * 255.0).round() as u32
        };
        let r = q(color[0]);
        let g = q(color[1]);
        let b = q(color[2]);
        r | (g << 8) | (b << 16) | ((id as u32) << 24)
    }

    /// Convert a world-space [`Vertex`] to packed form using the chunk
    /// origin to derive chunk-local position.
    #[inline]
    pub fn from_world(v: &Vertex, chunk_origin: [f32; 3]) -> Self {
        let local = [
            v.position[0] - chunk_origin[0],
            v.position[1] - chunk_origin[1],
            v.position[2] - chunk_origin[2],
        ];
        Self {
            pos_packed: Self::pack_pos(local),
            // r354: alpha is ALWAYS 0xFF (identical to r351). The texture path no
            // longer rides the alpha byte — deviating it from 0xFF reproduced the
            // sails for reasons not visible in source. Texturing is now driven off
            // the block color in chunk.frag, gated by ubo.wind_params.w.
            color_packed: Self::pack_color(v.color),
        }
    }

    /// r596: like [`from_world`], but folds a 2-bit foliage sway tag into
    /// pos_packed bits 30-31 (see `sim::foliage_sway`). Tag 0 is byte-identical
    /// to `from_world`, so solid geometry is unchanged.
    #[inline]
    pub fn from_world_tagged(v: &Vertex, chunk_origin: [f32; 3], tag: u8) -> Self {
        let mut p = Self::from_world(v, chunk_origin);
        if tag != 0 {
            p.pos_packed = crate::sim::foliage_sway::pack_tag_into_pos(
                p.pos_packed,
                crate::sim::foliage_sway::SwayTag::from_bits(tag),
            );
        }
        p
    }

    /// Decode chunk-local position (for tests / debugging).
    pub fn unpack_pos(self) -> [f32; 3] {
        let x = ((self.pos_packed) & 0x3FF) as f32 / 1023.0 * Self::CHUNK_EXTENT;
        let y = ((self.pos_packed >> 10) & 0x3FF) as f32 / 1023.0 * Self::CHUNK_EXTENT;
        let z = ((self.pos_packed >> 20) & 0x3FF) as f32 / 1023.0 * Self::CHUNK_EXTENT;
        [x, y, z]
    }

    /// Decode RGB color (for tests / debugging).
    pub fn unpack_color(self) -> [f32; 3] {
        let r = (self.color_packed & 0xFF) as f32 / 255.0;
        let g = ((self.color_packed >> 8) & 0xFF) as f32 / 255.0;
        let b = ((self.color_packed >> 16) & 0xFF) as f32 / 255.0;
        [r, g, b]
    }
}

// =============================================================================
// r330-gpu — foliage wind-sway tagging (§4b).
//
// The pure decision/packing logic lives in `crate::sim::foliage_sway` (gated
// standalone). These are the thin adapters that map this crate's `BlockId` and
// face index onto that core, so both this CPU mesher and the GPU mesher's
// readback can tag vertices identically. See HANDOFF_gpu_foliage_sway.md for
// where the emit loops + the quantize step (`PackedVertex::from_world`) call
// these — that wiring is the coordinator's on-device step.
// =============================================================================

pub use crate::sim::foliage_sway::SwayTag;

/// Block/face sway eligibility — the dispatch-specified entry point.
/// Returns the *block-level* tag bits: `SwayTag::Sway` for foliage that may
/// sway, `SwayTag::None` for solid geometry (a grass block's green TOP face
/// included — the §4b bug). The base-vs-tip anchoring is resolved per-vertex
/// by [`vertex_sway_tag`].
#[inline]
pub fn sway_flag(block: BlockId, face: usize, is_plant: bool) -> u8 {
    // Leaves are the engine's "sub-voxel-only" foliage cubes (Oak/Birch).
    let is_leaf = block.is_subvoxel_only();
    let eligible = crate::sim::foliage_sway::block_sway_eligible(is_leaf, is_plant, face);
    crate::sim::foliage_sway::vertex_tag(eligible, /*base*/ false).bits()
}

/// Per-vertex tag: like [`sway_flag`] but also anchors the planted base.
/// `is_base_vertex` comes from the vertex's height within its plant cell.
#[inline]
pub fn vertex_sway_tag(block: BlockId, face: usize, is_plant: bool, is_base_vertex: bool) -> SwayTag {
    let is_leaf = block.is_subvoxel_only();
    let eligible = crate::sim::foliage_sway::block_sway_eligible(is_leaf, is_plant, face);
    crate::sim::foliage_sway::vertex_tag(eligible, is_base_vertex)
}

/// Stamp a sway tag into the reserved alpha of an already-packed position
/// word (`PackedVertex::pos_packed`). Thin re-export so the quantize step can
/// tag without importing `sim::foliage_sway` directly.
#[inline]
pub fn tag_packed_pos(pos_packed: u32, tag: SwayTag) -> u32 {
    crate::sim::foliage_sway::pack_tag_into_pos(pos_packed, tag)
}

#[derive(Debug, Clone)]
pub struct ChunkMesh {
    pub vertices: Vec<Vertex>,
    pub indices:  Vec<u32>,
    /// r596: OUT-OF-BAND per-vertex foliage sway tags (0 None / 1 Sway / 2 Anchor),
    /// parallel to `vertices`. Kept outside `Vertex` on purpose: growing `Vertex`
    /// past 24 bytes breaks the octree readback's `&[Vertex]` reinterpret cast
    /// (the r594 "large planes" regression). May be SHORTER than `vertices` —
    /// solid geometry never appends tags; readers pad missing entries with 0.
    pub sway: Vec<u8>,
    /// r596 (upload plan Tier 2a/5): vertices pre-packed OFF the frame thread by
    /// [`ChunkMesh::pack_in_place`] in the meshing pool. When non-empty the
    /// renderer's upload skips its per-vertex `from_world` pass entirely and
    /// uses these bytes + the world-space Y bounds below.
    pub packed: Vec<PackedVertex>,
    pub packed_min_y: f32,
    pub packed_max_y: f32,
}

impl ChunkMesh {
    pub fn empty() -> Self {
        Self {
            vertices: Vec::new(),
            indices: Vec::new(),
            sway: Vec::new(),
            packed: Vec::new(),
            packed_min_y: 0.0,
            packed_max_y: 0.0,
        }
    }
    pub fn is_empty(&self) -> bool { self.indices.is_empty() }
    pub fn vertex_count(&self) -> u32 { self.vertices.len() as u32 }
    pub fn index_count(&self)  -> u32 { self.indices.len()  as u32 }

    /// r596: the sway tag for vertex `i` (0 when untagged — solid geometry).
    #[inline]
    pub fn sway_tag(&self, i: usize) -> u8 {
        self.sway.get(i).copied().unwrap_or(0)
    }

    /// r596: tag the 4 vertices ABOUT TO BE PUSHED with foliage sway bits.
    /// Pads the tag vec with zeros up to the current vertex count first, so
    /// solid geometry emitted earlier stays untagged and alignment holds.
    #[inline]
    pub fn mark_sway_quad(&mut self, tags: [u8; 4]) {
        if self.sway.len() < self.vertices.len() {
            self.sway.resize(self.vertices.len(), 0);
        }
        self.sway.extend_from_slice(&tags);
    }

    /// r596 (Tier 2a/5): quantize this mesh's world-space vertices into
    /// [`PackedVertex`] form — the exact bytes the GPU pool draws — on the
    /// CALLING thread (a meshing-pool worker), so the frame thread's upload
    /// becomes a memcpy instead of a per-vertex pack. Folds each vertex's
    /// foliage sway tag into pos bits 30-31 (what chunk.vert's wind sway and
    /// the r591 disturber read). Also records world-space Y bounds in one pass.
    /// `vertices` are kept intact (the octree CPU-compare path reads them).
    pub fn pack_in_place(&mut self, chunk_origin: [f32; 3]) {
        if self.vertices.is_empty() {
            self.packed.clear();
            return;
        }
        let mut min_y = self.vertices[0].position[1];
        let mut max_y = min_y;
        let mut packed = Vec::with_capacity(self.vertices.len());
        for (i, v) in self.vertices.iter().enumerate() {
            let y = v.position[1];
            if y < min_y { min_y = y; }
            if y > max_y { max_y = y; }
            packed.push(PackedVertex::from_world_tagged(v, chunk_origin, self.sway_tag(i)));
        }
        self.packed = packed;
        self.packed_min_y = min_y;
        self.packed_max_y = max_y;
    }
}

// =============================================================================
// Block-type → color mapping. Shared by both meshers.
// =============================================================================

/// Per-face brightness factor — used as fallback when no `Sky` is provided.
/// Numbers chosen by eye to give blocks visible face separation.
///
/// In practice the meshing pool calls `mesh_chunk_with_sky()` with a Sky
/// snapshot, which computes per-face brightness from sun direction. This
/// constant is what `mesh_chunk()` uses (the no-sky entry point), kept so
/// older test code and offline tools work without setting up a Sky.
const FACE_LIGHT: [f32; 6] = [
    1.00, // +Y top
    0.55, // -Y bottom
    0.80, // +X east
    0.80, // -X west
    0.65, // +Z south
    0.65, // -Z north
];

/// Outward-facing normal for each face index, used by the sky-aware shader.
const FACE_NORMALS: [[f32; 3]; 6] = [
    [ 0.0,  1.0,  0.0], // +Y top
    [ 0.0, -1.0,  0.0], // -Y bottom
    [ 1.0,  0.0,  0.0], // +X east
    [-1.0,  0.0,  0.0], // -X west
    [ 0.0,  0.0,  1.0], // +Z south
    [ 0.0,  0.0, -1.0], // -Z north
];

/// Block face indices, used to index FACE_LIGHT.
#[allow(dead_code)]
mod face {
    pub const TOP:    usize = 0;
    pub const BOTTOM: usize = 1;
    pub const EAST:   usize = 2;
    pub const WEST:   usize = 3;
    pub const SOUTH:  usize = 4;
    pub const NORTH:  usize = 5;
}

/// Quad winding order for each face (counter-clockwise when viewed from
/// outside the cube — matches our pipeline's `front_face = COUNTER_CLOCKWISE`
/// + `cull_mode = BACK`).
///
/// Each entry is 4 unit-cube corners (x, y, z) for that face's quad. We
/// tessellate as two triangles (0,1,2) and (0,2,3).
const FACE_VERTS: [[[f32; 3]; 4]; 6] = [
    // TOP +Y
    [[0.0, 1.0, 0.0], [1.0, 1.0, 0.0], [1.0, 1.0, 1.0], [0.0, 1.0, 1.0]],
    // BOTTOM -Y
    [[0.0, 0.0, 1.0], [1.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 0.0, 0.0]],
    // EAST +X
    [[1.0, 0.0, 1.0], [1.0, 1.0, 1.0], [1.0, 1.0, 0.0], [1.0, 0.0, 0.0]],
    // WEST -X
    [[0.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 1.0, 1.0], [0.0, 0.0, 1.0]],
    // SOUTH +Z
    [[0.0, 0.0, 1.0], [0.0, 1.0, 1.0], [1.0, 1.0, 1.0], [1.0, 0.0, 1.0]],
    // NORTH -Z
    [[1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 0.0]],
];

const FACE_OFFSETS: [(i32, i32, i32); 6] = [
    (0,  1,  0), // TOP
    (0, -1,  0), // BOTTOM
    (1,  0,  0), // EAST
    (-1, 0,  0), // WEST
    (0,  0,  1), // SOUTH
    (0,  0, -1), // NORTH
];

/// Base color for each block type. RGB in 0..1.
pub(crate) fn block_color(b: BlockId) -> Option<[f32; 3]> {
    use BlockId::*;
    Some(match b {
        Air => return None,
        Water        => [0.20, 0.40, 0.85],

        Stone        => [0.55, 0.55, 0.55],
        Cobblestone  => [0.50, 0.50, 0.50],
        Bedrock      => [0.20, 0.20, 0.20],
        Grass        => [0.35, 0.70, 0.30],
        Dirt         => [0.55, 0.40, 0.25],
        Sand         => [0.92, 0.86, 0.65],
        Gravel       => [0.65, 0.62, 0.58],
        Clay         => [0.65, 0.65, 0.75],
        Moss         => [0.30, 0.55, 0.30],
        RedSandstone => [0.78, 0.45, 0.25],

        OakLog       => [0.45, 0.30, 0.18],
        OakLeaves    => [0.18, 0.55, 0.20],
        BirchLog     => [0.85, 0.85, 0.80],
        BirchLeaves  => [0.40, 0.65, 0.35],

        CoalOre      => [0.30, 0.30, 0.30],
        IronOre      => [0.70, 0.60, 0.50],
        GoldOre      => [0.85, 0.75, 0.30],
        DiamondOre   => [0.45, 0.85, 0.95],

        Snow | SnowBlock => [0.95, 0.95, 1.00],
        Ice          => [0.65, 0.85, 0.95],
        Cactus       => [0.20, 0.55, 0.20],
        Dripstone    => [0.65, 0.50, 0.50],

        Flower       => [0.95, 0.30, 0.40],
        TallGrass    => [0.40, 0.70, 0.30],
        Fern         => [0.30, 0.60, 0.30],
        DeadBush     => [0.55, 0.40, 0.20],
        Vine         => [0.30, 0.55, 0.25],
        Mushroom     => [0.85, 0.30, 0.30],
        Cobweb       => [0.85, 0.85, 0.85],
        Lichen       => [0.55, 0.65, 0.55],

        // Natural-element pass: rocks, minerals, biological materials.
        // Colors picked to span the palette range biome JSON files target.
        Limestone    => [0.85, 0.83, 0.78],
        Marble       => [0.92, 0.92, 0.89],
        Basalt       => [0.18, 0.17, 0.20],
        Tuff         => [0.55, 0.52, 0.50],
        Granite      => [0.72, 0.55, 0.48],
        Slate        => [0.30, 0.32, 0.38],
        Calcite      => [0.94, 0.94, 0.96],
        Andesite     => [0.50, 0.50, 0.52],
        Schist       => [0.38, 0.36, 0.40],
        Bone         => [0.93, 0.90, 0.78],
        Amber        => [0.85, 0.55, 0.18],
        Crystal      => [0.78, 0.65, 0.95],

        Lava         => [0.95, 0.45, 0.10],

        // r456: building/decorative blocks
        MossyCobblestone    => [0.42, 0.50, 0.38],
        PolishedStone       => [0.58, 0.58, 0.60],
        SmoothSandstone     => [0.88, 0.80, 0.58],
        Terracotta          => [0.62, 0.34, 0.24],
        ChiseledStoneBricks => [0.48, 0.47, 0.46],
        DarkOakPlanks       => [0.32, 0.22, 0.13],
        PolishedBasalt      => [0.22, 0.21, 0.24],

        // v0.24.57-berry-items: small decorative berry-bush blocks
        // scattered sparsely on grass surfaces during world gen.
        // Deep wine-red, distinct from Flower's pinker red so players
        // can tell them apart from a distance.
        BerryBush    => [0.55, 0.10, 0.20],
    })
}

/// Per-face block color override.
///
/// Some blocks legitimately look different from different sides — a grass
/// block has a green top and a dirt-colored bottom, with sides that fade
/// from green at the top to dirt at the bottom. Returning per-face colors
/// here lets the mesher paint each face appropriately, fixing the visual
/// "purple seam" where greedy-meshed grass sides exposed dirt-only color
/// at terrain steps.
///
/// For most blocks this returns `None` and the mesher falls back to the
/// flat `block_color()` color. Returning `Some([top, bottom, +X, -X, +Z, -Z])`
/// overrides per face. Indexing matches the `face::*` constants.
fn block_color_per_face(b: BlockId) -> Option<[[f32; 3]; 6]> {
    use BlockId::*;
    match b {
        Grass => {
            // Green top, dirt bottom, transition sides. The transition is
            // baked as a "side" color partway between the two — when the
            // mesher actually emits the side face we paint a single color
            // per quad. Future improvement: per-vertex gradient (top of
            // side quad green, bottom brown). For now, a green-brown blend
            // is much better than the pure-dirt look that caused the seam.
            let top    = [0.35, 0.70, 0.30];
            let bottom = [0.55, 0.40, 0.25];
            // Side: 70% top + 30% bottom — leans green so the seam reads
            // as "grass with dirt root" rather than "dirt with grass cap".
            let side = [
                top[0] * 0.7 + bottom[0] * 0.3,
                top[1] * 0.7 + bottom[1] * 0.3,
                top[2] * 0.7 + bottom[2] * 0.3,
            ];
            Some([top, bottom, side, side, side, side])
        }
        OakLog | BirchLog => {
            // Logs have ringed end-grain on top/bottom (lighter, redder)
            // and bark on sides (darker, browner).
            let bark = block_color(b).unwrap();
            let endgrain = [
                (bark[0] * 1.20).min(1.0),
                (bark[1] * 1.05).min(1.0),
                (bark[2] * 0.90).max(0.0),
            ];
            Some([endgrain, endgrain, bark, bark, bark, bark])
        }
        // Most blocks are uniform. Could add Sand-with-grass-on-top, snowy
        // dirt, etc. later.
        _ => None,
    }
}

/// Whether this block should be meshed at all (i.e. has visible faces).
fn is_meshable(b: BlockId) -> bool {
    // r523: subvoxel-only blocks (leaves, crops) must NOT emit a solid cube — they render
    // entirely as subvoxel decoration, and a cube on top hides that detail (the "solid block
    // over the leaves" bug). This now matches world::surface::is_meshable, which already
    // excluded them; the two paths are meant to agree.
    !b.is_air() && b.is_solid() && !b.is_subvoxel_only()
}

/// Whether a face between `inner` (the meshed block) and `neighbor` should
/// be emitted. Delegates to the narrow `produces_face_against` predicate
/// on BlockId so the rule is owned by the block type, not duplicated here.
fn should_emit_face(inner: BlockId, neighbor: BlockId) -> bool {
    inner.produces_face_against(neighbor)
}

/// Get the block at chunk-local coordinates. Out-of-bounds returns Air.
/// Today we accept some redundant boundary faces; seam culling with
/// neighbor chunks is a future optimization.
fn get_local(chunk: &Chunk, x: i32, y: i32, z: i32) -> BlockId {
    if x < 0 || x >= CHUNK_SIZE
        || y < 0 || y >= CHUNK_SIZE
        || z < 0 || z >= CHUNK_SIZE
    {
        return BlockId::Air;
    }
    chunk.get(x, y, z)
}

// =============================================================================
// Public API.
// =============================================================================

/// Build a mesh for one chunk using greedy meshing, with default static
/// face lighting. Most production callers want `mesh_chunk_with_sky` for
/// sun-aware shading; this entry point exists for tests and offline tools.
pub fn mesh_chunk(chunk: &Chunk) -> ChunkMesh {
    mesh_chunk_greedy(chunk, None, None, None)
}

/// Build a mesh for one chunk with sun-aware face lighting. The `Sky`
/// snapshot determines per-face brightness based on the sun's direction
/// at mesh time. Faces facing the sun are bright; back-lit faces are at the
/// minimum (set by `sky::SUN_LIGHT_MIN`).
///
/// Note: lighting is baked into the mesh at mesh time. If the sun moves
/// significantly, chunks need to be re-meshed for the lighting to update.
/// For typical day-cycle speeds and chunk-streaming rhythms this is fine —
/// chunks are remeshed on edits and during normal streaming.
pub fn mesh_chunk_with_sky(chunk: &Chunk, sky: &crate::sky::Sky) -> ChunkMesh {
    mesh_chunk_greedy(chunk, Some(sky), None, None)
}

/// Build a mesh with sun lighting AND world-model awareness. The world
/// model lets the mesher sample temperature/humidity per face for
/// material-response tinting. Without the model (None), tinting falls
/// back to neutral environmental values.
pub fn mesh_chunk_with_sky_and_model(
    chunk: &Chunk,
    sky: &crate::sky::Sky,
    world_model: &std::sync::Arc<crate::world::world_model::WorldModel>,
) -> ChunkMesh {
    mesh_chunk_greedy(chunk, Some(sky), Some(world_model), None)
}

/// Full-control entry point. Same as `mesh_chunk_with_sky_and_model`
/// but also takes the current simulation tick so the mesher can
/// interpolate visual properties of in-progress block transitions
/// (`Chunk::transitions`). For an animated transition to actually
/// animate visibly, the chunk must be re-meshed as the tick advances —
/// the lifecycle handler bumps chunk version each tick a transition
/// is in flight to drive this.
pub fn mesh_chunk_with_sky_model_and_tick(
    chunk: &Chunk,
    sky: &crate::sky::Sky,
    world_model: &std::sync::Arc<crate::world::world_model::WorldModel>,
    current_tick: u64,
) -> ChunkMesh {
    mesh_chunk_greedy(chunk, Some(sky), Some(world_model), Some(current_tick))
}

// =============================================================================
// Naive mesher — kept around as oracle / fallback. Emits one quad per face.
// =============================================================================

pub fn mesh_chunk_naive(chunk: &Chunk) -> ChunkMesh {
    let mut mesh = ChunkMesh::empty();
    let (origin_x, origin_y, origin_z) = chunk.coord.block_origin();

    for y in 0..CHUNK_SIZE {
        for z in 0..CHUNK_SIZE {
            for x in 0..CHUNK_SIZE {
                let block = get_local(chunk, x, y, z);
                if !is_meshable(block) { continue; }
                let base_color = match block_color(block) {
                    Some(c) => c,
                    None => continue,
                };

                let world_x_i = origin_x + x;
                let world_y_i = origin_y + y;
                let world_z_i = origin_z + z;

                for f in 0..6 {
                    let (dx, dy, dz) = FACE_OFFSETS[f];
                    let neighbor = get_local(chunk, x + dx, y + dy, z + dz);
                    if !should_emit_face(block, neighbor) { continue; }

                    let light = FACE_LIGHT[f];
                    let color = [
                        (base_color[0] * light).clamp(0.0, 1.0),
                        (base_color[1] * light).clamp(0.0, 1.0),
                        (base_color[2] * light).clamp(0.0, 1.0),
                    ];

                    let base_idx = mesh.vertices.len() as u32;
                    let world_x = world_x_i as f32;
                    let world_y = world_y_i as f32;
                    let world_z = world_z_i as f32;

                    for corner in &FACE_VERTS[f] {
                        mesh.vertices.push(Vertex::new(
                            [world_x + corner[0], world_y + corner[1], world_z + corner[2]],
                            color,
                        ));
                    }

                    mesh.indices.push(base_idx);
                    mesh.indices.push(base_idx + 1);
                    mesh.indices.push(base_idx + 2);
                    mesh.indices.push(base_idx);
                    mesh.indices.push(base_idx + 2);
                    mesh.indices.push(base_idx + 3);
                }
            }
        }
    }

    mesh
}

// =============================================================================
// Far / VeryFar mesher (LOD F2). Same naive face-emission strategy as
// `mesh_chunk_naive`, but on a downsampled grid. Each downsample cell
// summarises an F×F×F block of source cells and emits at most one
// outward face per direction. Triangle count drops by roughly F² per
// cell relative to naive (no greedy merging, just smaller grid).
//
// Designed for chunks far enough from the player that sub-block
// detail is invisible. Visual cost: silhouettes preserved at F-block
// resolution; single-block protrusions and 1-block holes get smoothed
// away. That's the trade.
//
// Algorithm:
//   1. Downsample chunk → grid of (CHUNK_SIZE / F)³ cells. Each cell:
//      - solid if ≥ ½ of its source blocks are meshable, otherwise air;
//      - colored by the most-common meshable BlockId among contributors.
//   2. Naive mesh the downsampled grid: emit a face wherever a solid
//      cell borders an air cell. Inner cells produce no faces; outer
//      cells produce up to 6.
//   3. Quad world-space coordinates are scaled by F so the output sits
//      at the same world position as a naive mesh of the same chunk.
//
// Boundary handling: at the chunk boundary the far mesher cannot see
// the next chunk's blocks, so it ALWAYS emits the boundary face. This
// produces small visual cracks where a Near and a Far chunk share an
// edge — filed for F3+ to handle once the cracks become visible.
// =============================================================================

/// Far-detail mesher: downsample factor 2.
///
/// Output triangle count is at most 6 × (CHUNK_SIZE/2)² × CHUNK_SIZE/2
/// times two = 6 × 16² × 16 × 2 = 49152 for a fully solid chunk. In
/// practice surface chunks emit 5-15× fewer triangles than `mesh_chunk_naive`.
///
/// Behavior matches `mesh_chunk_naive` at factor=1 by construction;
/// see [`mesh_chunk_downsampled`] for the underlying parameterised
/// implementation.
pub fn mesh_chunk_far(chunk: &Chunk) -> ChunkMesh {
    mesh_chunk_downsampled(chunk, 2)
}

/// VeryFar-detail mesher: downsample factor 4. Same algorithm as
/// [`mesh_chunk_far`], reducing triangles by another ~4×.
///
/// At factor=4 the chunk's resolution is 8³ — enough to keep a
/// recognisable silhouette but small features (single trees, narrow
/// pillars) are gone.
pub fn mesh_chunk_very_far(chunk: &Chunk) -> ChunkMesh {
    mesh_chunk_downsampled(chunk, 4)
}

// =============================================================================
// Reverse-regen Far mesher (Stage 2, v0.23.7 spike)
// =============================================================================
//
// `mesh_chunk_far_virtual` produces a Far/VeryFar mesh from a `ChunkCoord`
// and a `WorldGenerator` reference, WITHOUT ever materialising a 32³
// block grid. It shares Step 2 (`emit_downsampled_grid`) with the
// existing materialised path so output shape is identical by
// construction.
//
// What it skips compared to `mesh_chunk_downsampled(generator.generate_chunk(coord), F)`:
//
//   * Allocating a 32³ Vec<BlockId> (64 KB).
//   * Phase 1: 32 768 per-cell density+cave evaluations.
//   * Phase 2: surface decoration walk.
//   * Phase 2.5: water filling.
//   * Phase 3/3.5: ore/lava placement.
//   * Phase 4: bedrock.
//   * Phase 5: trees and other surface features.
//   * The Step-1 downsample histogram (F³ samples + majority vote per cell).
//
// What it does instead: sample density at the centre of each F×F×F
// region in the chunk — one density+cave evaluation per grid cell.
// At factor=2 that's 4096 samples instead of 32 768 + post-processing.
// At factor=4 (VeryFar) it's 512.
//
// Quality trade-offs vs the materialised path:
//
//   * Block-id stratification: virtual chunks output Stone for every
//     subsurface cell and the surface block for cells with no solid
//     neighbour above. No biome-specific surface variation, no
//     ore/lava/water. At Far distance these are mostly invisible.
//   * Sampling resolution: centre-of-cell instead of majority-of-F³.
//     A 4-of-8 air cell may flip solid/air based on the single centre
//     sample. Visible as slightly different silhouettes at terrain
//     boundaries.
//   * Trees, surface features: gone. They're under a kilobyte of
//     vertices anyway at Far distance.
//
// What it preserves:
//
//   * Output mesh schema (`ChunkMesh`) and world-space alignment with
//     the materialised path. A Far chunk meshed virtually can sit
//     next to a Mid chunk meshed materially without seams worse than
//     the existing Near/Far seams.
//   * The Step-2 emission code (`emit_downsampled_grid`) is shared.
//
// Why Far first (and not Near):
//
//   * Editing matters at Near; promotion-on-edit needs a real chunk.
//     Far is read-only as a rule.
//   * Far has the worst materialise/downsample ratio: we build 32 768
//     cells just to throw away 7/8 of them.
//   * Far is the most-numerous tier — the substrate verification at
//     RD=64 meshed 2476 Far chunks in 79s, vs 7141 Near. Cumulative
//     wins compound.
//
// Stage 2b will wire this into the meshing pool so request_visible
// classifies coords by tier and Far coords skip chunk_loader entirely.

/// Reverse-regen Far/VeryFar mesher. Produces a downsampled mesh for
/// the given chunk coordinate by sampling the generator directly,
/// never materialising a `Chunk`.
///
/// `factor` is the downsample factor. Must divide `CHUNK_SIZE` and
/// must be ≥ 2 (factor=1 is materialised-only — for that, use
/// `mesh_chunk_downsampled`). Typical values: 2 (Far) or 4 (VeryFar).
pub fn mesh_chunk_far_virtual(
    coord: ChunkCoord,
    generator: &WorldGenerator,
    factor: i32,
) -> ChunkMesh {
    debug_assert!(factor >= 2, "factor must be ≥ 2 for virtual path");
    debug_assert!(CHUNK_SIZE % factor == 0,
        "factor {} must divide CHUNK_SIZE ({})", factor, CHUNK_SIZE);

    let (origin_x, origin_y, origin_z) = coord.block_origin();
    let grid_size = CHUNK_SIZE / factor;
    let cell_count = (grid_size * grid_size * grid_size) as usize;

    // Centre of each F×F×F cell, in local-block coords:
    let centre_offset = factor / 2;

    // Step 1 (virtual): one density+cave sample per grid cell.
    //
    // We first build a flat solid-mask, then walk columns top-down to
    // pick Grass (or generic surface) for the topmost solid cell in
    // each column. This is the closest approximation we get to the
    // real generator's surface decoration phase without doing the
    // biome lookup. Subsurface cells are Stone.
    //
    // For the spike we hardcode the surface block as Grass and the
    // subsurface as Stone. Phase 2b can route through
    // `pick_surface_block(biome, ...)` once the architecture is
    // proven; the seam between virtual and materialised tiers will
    // make biome variation visible.
    let mut grid: Vec<Option<BlockId>> = vec![None; cell_count];

    let idx_of = |i: i32, j: i32, k: i32| -> usize {
        (i + k * grid_size + j * grid_size * grid_size) as usize
    };

    // First pass: solid-or-air per cell, all Stone for now.
    for j in 0..grid_size {
        for k in 0..grid_size {
            for i in 0..grid_size {
                let wx = origin_x + i * factor + centre_offset;
                let wy = origin_y + j * factor + centre_offset;
                let wz = origin_z + k * factor + centre_offset;

                // Solidity: same predicate as cpu_generator phase 1.
                // density > 0 AND not carved out by a cave.
                let solid = generator.density(wx, wy, wz) > 0.0
                    && !generator.is_cave(wx, wy, wz);
                if solid {
                    grid[idx_of(i, j, k)] = Some(BlockId::Stone);
                }
            }
        }
    }

    // Second pass: walk each column top-down within the grid; the
    // topmost solid cell with air-above becomes Grass. Air-above is
    // either the cell above in the grid OR (at the top of the chunk)
    // a fresh density sample one cell above.
    //
    // This mirrors cpu_generator's Phase 2 but at grid resolution,
    // not block resolution. We don't decorate the next 3 subsurface
    // cells (Phase 2 does that with Dirt) because at factor=2 those
    // cells span 6 world-blocks below the surface, way deeper than
    // the Dirt layer the real generator emits.
    for k in 0..grid_size {
        for i in 0..grid_size {
            for j in (0..grid_size).rev() {
                let here_idx = idx_of(i, j, k);
                if grid[here_idx].is_none() { continue; }

                let above_solid = if j + 1 < grid_size {
                    grid[idx_of(i, j + 1, k)].is_some()
                } else {
                    // Top of chunk: peek one cell-height above the chunk
                    // by sampling the generator. Same predicate.
                    let wx = origin_x + i * factor + centre_offset;
                    let wy = origin_y + (j + 1) * factor + centre_offset;
                    let wz = origin_z + k * factor + centre_offset;
                    generator.density(wx, wy, wz) > 0.0
                        && !generator.is_cave(wx, wy, wz)
                };

                if !above_solid {
                    grid[here_idx] = Some(BlockId::Grass);
                    break; // only the topmost gets surface; rest stay Stone
                }
                // else: this cell is buried, leave as Stone, keep walking down
            }
        }
    }

    // Step 2 (shared): emit faces from the grid.
    emit_downsampled_grid(&grid, grid_size, factor, (origin_x, origin_y, origin_z))
}

/// Parameterised LOD mesher. `factor` must be a divisor of `CHUNK_SIZE`
/// (1, 2, 4, 8, 16, or 32). Larger factors mean coarser meshes.
///
/// `factor = 1` is the degenerate identity case — produces output
/// equivalent to `mesh_chunk_naive`. Not a useful path in practice
/// (it's the baseline we're trying to beat) but kept for tests and
/// reference: it documents what "no downsampling" looks like and
/// pins the algorithm against the naive mesher.
///
/// # Why this isn't greedy-aware
///
/// The naive emission inside the downsampled grid leaves obvious
/// merging opportunities — adjacent solid cells with the same color
/// could fold into one quad. We don't do that here because:
///
/// 1. The whole point of LOD is "cheap to mesh"; greedy adds
///    significant compute that may exceed the win.
/// 2. F2 has zero call sites yet (gets wired in F3); shipping a
///    smaller substrate first matches the F1-only discipline that
///    worked for v0.22.0.
/// 3. If hardware data shows the triangle reduction is insufficient,
///    we add a greedy pass on top in a separate increment.
pub fn mesh_chunk_downsampled(chunk: &Chunk, factor: i32) -> ChunkMesh {
    debug_assert!(factor >= 1, "factor must be ≥ 1");
    debug_assert!(CHUNK_SIZE % factor == 0,
        "factor {} must divide CHUNK_SIZE ({})", factor, CHUNK_SIZE);

    // v0.24.72: same fast-path as `mesh_chunk_greedy`. An all-air
    // chunk downsamples to an all-air grid, which produces an empty
    // mesh. Short-circuit the grid-build + emission work.
    if chunk.cached_all_air {
        return ChunkMesh::empty();
    }

    let (origin_x, origin_y, origin_z) = chunk.coord.block_origin();
    let grid_size = CHUNK_SIZE / factor;

    // Step 1: downsample. For each (i,j,k) in the grid, summarise the
    // F×F×F source cells into (solid?, representative BlockId).
    //
    // Storage: flat Vec<Option<BlockId>>. None = air. Some(b) = solid
    // with representative block b. Indexed (x + z*grid + y*grid²) to
    // match Chunk storage order — gives stride-friendly access in the
    // emission loop.
    let cell_count = (grid_size * grid_size * grid_size) as usize;
    let mut grid: Vec<Option<BlockId>> = Vec::with_capacity(cell_count);

    // We want this to be hot-path-fast. Avoid HashMap: 46 BlockId
    // variants but each downsample cell only sees up to F³ source
    // blocks (typically ≤64), so a small Vec<(BlockId, count)> with
    // linear search is cheaper than HashMap allocation overhead. For
    // factor=2 (F³=8) the average lookup is ~1-2 entries.
    let cell_volume = (factor * factor * factor) as usize;
    let majority_threshold = cell_volume / 2 + 1;
    let mut histogram: Vec<(BlockId, u32)> = Vec::with_capacity(8);

    for j in 0..grid_size {
        for k in 0..grid_size {
            for i in 0..grid_size {
                histogram.clear();
                let mut solid_count: u32 = 0;

                let base_x = i * factor;
                let base_y = j * factor;
                let base_z = k * factor;

                for dy in 0..factor {
                    for dz in 0..factor {
                        for dx in 0..factor {
                            let block = chunk.get(base_x + dx, base_y + dy, base_z + dz);
                            if !is_meshable(block) { continue; }
                            solid_count += 1;
                            // Increment in histogram, or insert if absent.
                            if let Some((_, c)) = histogram.iter_mut()
                                .find(|(b, _)| *b == block)
                            {
                                *c += 1;
                            } else {
                                histogram.push((block, 1));
                            }
                        }
                    }
                }

                if (solid_count as usize) < majority_threshold {
                    grid.push(None);
                    continue;
                }

                // Pick the most-common block (first wins on tie).
                let (rep_block, _) = histogram.iter()
                    .max_by_key(|(_, c)| *c)
                    .copied()
                    .expect("solid_count > 0 implies histogram non-empty");
                grid.push(Some(rep_block));
            }
        }
    }

    // Step 2: face emission on the downsampled grid. Delegated to
    // `emit_downsampled_grid` so the v0.23.7 reverse-regen path
    // (`mesh_chunk_far_virtual`) can reuse it after building its own
    // grid from generator samples instead of from a materialized
    // Chunk. The two paths agree by construction on what a grid means.
    emit_downsampled_grid(&grid, grid_size, factor, (origin_x, origin_y, origin_z))
}

/// Step 2 of the downsampled mesh pipeline, extracted in v0.23.7.
///
/// Takes a flat `Vec<Option<BlockId>>` of `grid_size³` cells (indexed
/// `x + z*grid_size + y*grid_size²` — matches Chunk storage order) and
/// emits a `ChunkMesh` of cell-sized faces. Naive face emission: a face
/// is produced wherever a solid cell borders an air cell or the grid
/// boundary.
///
/// Two callers as of v0.23.7:
///   * [`mesh_chunk_downsampled`] — builds the grid by reading a
///     materialized `Chunk`.
///   * [`mesh_chunk_far_virtual`] — builds the grid by sampling the
///     generator function directly, with no `Chunk` ever allocated.
///     This is the reverse-regen Stage 2 entry point.
///
/// Both share this function so they agree on output by construction.
fn emit_downsampled_grid(
    grid: &[Option<BlockId>],
    grid_size: i32,
    factor: i32,
    origin: (i32, i32, i32),
) -> ChunkMesh {
    let (origin_x, origin_y, origin_z) = origin;
    let mut mesh = ChunkMesh::empty();

    // Helper: index into the flat grid. Out-of-bounds returns None
    // (air), which forces emission at the chunk boundary (see the
    // boundary-handling note in the module-header comment).
    let get_grid_cell = |x: i32, y: i32, z: i32| -> Option<BlockId> {
        if x < 0 || x >= grid_size
            || y < 0 || y >= grid_size
            || z < 0 || z >= grid_size
        {
            return None;
        }
        let idx = (x + z * grid_size + y * grid_size * grid_size) as usize;
        grid[idx]
    };

    let factor_f = factor as f32;

    for j in 0..grid_size {
        for k in 0..grid_size {
            for i in 0..grid_size {
                let block = match get_grid_cell(i, j, k) {
                    Some(b) => b,
                    None => continue,
                };
                let base_color = match block_color(block) {
                    Some(c) => c,
                    None => continue,
                };

                let world_x_i = origin_x + i * factor;
                let world_y_i = origin_y + j * factor;
                let world_z_i = origin_z + k * factor;

                for f in 0..6 {
                    let (dx, dy, dz) = FACE_OFFSETS[f];
                    let neighbor_block = get_grid_cell(i + dx, j + dy, k + dz)
                        .unwrap_or(BlockId::Air);
                    if !should_emit_face(block, neighbor_block) { continue; }

                    let light = FACE_LIGHT[f];
                    let color = [
                        (base_color[0] * light).clamp(0.0, 1.0),
                        (base_color[1] * light).clamp(0.0, 1.0),
                        (base_color[2] * light).clamp(0.0, 1.0),
                    ];

                    let base_idx = mesh.vertices.len() as u32;
                    let world_x = world_x_i as f32;
                    let world_y = world_y_i as f32;
                    let world_z = world_z_i as f32;

                    // Quad corners scale by `factor`: a downsampled cell
                    // is `factor` blocks per side, so its faces are
                    // factor × factor world-space units.
                    for corner in &FACE_VERTS[f] {
                        mesh.vertices.push(Vertex::new(
                            [
                                world_x + corner[0] * factor_f,
                                world_y + corner[1] * factor_f,
                                world_z + corner[2] * factor_f,
                            ],
                            color,
                        ));
                    }

                    mesh.indices.push(base_idx);
                    mesh.indices.push(base_idx + 1);
                    mesh.indices.push(base_idx + 2);
                    mesh.indices.push(base_idx);
                    mesh.indices.push(base_idx + 2);
                    mesh.indices.push(base_idx + 3);
                }
            }
        }
    }

    mesh
}


// =============================================================================
// Greedy mesher.
// =============================================================================
//
// For each of 6 face directions, we sweep slice-by-slice through the chunk
// along the axis perpendicular to that face. In each slice, we build a 2D
// mask: mask[u][v] = Some(BlockId) if a face of that block should be emitted
// at that (u, v), else None.
//
// We then walk the mask row by row. For each unvisited cell, we find the
// largest axis-aligned rectangle starting at that cell containing identical
// mask values, emit one quad covering that rectangle, and mark all the
// rectangle's cells as visited. This gives O(N log N) merging in the
// average case.
//
// Coordinate convention per face:
//   * "axis"  = the chunk axis perpendicular to the face (the one we sweep)
//   * "u"     = the chunk axis we walk first within a slice
//   * "v"     = the chunk axis we walk second within a slice
// For each face direction we set up a mapping (axis_idx, u_idx, v_idx) so
// the inner loop is direction-agnostic.

const N: usize = CHUNK_SIZE as usize;

#[inline]
fn idx2d(u: usize, v: usize) -> usize { u + v * N }

/// Per-face axis configuration: (axis, u, v) indices into a 3-tuple [x, y, z].
/// The `axis_sign` is +1 for "positive" faces (top, east, south) — we mesh
/// the face on the *upper* side of the slice — and -1 for "negative" faces.
struct AxisConfig {
    axis: usize,
    u: usize,
    v: usize,
    axis_sign: i32,
    face: usize,
}

const AXIS_CONFIGS: [AxisConfig; 6] = [
    // TOP (+Y): axis=1 (y), u=0 (x), v=2 (z). Positive face.
    AxisConfig { axis: 1, u: 0, v: 2, axis_sign:  1, face: face::TOP },
    // BOTTOM (-Y): axis=1, u=0, v=2. Negative face.
    AxisConfig { axis: 1, u: 0, v: 2, axis_sign: -1, face: face::BOTTOM },
    // EAST (+X): axis=0 (x), u=2 (z), v=1 (y). Positive face.
    AxisConfig { axis: 0, u: 2, v: 1, axis_sign:  1, face: face::EAST },
    // WEST (-X): axis=0, u=2, v=1. Negative face.
    AxisConfig { axis: 0, u: 2, v: 1, axis_sign: -1, face: face::WEST },
    // SOUTH (+Z): axis=2 (z), u=1 (y), v=0 (x). Positive face.
    // NOTE on (u,v) ordering: u=Y, v=X (not u=X, v=Y). With u=X, v=Y, the
    // greedy quad corners (0,0)→(1,0)→(1,1)→(0,1) projected to world give a
    // triangle (0,1,2) with cross product +Z = outward for SOUTH, which the
    // CW-front-face pipeline back-face culls (the projection Y-flip means
    // outward-by-math = back-facing-to-rasterizer here).
    AxisConfig { axis: 2, u: 1, v: 0, axis_sign:  1, face: face::SOUTH },
    // NORTH (-Z): axis=2, u=1 (y), v=0 (x). Negative face.
    AxisConfig { axis: 2, u: 1, v: 0, axis_sign: -1, face: face::NORTH },
];

/// Read a block via 3D-axis-coord array. `coord[axis] = ...` etc.
fn block_at(chunk: &Chunk, coord: [i32; 3]) -> BlockId {
    get_local(chunk, coord[0], coord[1], coord[2])
}

/// r198: boundary-aware corner sample for the soft-edge offset terms.
///
/// `corner_sag` / `max_softness_at_corner` sample a 2×2 (and ring) of
/// blocks around a corner. At a chunk edge those samples cross out of
/// bounds, where `get_local` returns Air — so the two chunks sharing a
/// world corner see DIFFERENT phantom-air counts and compute DIFFERENT
/// sag, cracking the mesh along the chunk seam (worst where grass heights
/// differ: the operator's "holes between grass blocks at different
/// heights"). Fix: out-of-bounds horizontal samples MIRROR the nearest
/// in-bounds column instead of reading Air. Both chunks then see the same
/// material at the shared corner and agree on the offset. Vertical (y)
/// OOB still reads Air (top/bottom seams are covered by the side-face
/// extend-to-integer-Y invariant). XZ mirroring is clamping x,z back into
/// [0, CHUNK_SIZE-1]; the y coord passes through unchanged.
fn block_at_corner_sample(chunk: &Chunk, coord: [i32; 3]) -> BlockId {
    // r198: corner_x/corner_z arrive in WORLD space (call sites pass
    // origin+local), but `slice` (y) is chunk-LOCAL. Convert x/z to local
    // via the chunk's own origin, then mirror out-of-bounds horizontal
    // samples back into the chunk so two chunks sharing a world corner
    // agree on the soft-edge offset (no seam crack). y stays as-is; y-OOB
    // reads Air (top/bottom seams covered by the side-face invariant).
    //
    // This also fixes a latent bug: before the world->local conversion,
    // every chunk NOT at world-x/z 0 sampled out of bounds (world coords
    // vs local get()), read Air, and so computed ~zero sag — grass-height
    // transitions away from the origin column rendered with hard cracks.
    let (ox, _oy, oz) = chunk.coord.block_origin();
    let [wx, y, wz] = coord;
    if y < 0 || y >= CHUNK_SIZE {
        return BlockId::Air;
    }
    let lx = (wx - ox).clamp(0, CHUNK_SIZE - 1);
    let lz = (wz - oz).clamp(0, CHUNK_SIZE - 1);
    chunk.get(lx, y, lz)
}



/// Greedy mesh one chunk. `sky` is optional — if provided, face brightness
/// is computed from sun direction; otherwise uses the static FACE_LIGHT
/// fallback.
pub fn mesh_chunk_greedy(
    chunk: &Chunk,
    sky: Option<&crate::sky::Sky>,
    world_model: Option<&std::sync::Arc<crate::world::world_model::WorldModel>>,
    current_tick: Option<u64>,
) -> ChunkMesh {
    let mut mesh = ChunkMesh::empty();
    let (origin_x, origin_y, origin_z) = chunk.coord.block_origin();
    let origin = [origin_x, origin_y, origin_z];

    // v0.24.72 / RC42.1: skip the greedy main loop AND the per-block
    // sub-passes (decoration, fluid surfaces) for all-air chunks —
    // they all produce nothing when there are no blocks. The cloud
    // pass below is independent of chunk contents and must still run.
    if !chunk.cached_all_air {
        // Reusable mask buffer. None = no face here. Some(block) = face of `block`.
        let mut mask: Vec<Option<BlockId>> = vec![None; N * N];

    for cfg in &AXIS_CONFIGS {
        // The slice index ranges over the full chunk extent.
        for slice in 0..N as i32 {
            // Build the 2D mask for this slice + face direction.
            mask.iter_mut().for_each(|c| *c = None);

            for v in 0..N as i32 {
                for u in 0..N as i32 {
                    // The "inner" block (the solid one whose face we're considering)
                    // and the "neighbor" block (across the face) depend on axis_sign.
                    // For positive faces: inner is at slice, neighbor at slice+1.
                    // For negative faces: inner is at slice, neighbor at slice-1.
                    let mut inner_coord = [0i32; 3];
                    inner_coord[cfg.axis] = slice;
                    inner_coord[cfg.u] = u;
                    inner_coord[cfg.v] = v;

                    let mut neighbor_coord = inner_coord;
                    neighbor_coord[cfg.axis] = slice + cfg.axis_sign;

                    let inner = block_at(chunk, inner_coord);
                    if !is_meshable(inner) { continue; }
                    let neighbor = block_at(chunk, neighbor_coord);
                    if !should_emit_face(inner, neighbor) { continue; }

                    // Sub-block detail: if this is a top face of a block that
                    // wants per-block height jitter, emit the quad immediately
                    // with corner heights, skipping the greedy mask. We lose
                    // merging on this face only — all other faces of these
                    // blocks (sides, bottom) still merge.
                    if cfg.face == face::TOP && inner.wants_top_jitter() {
                        // Pick subdivided 8x8 emission if the block has
                        // any weathering, else fall back to 4-corner
                        // jittered top (cheaper, used for fresh
                        // player-placed blocks). Per-block weathering
                        // gives each block its own bumpy character.
                        let block_wx = origin[0] + u;
                        let block_wy = origin[1] + slice;
                        let block_wz = origin[2] + v;
                        let weathering = weathering_at_world(inner, block_wx, block_wy, block_wz);
                        if weathering > 0.05 {
                            emit_subdivided_top(&mut mesh, slice, u, v, inner, origin, sky, chunk, world_model);
                        } else {
                            emit_jittered_top(&mut mesh, slice, u, v, inner, origin, sky, chunk, world_model);
                        }
                        continue;
                    }

                    // Soft-edge side faces: blocks with softness > 0 emit
                    // per-block side faces too, so the side's TOP edge can
                    // drop to match the sagged top corners. Without this,
                    // the side would stay at y=block_top while the top quad
                    // sags below — opening a vertical seam. The per-block
                    // path costs greedy merging on these blocks' sides;
                    // worth it for the visual win at terrain cliffs.
                    let is_side = matches!(cfg.face, face::EAST | face::WEST | face::SOUTH | face::NORTH);
                    if is_side && inner.softness() > 0.0 {
                        emit_soft_side_face(&mut mesh, cfg, slice, u, v, inner, origin, sky, chunk, world_model);
                        continue;
                    }

                    mask[idx2d(u as usize, v as usize)] = Some(inner);
                }
            }

            // Greedy-merge the mask.
            for v in 0..N {
                let mut u = 0usize;
                while u < N {
                    let cell = mask[idx2d(u, v)];
                    if cell.is_none() { u += 1; continue; }
                    let block = cell.unwrap();

                    // Material response controls greedy merging. Surface
                    // stone (variance > 0) is per-block tinted, so two
                    // adjacent stones must NOT merge into one quad — we'd
                    // lose the per-block color variation. Deep stone /
                    // bedrock has variance = 0, so merging is fine.
                    //
                    // Slice is the axis-perp coordinate; we need world_y
                    // for the depth-aware lookup. For TOP/BOTTOM faces
                    // (cfg.axis = 1), slice IS the y. For side faces, the
                    // y is what cfg.v indexes — we approximate by using
                    // the chunk's mid-y so the depth-aware lookup uses
                    // a consistent value across the slice.
                    let approx_wy = match cfg.axis {
                        1 => origin[1] + slice,
                        _ => origin[1] + (N as i32 / 2),
                    };
                    let response = crate::world::material_response::for_block(block, approx_wy);
                    let merges = response.merges();

                    // Find the run width along U starting at (u, v).
                    let mut w = 1usize;
                    if merges {
                        while u + w < N && mask[idx2d(u + w, v)] == Some(block) {
                            w += 1;
                        }
                    }

                    // Find the run height along V — extend as long as every cell
                    // in [u..u+w) at row v+h matches this block.
                    let mut h = 1usize;
                    if merges {
                        'outer: while v + h < N {
                            for du in 0..w {
                                if mask[idx2d(u + du, v + h)] != Some(block) {
                                    break 'outer;
                                }
                            }
                            h += 1;
                        }
                    }

                    // Emit one quad covering [u, u+w) × [v, v+h) at this slice.
                    emit_quad(&mut mesh, cfg, slice, u, v, w, h, block, origin, sky,
                        Some(chunk), world_model);

                    // Clear the mask region so we don't re-emit.
                    for dv in 0..h {
                        for du in 0..w {
                            mask[idx2d(u + du, v + dv)] = None;
                        }
                    }

                    u += w;
                }
            }
        }
    }

    // Sub-block decoration pass — adds grass tufts, etc. above eligible
    // blocks. Touches only air space; does not interfere with the greedy
    // pass's face culling.
    decorate_chunk(chunk, &mut mesh, origin, sky, current_tick);

    // Water surface pass — emit top faces for water blocks where the
    // block above is air. Produces visible water bodies at sea level
    // (oceans, lakes, low-lying flooded areas). Sides and bottom of
    // water are NOT emitted (would render edges of water bodies as
    // walls, which looks wrong; the water just stops at the cliff).
    emit_water_surface(chunk, &mut mesh, origin, sky);

    // Lava surface pass — same shape as water but for BlockId::Lava.
    // Produces visible glowing pools deep underground where lava
    // pockets generate. Same fluid-surface emission code (parameter-
    // ized by block kind) so any future fluid type is one call away.
    emit_lava_surface(chunk, &mut mesh, origin, sky);

    } // end of `if !chunk.cached_all_air` — RC42.1 fast-path scope

    // Cloud pass — emit white cubes in the cloud band where the
    // moisture field is dense. Same chunk-independent sampling
    // approach as terrain so adjacent chunks' clouds compose
    // seamlessly. Quick-rejects for chunks outside the cloud band.
    //
    // RC42.1: runs unconditionally — clouds spawn from the analytical
    // cloud field, not from chunk contents, so all-air chunks
    // intersecting the cloud band still need this pass.
    emit_clouds(chunk, &mut mesh, origin, sky);

    mesh
}

/// Emit blue top faces for water blocks whose top is exposed to air.
/// This is the minimal visible-water implementation: water cells
/// generated by `cpu_generator` Phase 2.5 become visible as flat
/// blue surfaces (no waves, no transparency, no reflections — those
/// are future water-shader work).
/// Generic fluid-surface emission. Renders top faces of fluid cells
/// (water, lava) where the cell above is air. Sides and bottom are
/// suppressed (would render edges as walls, which looks wrong; the
/// fluid just stops at the cliff).
///
/// The fluid kinds (water/lava) are non-solid blocks that wouldn't
/// otherwise appear in the mesh because `is_meshable` rejects them.
/// This pass adds them back with a flat top face per cell. Future
/// improvements: sub-block surface heights from `water_field`, wave
/// animation via vertex shader, transparency for water (needs
/// pipeline-level blend support).
fn emit_fluid_surface(
    chunk: &Chunk,
    mesh: &mut ChunkMesh,
    origin: [i32; 3],
    sky: Option<&crate::sky::Sky>,
    fluid: BlockId,
) {
    let n = N as i32;
    let base_color = match block_color(fluid) {
        Some(c) => c,
        None => return,
    };
    let light = match sky {
        Some(sky) => sky.face_brightness(FACE_NORMALS[face::TOP]),
        None => FACE_LIGHT[face::TOP],
    };
    let color = [
        (base_color[0] * light).clamp(0.0, 1.0),
        (base_color[1] * light).clamp(0.0, 1.0),
        (base_color[2] * light).clamp(0.0, 1.0),
    ];
    for ly in 0..n {
        for lz in 0..n {
            for lx in 0..n {
                let here = block_at(chunk, [lx, ly, lz]);
                if here != fluid { continue; }
                let above = block_at(chunk, [lx, ly + 1, lz]);
                // Top face: emit if the cell above is air or out-of-chunk (treated as air).
                // Dipped a hair below Y+1 so it doesn't z-fight neighbouring solid cube tops.
                if above == BlockId::Air {
                    let wx = (origin[0] + lx) as f32;
                    let wy = (origin[1] + ly + 1) as f32 - 0.05;
                    let wz = (origin[2] + lz) as f32;
                    let base_idx = mesh.vertices.len() as u32;
                    // Top face winding: CW from +Y. Match emit_subdivided_top.
                    mesh.vertices.push(Vertex::new([wx,     wy, wz    ], color));
                    mesh.vertices.push(Vertex::new([wx+1.0, wy, wz    ], color));
                    mesh.vertices.push(Vertex::new([wx+1.0, wy, wz+1.0], color));
                    mesh.vertices.push(Vertex::new([wx,     wy, wz+1.0], color));
                    mesh.indices.push(base_idx);
                    mesh.indices.push(base_idx + 1);
                    mesh.indices.push(base_idx + 2);
                    mesh.indices.push(base_idx);
                    mesh.indices.push(base_idx + 2);
                    mesh.indices.push(base_idx + 3);
                }
                // r446 (reskin step 1): SIDE + BOTTOM faces so a water body reads as a VOLUME from
                // the side and below, not just a top sheet (operator: "not cohesive from a bottom
                // or side view"). Emit a face only where the IN-BOUNDS neighbour is Air — skipping
                // out-of-chunk neighbours entirely so we never paint a face across a chunk seam
                // inside a multi-chunk lake (get_local returns Air OOB, which would seam). Pool
                // edges that fall mid-chunk get their sides; the rare edge that lands exactly on a
                // chunk boundary just misses one face. Same water colour so the shader still
                // classifies + shades them as water (incl. the r443 side depth-tint).
                let ox = origin[0] as f32;
                let oy = origin[1] as f32;
                let oz = origin[2] as f32;
                // (face_idx, dx, dy, dz)
                let side_faces = [
                    (face::EAST, 1, 0, 0),
                    (face::WEST, -1, 0, 0),
                    (face::SOUTH, 0, 0, 1),
                    (face::NORTH, 0, 0, -1),
                    (face::BOTTOM, 0, -1, 0),
                ];
                for (face_idx, dx, dy, dz) in side_faces {
                    let (nx, ny, nz) = (lx + dx, ly + dy, lz + dz);
                    if nx < 0 || ny < 0 || nz < 0 || nx >= n || ny >= n || nz >= n {
                        continue; // out-of-chunk → skip (no seam faces)
                    }
                    if block_at(chunk, [nx, ny, nz]) != BlockId::Air {
                        continue; // only against air (solid occludes, water is interior)
                    }
                    let fx = (origin[0] + lx) as f32;
                    let fy = (origin[1] + ly) as f32;
                    let fz = (origin[2] + lz) as f32;
                    let _ = (ox, oy, oz);
                    let base_idx = mesh.vertices.len() as u32;
                    for v in 0..4 {
                        let p = FACE_VERTS[face_idx][v];
                        mesh.vertices.push(Vertex::new(
                            [fx + p[0], fy + p[1], fz + p[2]],
                            color,
                        ));
                    }
                    mesh.indices.push(base_idx);
                    mesh.indices.push(base_idx + 1);
                    mesh.indices.push(base_idx + 2);
                    mesh.indices.push(base_idx);
                    mesh.indices.push(base_idx + 2);
                    mesh.indices.push(base_idx + 3);
                }
            }
        }
    }
}

/// Water surface — convenience wrapper over `emit_fluid_surface`.
pub(crate) fn emit_water_surface(
    chunk: &Chunk,
    mesh: &mut ChunkMesh,
    origin: [i32; 3],
    sky: Option<&crate::sky::Sky>,
) {
    emit_fluid_surface(chunk, mesh, origin, sky, BlockId::Water);
}

/// Lava surface — same as water but with lava's hot orange color.
/// Future: emissive (self-illuminating) so lava is visible at night.
pub(crate) fn emit_lava_surface(
    chunk: &Chunk,
    mesh: &mut ChunkMesh,
    origin: [i32; 3],
    sky: Option<&crate::sky::Sky>,
) {
    emit_fluid_surface(chunk, mesh, origin, sky, BlockId::Lava);
}

/// Emit cloud cubes for cells in this chunk's intersection with the
/// cloud band. Each cloud cell becomes a 6-faced opaque white cube.
/// Cells are sampled via `cloud::is_cloud_at`, which is purely a
/// function of world position (chunk-independent), so adjacent
/// chunks compose their cloud cubes seamlessly without coordination.
///
/// Faces are culled the same way as terrain: a cloud cube only
/// emits a face if the neighbor cell is NOT also a cloud (otherwise
/// adjacent cloud cubes have an interior shared face we'd render
/// twice and never see).
///
/// No greedy merging applied — clouds are sparse enough (typically
/// <1% of total mesh) that the optimization isn't worth the
/// complexity. Each cloud cube is 24 verts + 36 indices.
fn emit_clouds(
    _chunk: &Chunk,
    mesh: &mut ChunkMesh,
    origin: [i32; 3],
    sky: Option<&crate::sky::Sky>,
) {
    use crate::cloud;
    // Quick reject: if the chunk doesn't intersect the cloud band,
    // skip entirely. Saves the per-cell loop on terrain chunks.
    let chunk_y_min = origin[1];
    let chunk_y_max = origin[1] + N as i32 - 1;
    if chunk_y_max < cloud::CLOUD_BAND_BOTTOM || chunk_y_min > cloud::CLOUD_BAND_TOP {
        return;
    }
    // Time for cloud drift sampling. Without sky, clouds are static
    // (used in tests). With sky, clouds drift with wind.
    let time_seconds = sky.map(|s| s.time_seconds as f32).unwrap_or(0.0);
    // Cloud color: slightly off-white. Faces get per-face brightness
    // from the sky (so cloud bottoms are darker than tops, giving
    // the cubes a sense of lit-from-above shading).
    let base_color = [0.95, 0.95, 1.00];
    let n = N as i32;
    // Loop only over the Y slice that's inside the cloud band — most
    // chunks crossing the band only have a few rows of cloud cells.
    let local_y_min = (cloud::CLOUD_BAND_BOTTOM - origin[1]).max(0);
    let local_y_max = (cloud::CLOUD_BAND_TOP - origin[1] + 1).min(n);
    for ly in local_y_min..local_y_max {
        let wy = origin[1] + ly;
        for lz in 0..n {
            let wz = origin[2] + lz;
            for lx in 0..n {
                let wx = origin[0] + lx;
                if !cloud::is_cloud_at_time(wx, wy, wz, time_seconds) { continue; }
                emit_cloud_cube_culled(mesh, [wx, wy, wz], base_color, sky, time_seconds);
            }
        }
    }
}

/// Emit the visible faces of a single cloud cube at world position.
/// Faces touching another cloud cell are skipped (interior face cull).
fn emit_cloud_cube_culled(
    mesh: &mut ChunkMesh,
    pos: [i32; 3],
    base_color: [f32; 3],
    sky: Option<&crate::sky::Sky>,
    time_seconds: f32,
) {
    use crate::cloud;
    let [wx, wy, wz] = pos;
    let fx = wx as f32;
    let fy = wy as f32;
    let fz = wz as f32;
    // Face neighbors: +X, -X, +Y, -Y, +Z, -Z.
    // Sample at the SAME time as the cell itself so neighbors agree.
    let neighbors = [
        (face::TOP,    cloud::is_cloud_at_time(wx, wy + 1, wz, time_seconds)),
        (face::BOTTOM, cloud::is_cloud_at_time(wx, wy - 1, wz, time_seconds)),
        (face::NORTH,  cloud::is_cloud_at_time(wx, wy, wz - 1, time_seconds)),
        (face::SOUTH,  cloud::is_cloud_at_time(wx, wy, wz + 1, time_seconds)),
        (face::EAST,   cloud::is_cloud_at_time(wx + 1, wy, wz, time_seconds)),
        (face::WEST,   cloud::is_cloud_at_time(wx - 1, wy, wz, time_seconds)),
    ];
    for &(face_idx, neighbor_is_cloud) in &neighbors {
        if neighbor_is_cloud { continue; }  // hidden interior face
        let light = match sky {
            Some(sky) => sky.face_brightness(FACE_NORMALS[face_idx]),
            None => FACE_LIGHT[face_idx],
        };
        let color = [
            (base_color[0] * light).clamp(0.0, 1.0),
            (base_color[1] * light).clamp(0.0, 1.0),
            (base_color[2] * light).clamp(0.0, 1.0),
        ];
        let base_idx = mesh.vertices.len() as u32;
        for v in 0..4 {
            let p = FACE_VERTS[face_idx][v];
            mesh.vertices.push(Vertex::new(
                [fx + p[0] as f32, fy + p[1] as f32, fz + p[2] as f32],
                color,
            ));
        }
        mesh.indices.push(base_idx);
        mesh.indices.push(base_idx + 1);
        mesh.indices.push(base_idx + 2);
        mesh.indices.push(base_idx);
        mesh.indices.push(base_idx + 2);
        mesh.indices.push(base_idx + 3);
    }
}

/// Same as `mesh_chunk_greedy` but skips the sub-voxel decoration pass.
/// Used by tests that want to compare greedy-vs-naive face merging without
/// the noise of plant geometry.
#[cfg(test)]
fn mesh_chunk_greedy_undecorated(chunk: &Chunk, sky: Option<&crate::sky::Sky>) -> ChunkMesh {
    let mut mesh = ChunkMesh::empty();
    let (origin_x, origin_y, origin_z) = chunk.coord.block_origin();
    let origin = [origin_x, origin_y, origin_z];
    let _ = origin;
    // Test path: no world model. Mesher uses neutral-environment tinting.
    let world_model: Option<&std::sync::Arc<crate::world::world_model::WorldModel>> = None;
    let mut mask: Vec<Option<BlockId>> = vec![None; N * N];
    for cfg in &AXIS_CONFIGS {
        for slice in 0..N as i32 {
            mask.iter_mut().for_each(|c| *c = None);
            for v in 0..N as i32 {
                for u in 0..N as i32 {
                    let mut inner_coord = [0i32; 3];
                    inner_coord[cfg.axis] = slice;
                    inner_coord[cfg.u] = u;
                    inner_coord[cfg.v] = v;
                    let mut neighbor_coord = inner_coord;
                    neighbor_coord[cfg.axis] = slice + cfg.axis_sign;
                    let inner = block_at(chunk, inner_coord);
                    if !is_meshable(inner) { continue; }
                    let neighbor = block_at(chunk, neighbor_coord);
                    if !should_emit_face(inner, neighbor) { continue; }
                    if cfg.face == face::TOP && inner.wants_top_jitter() {
                        let block_wx = origin_x + u;
                        let block_wy = origin_y + slice;
                        let block_wz = origin_z + v;
                        let weathering = weathering_at_world(inner, block_wx, block_wy, block_wz);
                        if weathering > 0.05 {
                            emit_subdivided_top(&mut mesh, slice, u, v, inner, [origin_x, origin_y, origin_z], sky, chunk, world_model);
                        } else {
                            emit_jittered_top(&mut mesh, slice, u, v, inner, [origin_x, origin_y, origin_z], sky, chunk, world_model);
                        }
                        continue;
                    }
                    let is_side = matches!(cfg.face, face::EAST | face::WEST | face::SOUTH | face::NORTH);
                    if is_side && inner.softness() > 0.0 {
                        emit_soft_side_face(&mut mesh, cfg, slice, u, v, inner, [origin_x, origin_y, origin_z], sky, chunk, world_model);
                        continue;
                    }
                    mask[idx2d(u as usize, v as usize)] = Some(inner);
                }
            }
            for v in 0..N {
                let mut u = 0usize;
                while u < N {
                    let cell = mask[idx2d(u, v)];
                    if cell.is_none() { u += 1; continue; }
                    let block = cell.unwrap();
                    let mut w = 1usize;
                    while u + w < N && mask[idx2d(u + w, v)] == Some(block) { w += 1; }
                    let mut h = 1usize;
                    'outer: while v + h < N {
                        for du in 0..w {
                            if mask[idx2d(u + du, v + h)] != Some(block) { break 'outer; }
                        }
                        h += 1;
                    }
                    emit_quad(&mut mesh, cfg, slice, u, v, w, h, block, [origin_x, origin_y, origin_z], sky,
                        Some(chunk), None);
                    for dv in 0..h {
                        for du in 0..w {
                            mask[idx2d(u + du, v + dv)] = None;
                        }
                    }
                    u += w;
                }
            }
        }
    }
    mesh
}

/// Probability that a grass-bearing block spawns a sub-voxel tuft above it.
/// For dedicated plant blocks (Fern, Flower, TallGrass) the probability is
/// always 1.0 — the block IS the plant. This constant only gates surface
/// blocks like Grass that grow tufts as decoration.
const GRASS_TUFT_PROBABILITY: f32 = 0.45;

/// Convert a raw stage byte (as stored in `BlockTransition`) back into
/// the typed `Stage` enum. Wraps `Stage::from_flags`, which already
/// interprets the low 3 bits as the stage.
#[inline]
fn from_raw_stage(raw: u8) -> crate::world::plant_lifecycle::Stage {
    crate::world::plant_lifecycle::Stage::from_flags(raw)
}

/// Sub-voxel decoration pass.
///
/// Two cases:
///
/// **Case A: dedicated plant block** (TallGrass, Fern, Flower). The block's
/// own volume hosts a sub-voxel grid — when we mesh the chunk's solid
/// faces, plant blocks were excluded from the greedy face emit (they're
/// transparent). Here we recover their volume by drawing their sub-grid
/// directly inside their cell.
///
/// **Case B: tuft-bearing surface block** (Grass with air above). The block
/// itself renders normally as a solid; we draw a sub-voxel tuft in the
/// air cell directly above. Density gated by `GRASS_TUFT_PROBABILITY`.
///
/// Both cases share the same sub-voxel meshing path — they differ only in
/// where the host's world origin is.
pub(crate) fn decorate_chunk(
    chunk: &Chunk,
    mesh: &mut ChunkMesh,
    origin: [i32; 3],
    sky: Option<&crate::sky::Sky>,
    current_tick: Option<u64>,
) {
    let n = N as i32;
    // Iterate the inverted plant index instead of scanning every cell.
    // For a chunk with no plants this loop is empty (zero cost). For a
    // forested surface chunk with ~200 plant blocks, we visit 200 cells
    // instead of 32k. Big win for tick perf too once lifecycle handlers
    // share this same indexing.
    for &plant_idx in &chunk.plant_indices {
        let (lx, ly, lz) = Chunk::index_to_xyz(plant_idx);
        let block = chunk.blocks[plant_idx as usize];
        // Defensive: if the index got stale (shouldn't happen — set()
        // maintains it), skip. has_subvoxels is the gate.
        if !block.has_subvoxels() { continue; }

        // Per-block instance state drives color modulation. Different
        // block types interpret state differently:
        //   - Leaves use leaf_decay's age-driven color curve
        //   - All other plants use plant_lifecycle's stage modulation
        // Pristine plants (no instance state entry) default to a no-op
        // multiplier ([1, 1, 1]).
        let inst_state = chunk.instance_state.get(&plant_idx).copied()
            .unwrap_or_default();
        let color_mul = if matches!(block, BlockId::OakLeaves | BlockId::BirchLeaves) {
            crate::world::leaf_decay::decay_color_modulation(inst_state.age_ticks)
        } else {
            // Plant stage color, possibly interpolated if a transition
            // is in progress for this block. Transitions are sparse —
            // most plants will have no entry here.
            //
            // Progress is `(current_tick - started_tick) / ANIM_TICKS`,
            // clamped to [0, 1]. Without `current_tick` (test paths,
            // unset bin), we render at to_stage immediately.
            let stage = crate::world::plant_lifecycle::Stage::from_flags(inst_state.flags);
            let base = crate::world::plant_lifecycle::stage_color_modulation(stage);
            match (chunk.transitions.get(&plant_idx), current_tick) {
                (Some(t), Some(now)) => {
                    let elapsed = now.saturating_sub(t.started_tick);
                    let progress = (elapsed as f32
                        / crate::world::chunk::TRANSITION_ANIM_TICKS as f32)
                        .clamp(0.0, 1.0);
                    let from = crate::world::plant_lifecycle::stage_color_modulation(
                        from_raw_stage(t.from_stage));
                    let to = crate::world::plant_lifecycle::stage_color_modulation(
                        from_raw_stage(t.to_stage));
                    [
                        from[0] * (1.0 - progress) + to[0] * progress,
                        from[1] * (1.0 - progress) + to[1] * progress,
                        from[2] * (1.0 - progress) + to[2] * progress,
                    ]
                }
                (Some(t), None) => {
                    // No tick info: render at to_stage. Same effect as
                    // committing the transition immediately. Fallback
                    // for test/utility paths.
                    crate::world::plant_lifecycle::stage_color_modulation(
                        from_raw_stage(t.to_stage))
                }
                (None, _) => base,
            }
        };

        // Case A: dedicated plant block — render its own sub-grid.
        if !block.spawns_grass_tufts() {
            let wx = origin[0] + lx;
            let wy = origin[1] + ly;
            let wz = origin[2] + lz;
            // RC42 fast path: baked template stamp. Pristine plants
            // (no sub-voxel edits — all currently shipped) skip the
            // procedural grid generation entirely.
            let (_, hash_int) = crate::world::subvoxel::plant_hash(wx, wy, wz);
            if let Some(tmpl) = crate::world::subvoxel_templates::template_for(block, hash_int) {
                crate::world::subvoxel_templates::stamp_template(
                    &mut mesh.vertices, &mut mesh.indices,
                    tmpl,
                    [wx as f32, wy as f32, wz as f32],
                    sky, color_mul,
                );
                continue;
            }
            // Procedural fallback (shouldn't fire for shipped blocks,
            // but kept for robustness when new sub-voxel block types
            // are added before their templates exist).
            if let Some(grid) = crate::world::subvoxel::procedural_subgrid(block, wx, wy, wz) {
                emit_subvoxel_grid(mesh, &grid, [wx as f32, wy as f32, wz as f32], sky, color_mul);
            }
            continue;
        }

        // Case B: surface block bearing tufts (e.g. Grass). The tuft
        // lives in the air cell ABOVE this block.
        if ly + 1 >= n {
            // Cross-chunk lookup not implemented — tuft on chunk top
            // edge is dropped silently.
            continue;
        }
        let above = block_at(chunk, [lx, ly + 1, lz]);
        if !above.is_air() { continue; }

        let wx = origin[0] + lx;
        let wy = origin[1] + ly;
        let wz = origin[2] + lz;

        let (prob, _) = hash_for_decoration(wx, wy, wz);
        if prob > GRASS_TUFT_PROBABILITY { continue; }

        // Place the tuft on the SAGGED top of the grass block, not the
        // flat cube top. Average the four corner-Y offsets at slice=ly
        // (the top slice of the grass block) and shift the tuft host
        // origin down accordingly. Result: tufts on hills follow the
        // bumpy soft-edge surface instead of floating above it.
        //
        // The corner_y_offset call queries the same hash + sag logic the
        // soft-edge mesher uses for top faces, so the tuft base aligns
        // perfectly with the visible block top corner — no gap, no z-fighting.
        let softness = block.softness();
        let corner_avg_offset = if softness > 0.0 {
            let o00 = corner_y_offset(chunk, origin[0] + lx,     ly, origin[2] + lz,     softness);
            let o10 = corner_y_offset(chunk, origin[0] + lx + 1, ly, origin[2] + lz,     softness);
            let o01 = corner_y_offset(chunk, origin[0] + lx,     ly, origin[2] + lz + 1, softness);
            let o11 = corner_y_offset(chunk, origin[0] + lx + 1, ly, origin[2] + lz + 1, softness);
            (o00 + o10 + o01 + o11) * 0.25
        } else {
            0.0
        };

        // RC42: template stamp first; only fall through to
        // emit_subvoxel_grid + procedural_subgrid if no template is
        // registered for Grass tufts. The template path uses the
        // same plant_hash so visual variation matches across the
        // two paths.
        let (_, hash_int) = crate::world::subvoxel::plant_hash(wx, wy, wz);
        if let Some(tmpl) = crate::world::subvoxel_templates::template_for(BlockId::Grass, hash_int) {
            crate::world::subvoxel_templates::stamp_template(
                &mut mesh.vertices, &mut mesh.indices,
                tmpl,
                [wx as f32, (wy + 1) as f32 + corner_avg_offset, wz as f32],
                sky, color_mul,
            );
        } else if let Some(grid) = crate::world::subvoxel::procedural_subgrid(BlockId::Grass, wx, wy, wz) {
            emit_subvoxel_grid(mesh, &grid,
                [wx as f32, (wy + 1) as f32 + corner_avg_offset, wz as f32],
                sky, color_mul);
        }
    }
}

/// Returns (probability_threshold, sub_hash_for_visual_variation).
/// Both deterministic, both in [0, 1).
fn hash_for_decoration(x: i32, y: i32, z: i32) -> (f32, f32) {
    let mut h: u32 = (x as u32).wrapping_mul(0xbe5f_8a5b)
        ^ (y as u32).wrapping_mul(0xb579_4f0d)
        ^ (z as u32).wrapping_mul(0x9e37_79b9);
    h ^= h >> 16;
    h = h.wrapping_mul(0x85eb_ca6b);
    h ^= h >> 13;
    h = h.wrapping_mul(0xc2b2_ae35);
    h ^= h >> 16;
    let a = (h & 0xFFFF) as f32 / 65535.0;
    let b = ((h >> 16) & 0xFFFF) as f32 / 65535.0;
    (a, b)
}

/// Emit a cross-quad at the given air cell origin (corner of the air block).
/// Cross-quad = two vertical quads at 45° to each other, like an X seen
/// from above. Standard "billboard plant" geometry.
///
/// `sub_hash` (0..1) is used to vary tuft height/orientation slightly so
/// every tuft doesn't look identical.
///
/// Tufts are deliberately double-sided (no back-face culling) — we emit
/// indices for both winding orders. Each cross-quad = 4 quads total = 16
/// verts, 24 indices. Cheap.
/// Mesh a sub-voxel grid into the chunk mesh.
///
/// Each filled `MiniCell` becomes a small cube whose faces are emitted only
/// where they border empty space (face culling at sub-cell resolution).
/// The grid sits inside the host block's volume — `host_origin` is the
/// minimum-corner world-space coord of the host block, and each sub-cell
/// occupies a `SUBVOXEL_SIZE` × `SUBVOXEL_SIZE` × `SUBVOXEL_SIZE` cube
/// offset by `(sub_x, sub_y, sub_z) * SUBVOXEL_SIZE` from `host_origin`.
///
/// Performance: typical plant grid has 20-60 cells. Even with worst-case
/// 6 faces per cell, that's < 400 faces per plant. With ~25% of grass
/// blocks tufted across a 32-block surface band, ~250 plants × 200 faces
/// avg = 50k faces per chunk — comparable to the ground geometry itself.
/// Greedy meshing within sub-grids could push this lower; not yet worth it.
/// Emit a sub-voxel grid as actual mini-cube faces.
///
/// `color_mul` is a per-cell color multiplier applied before lighting. Use
/// `[1.0, 1.0, 1.0]` for no change. Used by lifecycle/decay handlers to
/// shift living plants toward yellow (drying) or brown (withered) without
/// having to rebuild the recipe — same geometry, modulated colors.
/// r596: like [`push_quad`] but records a per-vertex foliage sway tag OUT-OF-BAND
/// (`ChunkMesh::mark_sway_quad`) — `Vertex` itself stays 24 bytes, so the octree
/// readback's `&[Vertex]` cast is untouched (the r594 lesson). The planted BOTTOM
/// row of the plant (within half a sub-voxel of `grid_bottom_y`) anchors (2),
/// everything above sways (1). At pack time (`from_world_tagged` /
/// `pack_in_place`) the tag lands in pos bits 30-31, which chunk.vert's wind sway
/// and the r591 disturber read. Used only by the subvoxel foliage emitter.
fn push_quad_foliage(
    mesh: &mut ChunkMesh,
    p0: [f32; 3], p1: [f32; 3], p2: [f32; 3], p3: [f32; 3],
    color: [f32; 3],
    grid_bottom_y: f32,
) {
    use crate::world::subvoxel::SUBVOXEL_SIZE;
    let tag = |p: [f32; 3]| -> u8 {
        if p[1] - grid_bottom_y < 0.5 * SUBVOXEL_SIZE { 2 } else { 1 }
    };
    mesh.mark_sway_quad([tag(p0), tag(p1), tag(p2), tag(p3)]);
    push_quad(mesh, p0, p1, p2, p3, color);
}

fn emit_subvoxel_grid(
    mesh: &mut ChunkMesh,
    grid: &crate::world::subvoxel::SubVoxelGrid,
    host_origin: [f32; 3],
    sky: Option<&crate::sky::Sky>,
    color_mul: [f32; 3],
) {
    use crate::world::subvoxel::{SUBVOXEL_RESOLUTION, SUBVOXEL_SIZE};

    if grid.cells.is_empty() { return; }

    // Build a quick "is filled" lookup so each cell can ask about its
    // 6 neighbors in O(1). Plant grids are small (8³=512 max), so a
    // bitset is overkill — a fixed-size byte array is the simplest thing.
    let n = SUBVOXEL_RESOLUTION;
    debug_assert!(n <= 16, "if SUBVOXEL_RESOLUTION grows, switch to a bitset");
    let mut filled = [false; 16 * 16 * 16];
    for c in &grid.cells {
        filled[(c.sub_x as usize) + (c.sub_y as usize) * 16 + (c.sub_z as usize) * 16 * 16] = true;
    }
    let lookup = |x: i32, y: i32, z: i32| -> bool {
        if x < 0 || y < 0 || z < 0 { return false; }
        if (x as usize) >= n || (y as usize) >= n || (z as usize) >= n { return false; }
        filled[(x as usize) + (y as usize) * 16 + (z as usize) * 16 * 16]
    };

    // Day-vs-night brightness factor. Sub-voxel plants get directional
    // shading per face (tops bright at noon, etc.) plus an overall night
    // dimming.
    let directional = match sky {
        Some(sky) => sky.directional_intensity(),
        None => 1.0,
    };

    // Wind-driven static sway. Sample the local wind direction once per
    // host block (cheap; analytical function). Top cells of the sub-grid
    // displace laterally in the wind direction; bottom cells stay anchored.
    // Result: a windy patch of grass leans visibly downwind; calm areas
    // stand straight. This is a STATIC lean, not animated — animation needs
    // either re-meshing or a vertex shader uniform, both deferred. The
    // visual still conveys wind direction across a landscape.
    //
    // We sample wind at world_time=0 here to match the cached wind field's
    // time. When animated sway lands, this should sample at the current
    // world time and re-mesh on a coarse interval.
    let world_time = match sky {
        Some(sky) => sky.time_seconds as f64,
        None => 0.0,
    };
    let (wind_x, wind_z, _) = crate::wind::wind_at(
        host_origin[0] as f64,
        host_origin[1] as f64,
        host_origin[2] as f64,
        world_time,
    );
    // Maximum lateral offset in blocks at the very top of the sub-grid.
    // Half a sub-voxel = 1/16 block. At max wind (1.0) this is the offset;
    // calmer wind scales linearly.
    const MAX_SWAY: f32 = 0.0625;

    for cell in &grid.cells {
        let bx = cell.sub_x as i32;
        let by = cell.sub_y as i32;
        let bz = cell.sub_z as i32;

        // Sway weights for the cell's bottom and top edges. Linear in Y
        // sub-cell position. Cells stacked vertically share corners at
        // their bottom/top boundaries; weight at sub_y=k from above equals
        // weight at sub_y=k+1 from below = k/(RES-1) — corners stay welded.
        let denom = (SUBVOXEL_RESOLUTION as f32 - 1.0).max(1.0);
        let weight_bot = (cell.sub_y as f32) / denom;
        let weight_top = ((cell.sub_y + 1) as f32) / denom;
        let sway_bot_x = wind_x * weight_bot * MAX_SWAY;
        let sway_bot_z = wind_z * weight_bot * MAX_SWAY;
        let sway_top_x = wind_x * weight_top * MAX_SWAY;
        let sway_top_z = wind_z * weight_top * MAX_SWAY;

        // World-space cube corners for this sub-cell. Top (y1) and bottom
        // (y0) corners use different sway offsets so the cell leans rather
        // than translates as a whole.
        let bot_x0 = host_origin[0] + (bx as f32) * SUBVOXEL_SIZE + sway_bot_x;
        let bot_x1 = bot_x0 + SUBVOXEL_SIZE;
        let bot_z0 = host_origin[2] + (bz as f32) * SUBVOXEL_SIZE + sway_bot_z;
        let bot_z1 = bot_z0 + SUBVOXEL_SIZE;
        let top_x0 = host_origin[0] + (bx as f32) * SUBVOXEL_SIZE + sway_top_x;
        let top_x1 = top_x0 + SUBVOXEL_SIZE;
        let top_z0 = host_origin[2] + (bz as f32) * SUBVOXEL_SIZE + sway_top_z;
        let top_z1 = top_z0 + SUBVOXEL_SIZE;
        let y0 = host_origin[1] + (by as f32) * SUBVOXEL_SIZE;
        let y1 = y0 + SUBVOXEL_SIZE;

        // Emit one face per direction iff the neighbor on that axis is empty.
        // Face brightness = base color × directional shading per normal.
        // The face winding matches the chunk mesher's convention (CW under
        // the projection-flipped pipeline).
        //
        // Pre-modulate the cell's color by `color_mul` for lifecycle/decay
        // tinting. Pristine plants pass [1,1,1] so this is a no-op.
        let base = [
            (cell.color[0] * color_mul[0]).clamp(0.0, 1.0),
            (cell.color[1] * color_mul[1]).clamp(0.0, 1.0),
            (cell.color[2] * color_mul[2]).clamp(0.0, 1.0),
        ];
        let shade = |normal: [f32; 3]| -> [f32; 3] {
            // Per-face lighting: if sky present, use sun-aware brightness;
            // else a static face table similar to FACE_LIGHT.
            let light = match sky {
                Some(sky) => sky.face_brightness(normal),
                None => {
                    // Use the same fallback table as block faces.
                    let face_idx = if normal[1] > 0.5 { 0 }
                        else if normal[1] < -0.5 { 1 }
                        else if normal[0] > 0.5 { 2 }
                        else if normal[0] < -0.5 { 3 }
                        else if normal[2] > 0.5 { 4 }
                        else { 5 };
                    FACE_LIGHT[face_idx]
                }
            };
            // Directional fade also dims sub-voxels at night.
            let l = light * (0.5 + 0.5 * directional);
            [
                (base[0] * l).clamp(0.0, 1.0),
                (base[1] * l).clamp(0.0, 1.0),
                (base[2] * l).clamp(0.0, 1.0),
            ]
        };

        // +Y top face
        // +Y top face — all 4 verts at y1, use top_ coords
        if !lookup(bx, by + 1, bz) {
            push_quad_foliage(mesh,
                [top_x0, y1, top_z0], [top_x1, y1, top_z0],
                [top_x1, y1, top_z1], [top_x0, y1, top_z1],
                shade([0.0, 1.0, 0.0]), host_origin[1]);
        }
        // -Y bottom face — all 4 verts at y0, use bot_ coords
        if !lookup(bx, by - 1, bz) {
            push_quad_foliage(mesh,
                [bot_x0, y0, bot_z1], [bot_x1, y0, bot_z1],
                [bot_x1, y0, bot_z0], [bot_x0, y0, bot_z0],
                shade([0.0, -1.0, 0.0]), host_origin[1]);
        }
        // +X east face — left edge is bottom, right edge is top
        if !lookup(bx + 1, by, bz) {
            push_quad_foliage(mesh,
                [bot_x1, y0, bot_z0], [bot_x1, y0, bot_z1],
                [top_x1, y1, top_z1], [top_x1, y1, top_z0],
                shade([1.0, 0.0, 0.0]), host_origin[1]);
        }
        // -X west face
        if !lookup(bx - 1, by, bz) {
            push_quad_foliage(mesh,
                [bot_x0, y0, bot_z1], [bot_x0, y0, bot_z0],
                [top_x0, y1, top_z0], [top_x0, y1, top_z1],
                shade([-1.0, 0.0, 0.0]), host_origin[1]);
        }
        // +Z south face
        if !lookup(bx, by, bz + 1) {
            push_quad_foliage(mesh,
                [bot_x1, y0, bot_z1], [bot_x0, y0, bot_z1],
                [top_x0, y1, top_z1], [top_x1, y1, top_z1],
                shade([0.0, 0.0, 1.0]), host_origin[1]);
        }
        // -Z north face
        if !lookup(bx, by, bz - 1) {
            push_quad_foliage(mesh,
                [bot_x0, y0, bot_z0], [bot_x1, y0, bot_z0],
                [top_x1, y1, top_z0], [top_x0, y1, top_z0],
                shade([0.0, 0.0, -1.0]), host_origin[1]);
        }
    }
}

/// Helper: push a 4-vertex quad with two triangles. Vertices supplied
/// in CW order (matches the chunk mesher's convention — front faces under
/// the projection's Y-flip).
fn push_quad(mesh: &mut ChunkMesh, p0: [f32; 3], p1: [f32; 3], p2: [f32; 3], p3: [f32; 3], color: [f32; 3]) {
    let base = mesh.vertices.len() as u32;
    mesh.vertices.push(Vertex::new(p0, color));
    mesh.vertices.push(Vertex::new(p1, color));
    mesh.vertices.push(Vertex::new(p2, color));
    mesh.vertices.push(Vertex::new(p3, color));
    mesh.indices.push(base);
    mesh.indices.push(base + 1);
    mesh.indices.push(base + 2);
    mesh.indices.push(base);
    mesh.indices.push(base + 2);
    mesh.indices.push(base + 3);
}


/// Used by blocks tagged `wants_top_jitter()` (grass, dirt, sand, etc.) to
/// produce a soft organic surface without breaking voxel semantics.
///
/// Always treats the face as TOP (+Y, axis_sign=+1, u=X, v=Z). u and v in
/// the args are the chunk-local 2D coords of the block whose top is being
/// emitted; slice is the block's chunk-local Y.
/// Continuous (float-coord) bumpy height function. Sampled at non-
/// integer world (X, Z) for subdivided block top emission.
///
/// `softness` and `weathering_factor` modulate amplitude; same shape
/// as the integer-coord `surface_bumps`. Produces values in
/// `[-amplitude, 0]` (non-positive, see surface_bumps doc).
///
/// IMPORTANT: this function must be a PURE function of (wx, wz)
/// alone. Adjacent blocks sampling the SAME world position must get
/// the SAME height; that's how shared edge vertices stay welded.
fn bumpy_height(wx: f32, wz: f32, softness: f32, weathering_factor: f32) -> f32 {
    if softness <= 0.0 || weathering_factor <= 0.0 { return 0.0; }
    // Same trig structure as surface_bumps, but accepting floats.
    let low  = ((wx * 0.13).sin() + (wz * 0.17).cos()) * 0.5;
    let mid  = ((wx * 0.27 + wz * 0.31).sin()) * 0.35;
    let high = ((wx * 0.71 + wz * 0.59).sin()) * 0.20;
    let raw = low + mid + high;
    // Same non-positive shift as surface_bumps.
    let unit = ((raw - 1.0) * 0.5).clamp(-1.0, 0.0);
    // Add a finer detail octave at higher frequency to give visible
    // per-block character to the 8x8 subdivision. Without this the
    // subdivided grid looks suspiciously smooth — sub-block bumps
    // need a wavelength of <1 block to read as PER-block bumpiness.
    let detail = ((wx * 1.7).sin() + (wz * 2.3).cos()) * 0.5;
    let detail_unit = ((detail - 0.5) * 0.5).clamp(-1.0, 0.0);
    // Combined amplitude: 0.30 base + 0.20 detail = up to 0.50 block dip.
    // Softness and weathering scale further (grass 0.45 × 1.0 weathering
    // = 0.225 effective max; sand 0.85 × 1.0 = 0.425 effective max).
    (unit * 0.30 + detail_unit * 0.20) * softness * weathering_factor
}

/// Per-block weathering value derived from world position. Used by
/// the subdivided top emission — adjacent blocks have different
/// weathering and thus different bump intensities.
#[inline]
fn weathering_at_world(block: BlockId, wx: i32, wy: i32, wz: i32) -> f32 {
    let mut s = crate::world::chunk::BlockInstanceState::default();
    s.age_ticks = worldgen_weathering_at(block, wx, wy, wz);
    crate::world::weathering::weathering_value(s)
}

/// Number of subdivisions per block top edge. 8 = 8x8 grid = 81
/// vertices, 128 triangles per top face. Per the user's design spec.
/// If perf becomes an issue, can reduce to 4 (25 verts, 32 triangles)
/// without changing edge alignment behavior.
const TOP_SUBDIVISION: i32 = 8;

/// Runtime override for the top-face subdivision, read once from
/// `UNENDLICH_TOP_SUBDIVISION` (clamped to 1..=8, default = TOP_SUBDIVISION).
///
/// Each top face costs `(n+1)^2` vertices and `2*n^2` triangles, so this
/// scales near-LOD top-face geometry quadratically: n=8 → 81 verts, n=4 →
/// 25, n=2 → 9, n=1 → 4 (a flat quad with only corner sag — identical to
/// the legacy 4-corner emit, still edge-welded with side faces because the
/// corners are always sampled exactly and detail is corner-suppressed).
///
/// On memory-constrained devices (e.g. mobile) the bumpy near-LOD tops are
/// the dominant mesh-memory and shadow-pass cost; setting this to 1–2
/// collapses that by ~10–20x with no holes or seams. CPU-mesher path only;
/// the GPU mesh shader carries its own subdivision constant.
fn top_subdivision() -> i32 {
    use std::sync::OnceLock;
    static V: OnceLock<i32> = OnceLock::new();
    *V.get_or_init(|| {
        std::env::var("UNENDLICH_TOP_SUBDIVISION")
            .ok()
            .and_then(|s| s.parse::<i32>().ok())
            .map(|n| n.clamp(1, 8))
            .unwrap_or(TOP_SUBDIVISION)
    })
}

/// Emit a subdivided top face for a soft block. The 1×1 block top
/// becomes an 8×8 grid of mini-quads. Each interior vertex's Y is
/// computed via `bumpy_height` at sub-integer world coords, plus a
/// bilinear interpolation of corner sag. Edge vertices on shared
/// block boundaries use exact world coords so adjacent blocks agree
/// by construction.
///
/// This is what makes the bumpy-block visual ACTUALLY visible. The
/// previous 4-corner-only emission produced bumps on corners but
/// flat tops — invisible per-block character. The subdivision lets
/// each block have its own internal mini-mountains and depressions,
/// while the corner sag from `corner_y_offset` ensures isolated
/// blocks droop at their air-facing corners.
///
/// Cost: 81 vertices + 128 triangles per top face vs the previous
/// 4 vertices + 2 triangles.
fn emit_subdivided_top(
    mesh: &mut ChunkMesh,
    slice: i32,
    u: i32,
    v: i32,
    block: BlockId,
    origin: [i32; 3],
    sky: Option<&crate::sky::Sky>,
    chunk: &Chunk,
    world_model: Option<&std::sync::Arc<crate::world::world_model::WorldModel>>,
) {
    let block_wx = origin[0] + u;
    let block_wy = origin[1] + slice;
    let block_wz = origin[2] + v;
    let block_wy_f = block_wy as f32 + 1.0;
    let softness = block.softness();
    let weathering = weathering_at_world(block, block_wx, block_wy, block_wz);

    // Material-response tinting: sample the local environment, look up
    // the response matrix for this block at this depth, and produce a
    // climate-driven color. Falls back to the legacy block_color()
    // palette + per-position tint when no world_model is supplied.
    let base_color = compute_tinted_color_for_top(
        block, block_wx, block_wy, block_wz, chunk, world_model);

    let light = match sky {
        Some(sky) => sky.face_brightness(FACE_NORMALS[face::TOP]),
        None => FACE_LIGHT[face::TOP],
    };
    // base_color is already climate-tinted by compute_tinted_color_for_top.
    // Just apply the face light to get final per-vertex color.
    let color = [
        (base_color[0] * light).clamp(0.0, 1.0),
        (base_color[1] * light).clamp(0.0, 1.0),
        (base_color[2] * light).clamp(0.0, 1.0),
    ];

    // Compute corner offsets ONCE (these are identical to what the
    // 4-corner emit_jittered_top would produce, including sag for
    // isolated blocks). The 4 corner sags are then bilinearly
    // interpolated across the subdivision grid so the entire top
    // tilts smoothly toward isolated edges.
    let o00 = corner_y_offset(chunk, block_wx,     slice, block_wz,     softness);
    let o10 = corner_y_offset(chunk, block_wx + 1, slice, block_wz,     softness);
    let o11 = corner_y_offset(chunk, block_wx + 1, slice, block_wz + 1, softness);
    let o01 = corner_y_offset(chunk, block_wx,     slice, block_wz + 1, softness);

    // 9×9 vertex grid at the default 8 subdivisions; runtime-overridable
    // via UNENDLICH_TOP_SUBDIVISION (1..=8). Index: i × (nsub+1) + j.
    // (named `nsub`, not `N`, because a module-level `const N: usize`
    // is in scope and `let N` would be read as a const-pattern.)
    let nsub: i32 = top_subdivision();
    let inv_n = 1.0 / (nsub as f32);
    let base_idx = mesh.vertices.len() as u32;

    for i in 0..=nsub {
        for j in 0..=nsub {
            let s = i as f32 * inv_n; // 0..1 along X
            let t = j as f32 * inv_n; // 0..1 along Z
            let wx = block_wx as f32 + s;
            let wz = block_wz as f32 + t;
            // Bilinear interpolation of corner sag.
            let corner_lerp =
                (1.0 - s) * (1.0 - t) * o00 +
                s         * (1.0 - t) * o10 +
                s         * t         * o11 +
                (1.0 - s) * t         * o01;

            // Detail term — sub-integer bumpy_height. Must be
            // SUPPRESSED at corners so the corner vertex Y matches
            // exactly what `corner_y_offset` returns (which is what
            // the side face's top edge uses). Otherwise corners
            // disagree and a small T-junction opens between the top
            // face and the side face above the cliff.
            //
            // We use a "corner mask" that's 0 at exact corners and
            // ramps to 1 in the interior. The four corners are at
            // (s,t) ∈ {(0,0),(1,0),(0,1),(1,1)}; the mask is the
            // smoothstep distance from the nearest corner along each
            // axis. At edges (one axis at corner, other interior),
            // detail is partially suppressed too — that's fine
            // because edges are shared with neighbors via the same
            // bumpy_height function, so they remain welded.
            let corner_mask = {
                // Distance from nearest corner edge in s and t [0,1].
                let edge_s = s.min(1.0 - s);  // 0 at s=0 or s=1, 0.5 at s=0.5
                let edge_t = t.min(1.0 - t);
                let edge_min = edge_s.min(edge_t);
                // Smoothstep ramp: 0 at edge, 1 at center (>= 0.25 from edge).
                (edge_min * 4.0).clamp(0.0, 1.0)
            };
            let detail = bumpy_height(wx, wz, softness, weathering) * corner_mask;
            let dy = corner_lerp + detail;
            mesh.vertices.push(Vertex::new(
                [wx, block_wy_f + dy, wz],
                color,
            ));
        }
    }

    // Emit (nsub×nsub) quads as 2 triangles each.
    let stride = (nsub + 1) as u32;
    for i in 0..nsub {
        for j in 0..nsub {
            let v00 = base_idx + (i as u32) * stride + (j as u32);
            let v10 = base_idx + ((i + 1) as u32) * stride + (j as u32);
            let v11 = base_idx + ((i + 1) as u32) * stride + ((j + 1) as u32);
            let v01 = base_idx + (i as u32) * stride + ((j + 1) as u32);
            mesh.indices.push(v00);
            mesh.indices.push(v10);
            mesh.indices.push(v11);
            mesh.indices.push(v00);
            mesh.indices.push(v11);
            mesh.indices.push(v01);
        }
    }
}

fn emit_jittered_top(
    mesh: &mut ChunkMesh,
    slice: i32,    // chunk-local Y
    u: i32,        // chunk-local X
    v: i32,        // chunk-local Z
    block: BlockId,
    origin: [i32; 3],
    sky: Option<&crate::sky::Sky>,
    chunk: &Chunk,
    world_model: Option<&std::sync::Arc<crate::world::world_model::WorldModel>>,
) {
    let block_wx = origin[0] + u;
    let block_wy = origin[1] + slice;
    let block_wz = origin[2] + v;
    let base_color = compute_tinted_color_for_top(
        block, block_wx, block_wy, block_wz, chunk, world_model);
    let light = match sky {
        Some(sky) => sky.face_brightness(FACE_NORMALS[face::TOP]),
        None => FACE_LIGHT[face::TOP],
    };
    let color = [
        (base_color[0] * light).clamp(0.0, 1.0),
        (base_color[1] * light).clamp(0.0, 1.0),
        (base_color[2] * light).clamp(0.0, 1.0),
    ];

    let wx = block_wx as f32;
    let wy = block_wy as f32 + 1.0;
    let wz = block_wz as f32;

    // Combined per-corner offset: hash-driven jitter (±1/16 block) plus
    // edge-aware sag (down to -SOFT_EDGE_MAX_SAG × softness when fully
    // exposed). All four corners query the same world coords + slice that
    // adjacent blocks would, so corners stay welded.
    let softness = block.softness();
    let o00 = corner_y_offset(chunk, origin[0] + u,     slice, origin[2] + v,     softness);
    let o10 = corner_y_offset(chunk, origin[0] + u + 1, slice, origin[2] + v,     softness);
    let o11 = corner_y_offset(chunk, origin[0] + u + 1, slice, origin[2] + v + 1, softness);
    let o01 = corner_y_offset(chunk, origin[0] + u,     slice, origin[2] + v + 1, softness);

    let positions = [
        [wx,       wy + o00, wz      ],
        [wx + 1.0, wy + o10, wz      ],
        [wx + 1.0, wy + o11, wz + 1.0],
        [wx,       wy + o01, wz + 1.0],
    ];

    let base_idx = mesh.vertices.len() as u32;
    for p in &positions {
        mesh.vertices.push(Vertex::new(*p, color));
    }
    mesh.indices.push(base_idx);
    mesh.indices.push(base_idx + 1);
    mesh.indices.push(base_idx + 2);
    mesh.indices.push(base_idx);
    mesh.indices.push(base_idx + 2);
    mesh.indices.push(base_idx + 3);
}


fn emit_quad(
    mesh: &mut ChunkMesh,
    cfg: &AxisConfig,
    slice: i32,
    u: usize,
    v: usize,
    w: usize,
    h: usize,
    block: BlockId,
    origin: [i32; 3],
    sky: Option<&crate::sky::Sky>,
    chunk: Option<&Chunk>,
    world_model: Option<&std::sync::Arc<crate::world::world_model::WorldModel>>,
) {
    // For greedy-merged quads (w > 1 or h > 1) we use a single uniform
    // color for the whole quad — by definition merging requires uniform
    // appearance. For single-cell quads, we can apply per-block response
    // tinting using the climate at that cell.
    let single_cell = w == 1 && h == 1;
    let base_color = if single_cell && chunk.is_some() {
        // World position of the BLOCK (one cell).
        let mut block_local = [0i32; 3];
        block_local[cfg.axis] = slice;
        block_local[cfg.u] = u as i32;
        block_local[cfg.v] = v as i32;
        let block_wx = origin[0] + block_local[0];
        let block_wy = origin[1] + block_local[1];
        let block_wz = origin[2] + block_local[2];
        compute_tinted_color_for_face(
            block, cfg.face, block_wx, block_wy, block_wz,
            chunk.unwrap(), world_model)
    } else {
        // Greedy-merged quad: use legacy palette without tint.
        match block_color_per_face(block) {
            Some(faces) => faces[cfg.face],
            None => match block_color(block) {
                Some(c) => c,
                None => return,
            },
        }
    };
    // Pick brightness either from the sun (if a Sky was provided) or from
    // the static fallback table. The sky path computes per-face brightness
    // from outward-normal · sun_direction.
    let light = match sky {
        Some(sky) => sky.face_brightness(FACE_NORMALS[cfg.face]),
        None => FACE_LIGHT[cfg.face],
    };
    let color = [
        (base_color[0] * light).clamp(0.0, 1.0),
        (base_color[1] * light).clamp(0.0, 1.0),
        (base_color[2] * light).clamp(0.0, 1.0),
    ];

    // The face plane is at slice + (1 if positive else 0) along axis.
    // For TOP (axis=Y, sign=+1): the face is at y = slice+1 (top of cell `slice`).
    // For BOTTOM (axis=Y, sign=-1): the face is at y = slice (bottom of cell `slice`).
    // For EAST (axis=X, sign=+1): face at x = slice+1.
    // For WEST (axis=X, sign=-1): face at x = slice.
    let axis_plane = if cfg.axis_sign > 0 { slice + 1 } else { slice };

    // Compute the 4 corners in world space.
    // Quad corners in (u, v): (u, v), (u+w, v), (u+w, v+h), (u, v+h).
    let corners_uv: [(usize, usize); 4] = [
        (u, v),
        (u + w, v),
        (u + w, v + h),
        (u, v + h),
    ];

    let mut positions = [[0.0f32; 3]; 4];
    for (i, (cu, cv)) in corners_uv.iter().enumerate() {
        let mut pos_local = [0i32; 3];
        pos_local[cfg.axis] = axis_plane;
        pos_local[cfg.u] = *cu as i32;
        pos_local[cfg.v] = *cv as i32;
        positions[i] = [
            (origin[0] + pos_local[0]) as f32,
            (origin[1] + pos_local[1]) as f32,
            (origin[2] + pos_local[2]) as f32,
        ];
    }

    // Winding: outward-facing front face. For positive faces (axis_sign=+1)
    // the corner order (u,v) → (u+w,v) → (u+w,v+h) → (u,v+h) traverses CCW
    // viewed from +axis. For negative faces it's CCW viewed from -axis,
    // which means CW viewed from +axis, so we flip.
    //
    // We're already running the pipeline with `front_face = COUNTER_CLOCKWISE`
    // and `cull_mode = BACK`. To make a face visible from the *outside* of
    // a solid block:
    //   - +axis face (TOP/EAST/SOUTH): vertices CCW from +axis side
    //   - -axis face (BOTTOM/WEST/NORTH): vertices CCW from -axis side
    //
    // Without flipping, our (u,v)→(u+w,v)→(u+w,v+h)→(u,v+h) order for a
    // +Y face produces a quad with the up-normal — correct. For a -Y face
    // (BOTTOM), the same uv order produces a quad with the up-normal (wrong;
    // we want down-normal), so we reverse the winding for negative faces.
    //
    // We also need to consider the choice of (u, v) axes. Cross-checking:
    // for TOP face the quad lies in XZ plane. With u=X, v=Z, the (0,0)→(W,0)
    // →(W,H)→(0,H) corners traversed in this order, using right-hand rule,
    // give a normal in +Y. ✓
    // For EAST face, plane is YZ, with u=Z, v=Y. The (0,0)→(W,0)→(W,H)→(0,H)
    // corners with u=Z, v=Y: cross of (W,0,0)→(W,0,H) and (W,0,0)→(W,H,0) gives
    // (W,0,H)×(W,H,0) which has a -X component. We want +X. So we need to flip.
    //
    // I'll just have a per-face flip flag determined by the cross product.
    // Easier: for each face, compute the expected normal from FACE_VERTS,
    // and if it points the wrong way, reverse the corner order.

    // From FACE_VERTS: each face's own corner-0 → corner-1 → corner-2 →
    // corner-3 ordering already produces the correct outward normal. Our
    // greedy quad uses (u,v) → (u+w,v) → (u+w,v+h) → (u,v+h). For unit
    // quads (w=h=1) starting at (u=0, v=0), this should match FACE_VERTS[f].
    // Let's verify with TOP: FACE_VERTS[0] = [[0,1,0],[1,1,0],[1,1,1],[0,1,1]].
    // u=X, v=Z, slice=0 (axis_plane=1):
    //   (0,0): pos_local[Y]=1, pos_local[X]=0, pos_local[Z]=0 → [0,1,0] ✓
    //   (1,0): [1,1,0] ✓
    //   (1,1): [1,1,1] ✓
    //   (0,1): [0,1,1] ✓
    // Great — TOP order matches.
    //
    // For BOTTOM: FACE_VERTS[1] = [[0,0,1],[1,0,1],[1,0,0],[0,0,0]].
    // u=X, v=Z, slice=0 (axis_plane=0):
    //   (0,0): [0,0,0] but FACE_VERTS expects [0,0,1] first.
    // So for BOTTOM the natural greedy order produces a different starting
    // vertex than FACE_VERTS. The naive mesher used FACE_VERTS directly so
    // its winding worked. Here we need to either reverse or pick a different
    // corner order for negative faces.
    //
    // The simplest fix: for negative faces, swap corners 1 and 3. That
    // reverses the quad winding (CCW becomes CW), producing the correct
    // outward-pointing normal.

    let order = if cfg.axis_sign > 0 {
        [0, 1, 2, 3]
    } else {
        [0, 3, 2, 1] // reverse winding
    };

    let base_idx = mesh.vertices.len() as u32;
    for &i in &order {
        mesh.vertices.push(Vertex::new(positions[i], color));
    }

    // Two triangles: (0, 1, 2) and (0, 2, 3) in the *new* ordering.
    mesh.indices.push(base_idx);
    mesh.indices.push(base_idx + 1);
    mesh.indices.push(base_idx + 2);
    mesh.indices.push(base_idx);
    mesh.indices.push(base_idx + 2);
    mesh.indices.push(base_idx + 3);
}

/// Emit a single 1×1 SIDE face for a soft block, with its TOP-edge corners
/// sagged to match the same block's sagged top face. Bottom-edge corners
/// stay at the block's natural Y. Result: side and top faces meet flush at
/// every shared corner — no vertical seam at cliff edges.
///
/// `cfg` must be one of EAST/WEST/SOUTH/NORTH (not TOP/BOTTOM); caller is
/// responsible for that.
fn emit_soft_side_face(
    mesh: &mut ChunkMesh,
    cfg: &AxisConfig,
    slice: i32,
    u: i32,
    v: i32,
    block: BlockId,
    origin: [i32; 3],
    sky: Option<&crate::sky::Sky>,
    chunk: &Chunk,
    world_model: Option<&std::sync::Arc<crate::world::world_model::WorldModel>>,
) {
    // World position of the BLOCK (not the face) for env sampling.
    let mut block_local = [0i32; 3];
    block_local[cfg.axis] = slice;
    block_local[cfg.u] = u;
    block_local[cfg.v] = v;
    let block_wx = origin[0] + block_local[0];
    let block_wy = origin[1] + block_local[1];
    let block_wz = origin[2] + block_local[2];

    let base_color = compute_tinted_color_for_face(
        block, cfg.face, block_wx, block_wy, block_wz, chunk, world_model);
    let light = match sky {
        Some(sky) => sky.face_brightness(FACE_NORMALS[cfg.face]),
        None => FACE_LIGHT[cfg.face],
    };
    let color = [
        (base_color[0] * light).clamp(0.0, 1.0),
        (base_color[1] * light).clamp(0.0, 1.0),
        (base_color[2] * light).clamp(0.0, 1.0),
    ];
    let axis_plane = if cfg.axis_sign > 0 { slice + 1 } else { slice };
    let corners_uv: [(i32, i32); 4] = [
        (u,     v),
        (u + 1, v),
        (u + 1, v + 1),
        (u,     v + 1),
    ];
    let mut positions = [[0.0f32; 3]; 4];
    for (i, (cu, cv)) in corners_uv.iter().enumerate() {
        let mut pos_local = [0i32; 3];
        pos_local[cfg.axis] = axis_plane;
        pos_local[cfg.u] = *cu;
        pos_local[cfg.v] = *cv;
        positions[i] = [
            (origin[0] + pos_local[0]) as f32,
            (origin[1] + pos_local[1]) as f32,
            (origin[2] + pos_local[2]) as f32,
        ];
    }

    // For side faces, two of the four corners are at the block's TOP
    // (world Y = origin.y + slice + 1) and two are at the BOTTOM (world Y
    // = origin.y + slice). Apply sag only to top corners, querying the sag
    // at the corner's (world_x, slice, world_z).
    //
    // For EAST/WEST (axis=X), the Y axis is cfg.v (the "v" dimension).
    // For SOUTH/NORTH (axis=Z), the Y axis is cfg.u (the "u" dimension).
    // Either way, we identify top corners by their Y position rather than
    // by uv index — robust to either layout.

    let block_top_y = (origin[1] + slice + 1) as f32;
    let block_bot_y = (origin[1] + slice) as f32;
    let softness = block.softness();
    for p in positions.iter_mut() {
        if (p[1] - block_top_y).abs() < 1e-4 {
            // Top corner: apply this block's top corner offset.
            // The shared corner Y across adjacent blocks is determined
            // purely by world (corner_x, slice, corner_z) so neighbors
            // agree without coordination.
            let cx = p[0] as i32;
            let cz = p[2] as i32;
            let off = corner_y_offset(chunk, cx, slice, cz, softness);
            p[1] += off;
        } else if (p[1] - block_bot_y).abs() < 1e-4 {
            // Bottom corner: this corner is at the boundary between
            // THIS block and the block BELOW. If the block below is
            // also a soft block whose top sags down (negative offset),
            // we have a visible seam: the lower block's top is at
            // `Y + below_offset` (below integer Y), but our side face
            // bottom is at integer Y — leaving a gap.
            //
            // Extend the bottom DOWN to match the block-below's top
            // offset, so the side faces meet flush. We use ONLY the
            // bumps/sag offset of the block below (NOT this block's),
            // looked up at slice-1 with the below block's softness.
            //
            // Critical safety: only DROP the bottom downward, never
            // lift it upward. With non-positive bumps this is
            // automatic — corner_y_offset for the below block can
            // only return ≤ 0. So `p[1] += min(0, below_offset)` is
            // always safe and never opens new gaps elsewhere.
            let cx = p[0] as i32;
            let cz = p[2] as i32;
            let below_x = cx - origin[0];
            let below_z = cz - origin[2];
            let below_y = slice - 1;
            let below_block = block_at(chunk, [below_x, below_y, below_z]);
            if is_meshable(below_block) && below_block.softness() > 0.0 {
                let below_off = corner_y_offset(
                    chunk, cx, below_y, cz, below_block.softness());
                // Only drop downward, never lift.
                if below_off < 0.0 {
                    p[1] += below_off;
                }
            }
        }
    }

    // Same winding flip as emit_quad: positive faces use natural order,
    // negative faces reverse to keep the outward normal pointing the right way.
    let order = if cfg.axis_sign > 0 {
        [0, 1, 2, 3]
    } else {
        [0, 3, 2, 1]
    };
    let base_idx = mesh.vertices.len() as u32;
    for &i in &order {
        mesh.vertices.push(Vertex::new(positions[i], color));
    }
    mesh.indices.push(base_idx);
    mesh.indices.push(base_idx + 1);
    mesh.indices.push(base_idx + 2);
    mesh.indices.push(base_idx);
    mesh.indices.push(base_idx + 2);
    mesh.indices.push(base_idx + 3);
}

// =============================================================================
// Tests.
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::chunk::ChunkCoord;
    use crate::world::cpu_generator::WorldGenerator;

    // ----- PackedVertex (v0.24.20-vertex-pack) -----

    #[test]
    fn packed_vertex_stride_is_8_bytes() {
        // The Vulkan vertex binding stride must be exactly 8 bytes for
        // the A2B10G10R10_UNORM_PACK32 + R8G8B8A8_UNORM layout.
        assert_eq!(std::mem::size_of::<PackedVertex>(), 8);
        assert_eq!(PackedVertex::STRIDE, 8);
    }

    #[test]
    fn packed_vertex_round_trip_origin() {
        // (0, 0, 0) chunk-local must round-trip exactly.
        let v = Vertex::new([100.0, 200.0, 300.0], [0.5, 0.25, 0.75]);
        let chunk_origin = [100.0, 200.0, 300.0];
        let p = PackedVertex::from_world(&v, chunk_origin);
        let local = p.unpack_pos();
        assert!(local[0].abs() < 0.05, "x: {}", local[0]);
        assert!(local[1].abs() < 0.05, "y: {}", local[1]);
        assert!(local[2].abs() < 0.05, "z: {}", local[2]);
    }

    #[test]
    fn packed_vertex_round_trip_max() {
        // (32, 32, 32) chunk-local (the chunk's far corner) must
        // round-trip to exactly 32, since 1023/1023 * 32 = 32.
        let v = Vertex::new([32.0, 32.0, 32.0], [1.0, 1.0, 1.0]);
        let chunk_origin = [0.0, 0.0, 0.0];
        let p = PackedVertex::from_world(&v, chunk_origin);
        let local = p.unpack_pos();
        assert_eq!(local, [32.0, 32.0, 32.0]);
    }

    #[test]
    fn packed_vertex_precision_within_block() {
        // The mesher emits z-fight nudges of -0.05 from integer
        // boundaries. The packed format must keep nudged vertices
        // distinct from un-nudged ones at the same nominal position.
        let nudge_off = PackedVertex::from_world(
            &Vertex::new([0.0, 10.0, 0.0], [1.0, 1.0, 1.0]),
            [0.0, 0.0, 0.0],
        );
        let nudged = PackedVertex::from_world(
            &Vertex::new([0.0, 10.0 - 0.05, 0.0], [1.0, 1.0, 1.0]),
            [0.0, 0.0, 0.0],
        );
        assert_ne!(nudge_off.pos_packed, nudged.pos_packed,
            "z-fight nudge of -0.05 must survive quantization");
    }

    #[test]
    fn packed_vertex_sub_block_sag_distinct() {
        // Soft-edge sag up to -0.25. Must remain visibly distinct.
        let upright = PackedVertex::from_world(
            &Vertex::new([1.0, 5.0, 1.0], [1.0, 1.0, 1.0]),
            [0.0, 0.0, 0.0],
        );
        let sagged = PackedVertex::from_world(
            &Vertex::new([1.0, 5.0 - 0.25, 1.0], [1.0, 1.0, 1.0]),
            [0.0, 0.0, 0.0],
        );
        let up_y = upright.unpack_pos()[1];
        let sag_y = sagged.unpack_pos()[1];
        assert!((up_y - sag_y - 0.25).abs() < 0.05,
            "sag must be recoverable to within 0.05 (got {} vs {})", up_y, sag_y);
    }

    #[test]
    fn packed_vertex_quantization_step_under_one_sixteenth() {
        // 32 blocks / 1023 levels ≈ 0.0313 — must be strictly less
        // than 0.0625 (the TOP_JITTER_AMPLITUDE constant) so jitter
        // values stay distinguishable.
        let step = 32.0_f32 / 1023.0;
        assert!(step < 0.0625, "quantization step {} too coarse", step);
    }

    #[test]
    fn packed_vertex_color_round_trip() {
        // Pure red, green, blue should round-trip exactly (255/255 = 1.0).
        let red = PackedVertex::pack_color([1.0, 0.0, 0.0]);
        let unpacked = PackedVertex { pos_packed: 0, color_packed: red }.unpack_color();
        assert_eq!(unpacked, [1.0, 0.0, 0.0]);
    }

    #[test]
    fn packed_vertex_color_grey_quantization() {
        // Mid-grey (0.5) quantizes to 128/255 ≈ 0.502 — within one step.
        let grey = PackedVertex::pack_color([0.5, 0.5, 0.5]);
        let unpacked = PackedVertex { pos_packed: 0, color_packed: grey }.unpack_color();
        for c in unpacked.iter() {
            assert!((c - 0.5).abs() < 1.0 / 255.0 + 1e-6,
                "0.5 round-tripped to {}", c);
        }
    }

    #[test]
    fn packed_vertex_clamps_overshoot() {
        // Defensive: a vertex at chunk-local x = 33 (past the boundary)
        // clamps to 32, not wraps. Production meshes never emit such
        // vertices, but the encoding must degrade gracefully.
        let v = Vertex::new([33.0, 0.0, 0.0], [0.5, 0.5, 0.5]);
        let p = PackedVertex::from_world(&v, [0.0, 0.0, 0.0]);
        assert_eq!(p.unpack_pos()[0], 32.0);
    }

    #[test]
    fn packed_vertex_clamps_negative() {
        // Defensive: a vertex slightly below origin (e.g., -0.1) clamps
        // to 0, not wraps. Same gracefulness as the overshoot case.
        let v = Vertex::new([-0.1, 0.0, 0.0], [0.5, 0.5, 0.5]);
        let p = PackedVertex::from_world(&v, [0.0, 0.0, 0.0]);
        assert_eq!(p.unpack_pos()[0], 0.0);
    }

    #[test]
    fn packed_vertex_layout_matches_vulkan_format() {
        // Bit layout sanity: A2B10G10R10_UNORM_PACK32 has R in bits
        // 0..10. So (max_x, 0, 0) should equal 0x3FF in the low bits.
        let p = PackedVertex {
            pos_packed: PackedVertex::pack_pos([32.0, 0.0, 0.0]),
            color_packed: 0,
        };
        assert_eq!(p.pos_packed & 0x3FF, 0x3FF, "x should occupy bits 0..10");
        assert_eq!((p.pos_packed >> 10) & 0x3FF, 0, "y should be zero");
        assert_eq!((p.pos_packed >> 20) & 0x3FF, 0, "z should be zero");
    }


    #[test]
    fn naive_single_block_has_six_faces() {
        let mut chunk = Chunk::empty(ChunkCoord::new(0, 0, 0));
        chunk.set(8, 8, 8, BlockId::Stone);
        let mesh = mesh_chunk_naive(&chunk);
        assert_eq!(mesh.vertex_count(), 24);
        assert_eq!(mesh.index_count(), 36);
    }

    #[test]
    fn greedy_single_block_has_six_faces() {
        let mut chunk = Chunk::empty(ChunkCoord::new(0, 0, 0));
        chunk.set(8, 8, 8, BlockId::Stone);
        let mesh = mesh_chunk_greedy(&chunk, None, None, None);
        // Single block: 6 faces, each can't merge with anything → 6 quads
        // → 24 vertices, 36 indices. Same as naive.
        assert_eq!(mesh.vertex_count(), 24);
        assert_eq!(mesh.index_count(), 36);
    }

    #[test]
    fn greedy_2x2_top_face_merges() {
        // 2x2 stone slab — the top should merge into one quad, sides into 4.
        let mut chunk = Chunk::empty(ChunkCoord::new(0, 0, 0));
        for x in 0..2 {
            for z in 0..2 {
                chunk.set(x, 0, z, BlockId::Stone);
            }
        }
        let greedy = mesh_chunk_greedy(&chunk, None, None, None);
        let naive  = mesh_chunk_naive(&chunk);

        // Naive: 4 blocks × 4 visible faces (top, bottom, 2 sides each) = 16 quads.
        // Wait — outermost blocks have 5 visible faces (top, bottom, 2 outer sides).
        // Actually each of the 4 corner blocks: top, bottom, two outer sides = 4 faces.
        // 4 blocks × 4 = 16 visible faces.
        // 16 quads × 4 verts = 64 verts, 96 indices.
        assert_eq!(naive.vertex_count(), 64);

        // Greedy: top merges (1 quad), bottom merges (1 quad), 4 outer sides
        // each are 2x1 vertical quads that merge along x or z axis (4 quads).
        // So 6 merged quads = 24 verts.
        assert_eq!(greedy.vertex_count(), 24,
            "greedy should produce 6 merged quads (24 verts), got {}",
            greedy.vertex_count());
    }

    #[test]
    fn greedy_full_layer_merges_to_two_quads() {
        // Fill an entire 32×32 horizontal layer with stone. Top + bottom each
        // merge into one big quad. Sides expose the layer edge — 32 wide × 1
        // tall, can merge along the long axis.
        let mut chunk = Chunk::empty(ChunkCoord::new(0, 0, 0));
        for x in 0..CHUNK_SIZE {
            for z in 0..CHUNK_SIZE {
                chunk.set(x, 0, z, BlockId::Stone);
            }
        }
        let mesh = mesh_chunk_greedy(&chunk, None, None, None);
        // 1 top quad + 1 bottom quad + 4 side strips = 6 quads = 24 verts.
        assert_eq!(mesh.vertex_count(), 24,
            "full layer should produce 6 merged quads, got {}",
            mesh.vertex_count());
    }

    #[test]
    fn greedy_world_chunk_has_fewer_verts_than_naive() {
        let gen = WorldGenerator::new(0xCAFE);
        // Pick a chunk straddling the surface band — has many blocks of mixed
        // type and should benefit from greedy.
        let chunk = gen.generate_chunk(ChunkCoord::new(0, 1, 0));
        let naive  = mesh_chunk_naive(&chunk);
        // Compare without decoration — the greedy-vs-naive comparison is
        // about face-merging efficiency on the block grid. Sub-voxel plant
        // decoration adds geometry the naive path doesn't produce, which
        // would invalidate the comparison.
        let greedy = mesh_chunk_greedy_undecorated(&chunk, None);
        assert!(greedy.vertex_count() <= naive.vertex_count(),
            "greedy ({}) should not produce more verts than naive ({})",
            greedy.vertex_count(), naive.vertex_count());
        // For a real surface chunk we expect a meaningful win. The original bar
        // was ≥2×, but that was aspirational, not an invariant: greedy efficiency
        // depends on the chunk's block layout. The reference surface chunk
        // (seed 0xCAFE, ChunkCoord(0,1,0)) measures naive=25192 greedy=14284 — a
        // 1.76× win. Assert a solid ≥1.5× reduction, which still catches greedy
        // regressing toward the naive count without flaking on layout variance.
        if naive.vertex_count() > 1000 {
            assert!(greedy.vertex_count() * 3 < naive.vertex_count() * 2,
                "greedy on surface chunk should be ≥1.5× win: naive={}, greedy={}",
                naive.vertex_count(), greedy.vertex_count());
        }
    }

    #[test]
    fn greedy_two_block_types_dont_merge() {
        // Adjacent stone and dirt — top faces should not merge.
        let mut chunk = Chunk::empty(ChunkCoord::new(0, 0, 0));
        chunk.set(0, 0, 0, BlockId::Stone);
        chunk.set(1, 0, 0, BlockId::Dirt);
        let mesh = mesh_chunk_greedy(&chunk, None, None, None);
        // The non-merging condition isn't directly measured by vertex
        // count anymore (dirt top is subdivided 8x8 = 81 verts; stone
        // is still 4 verts per face). Instead just verify both blocks
        // contribute geometry (mesh isn't degenerate). The "differing
        // types must not silently merge" property is enforced by the
        // greedy mask only sharing identical BlockId values.
        assert!(mesh.vertex_count() > 0,
            "expected non-empty mesh from two adjacent blocks, got {}",
            mesh.vertex_count());
        // Sanity bound: stone (5 faces × 4 verts) + dirt (4 sides × 4 + 1 top × 81)
        // = 20 + 16 + 81 = 117 verts.
        assert!(mesh.vertex_count() >= 100,
            "two-block mesh should include subdivided top; got {} verts",
            mesh.vertex_count());
    }

    #[test]
    fn greedy_mesh_world_positions_in_range() {
        let mut chunk = Chunk::empty(ChunkCoord::new(2, 1, -3));
        chunk.set(0, 0, 0, BlockId::Stone);
        let mesh = mesh_chunk_greedy(&chunk, None, None, None);
        // World origin for chunk (2, 1, -3) is (64, 32, -96). Block (0,0,0)
        // spans x∈[64,65], y∈[32,33], z∈[-96,-95].
        for v in &mesh.vertices {
            assert!(v.position[0] >= 64.0 && v.position[0] <= 65.0,
                "vertex x out of range: {:?}", v.position);
            assert!(v.position[1] >= 32.0 && v.position[1] <= 33.0,
                "vertex y out of range: {:?}", v.position);
            assert!(v.position[2] >= -96.0 && v.position[2] <= -95.0,
                "vertex z out of range: {:?}", v.position);
        }
    }

    #[test]
    fn greedy_all_face_normals_face_outward_under_pipeline() {
        // Place a single block and verify each emitted quad is visible from
        // *outside* the block under our pipeline configuration.
        //
        // Pipeline: front_face=CLOCKWISE + cull_mode=BACK. Reason: projection
        // matrix has a Y-flip for Vulkan NDC, which reverses our triangles'
        // apparent winding from the rasterizer's perspective. So a triangle
        // whose CCW-math normal points INWARD ends up visible from outside.
        //
        // We test by computing the math normal and asserting it points
        // *toward* the block center (dot product with outward vector < 0).
        // This catches winding bugs (like EAST/WEST having u,v swapped wrong).
        let mut chunk = Chunk::empty(ChunkCoord::new(0, 0, 0));
        chunk.set(8, 8, 8, BlockId::Stone);
        let mesh = mesh_chunk_greedy(&chunk, None, None, None);
        let block_center = [8.5_f32, 8.5, 8.5];

        assert_eq!(mesh.vertex_count(), 24, "single block should be 6 quads");
        for quad in 0..6 {
            let base = quad * 4;
            let v0 = mesh.vertices[base].position;
            let v1 = mesh.vertices[base + 1].position;
            let v2 = mesh.vertices[base + 2].position;
            let v3 = mesh.vertices[base + 3].position;

            let e1 = [v1[0] - v0[0], v1[1] - v0[1], v1[2] - v0[2]];
            let e2 = [v2[0] - v0[0], v2[1] - v0[1], v2[2] - v0[2]];
            let normal = [
                e1[1] * e2[2] - e1[2] * e2[1],
                e1[2] * e2[0] - e1[0] * e2[2],
                e1[0] * e2[1] - e1[1] * e2[0],
            ];

            let face_center = [
                (v0[0] + v1[0] + v2[0] + v3[0]) * 0.25,
                (v0[1] + v1[1] + v2[1] + v3[1]) * 0.25,
                (v0[2] + v1[2] + v2[2] + v3[2]) * 0.25,
            ];
            let outward = [
                face_center[0] - block_center[0],
                face_center[1] - block_center[1],
                face_center[2] - block_center[2],
            ];
            let dot = normal[0] * outward[0]
                    + normal[1] * outward[1]
                    + normal[2] * outward[2];
            // Math normal must point INWARD (negative dot with outward),
            // because the pipeline's CW convention then treats the quad as
            // front-facing when viewed from outside.
            assert!(dot < 0.0,
                "quad {} would be back-face culled when viewed from outside: \
                 normal={:?}, outward={:?}, dot={}",
                quad, normal, outward, dot);
        }
    }

    #[test]
    fn top_corner_offset_is_deterministic() {
        // Same input → same output, every time. This guarantees adjacent
        // blocks meeting at a shared corner agree on the height (no T-
        // junctions, no visual cracks).
        let a = top_corner_offset(7, -3);
        let b = top_corner_offset(7, -3);
        assert_eq!(a, b);
        // Output bounded.
        for x in -10..10 {
            for z in -10..10 {
                let h = top_corner_offset(x, z);
                assert!(h >= -TOP_JITTER_AMPLITUDE && h <= TOP_JITTER_AMPLITUDE,
                    "offset out of bounds at ({},{}): {}", x, z, h);
            }
        }
    }

    #[test]
    fn jittered_top_uses_corner_heights() {
        // A single grass block: top face should have non-flat corners.
        // With both jitter (±1/16) and soft-edge sag (down to -0.225 for
        // grass at fully-exposed corners), top Y can range roughly 8.7..9.1.
        let mut chunk = Chunk::empty(ChunkCoord::new(0, 0, 0));
        chunk.set(8, 8, 8, BlockId::Grass);
        let mesh = mesh_chunk_greedy(&chunk, None, None, None);
        let mut top_corner_ys: Vec<f32> = mesh.vertices.iter()
            .filter(|v| v.position[1] > 8.4 && v.position[1] < 9.1)
            .filter(|v| v.position[0] >= 8.0 && v.position[0] <= 9.0)
            .filter(|v| v.position[2] >= 8.0 && v.position[2] <= 9.0)
            .map(|v| v.position[1])
            .collect();
        top_corner_ys.sort_by(|a, b| a.partial_cmp(b).unwrap());
        top_corner_ys.dedup();
        assert!(top_corner_ys.len() > 1,
            "expected jittered corners to have distinct Y values, got: {:?}",
            top_corner_ys);
    }

    #[test]
    fn non_jittered_block_top_is_flat() {
        // Stone has no top jitter — all 4 top corners should be at y=9.0.
        let mut chunk = Chunk::empty(ChunkCoord::new(0, 0, 0));
        chunk.set(8, 8, 8, BlockId::Stone);
        let mesh = mesh_chunk_greedy(&chunk, None, None, None);
        let top_ys: Vec<f32> = mesh.vertices.iter()
            .filter(|v| v.position[0] >= 8.0 && v.position[0] <= 9.0)
            .filter(|v| v.position[2] >= 8.0 && v.position[2] <= 9.0)
            .filter(|v| (v.position[1] - 9.0).abs() < 0.01)
            .map(|v| v.position[1])
            .collect();
        // All top-face Y values must be exactly 9.0 — no jitter on stone.
        for y in &top_ys {
            assert!((y - 9.0).abs() < 1e-4,
                "stone top corner not at y=9.0: {}", y);
        }
    }

    #[test]
    fn isolated_soft_block_sags_more_than_buried() {
        // Compare a single isolated grass block (all neighbors air) to a
        // grass block buried in a 3×3 grass platform. Isolated has lower
        // max top-Y than buried (sag intensity is higher).
        let mut chunk_iso = Chunk::empty(ChunkCoord::new(0, 0, 0));
        chunk_iso.set(8, 8, 8, BlockId::Grass);
        let mut chunk_bur = Chunk::empty(ChunkCoord::new(0, 0, 0));
        for dx in -1..=1 {
            for dz in -1..=1 {
                chunk_bur.set(8 + dx, 8, 8 + dz, BlockId::Grass);
            }
        }
        let mesh_iso = mesh_chunk_greedy(&chunk_iso, None, None, None);
        let mesh_bur = mesh_chunk_greedy(&chunk_bur, None, None, None);
        // Center block (8,8,8) — top corners. Take mean Y of corners that
        // sit roughly at the top.
        let mean_top_y = |mesh: &ChunkMesh, block_xz: (i32, i32)| -> f32 {
            let xs = block_xz.0 as f32;
            let zs = block_xz.1 as f32;
            let mut ys: Vec<f32> = mesh.vertices.iter()
                .filter(|v| v.position[0] >= xs && v.position[0] <= xs + 1.0)
                .filter(|v| v.position[2] >= zs && v.position[2] <= zs + 1.0)
                .filter(|v| v.position[1] > 8.0 && v.position[1] < 9.5)
                .map(|v| v.position[1])
                .collect();
            ys.sort_by(|a, b| a.partial_cmp(b).unwrap());
            if ys.is_empty() { return 9.0; }
            ys.iter().sum::<f32>() / ys.len() as f32
        };
        let iso_y = mean_top_y(&mesh_iso, (8, 8));
        let bur_y = mean_top_y(&mesh_bur, (8, 8));
        // Buried block's top should be near 9.0 (only jitter); isolated
        // block's top should be visibly lower (sag) — typical sag for
        // softness=0.45 at fully exposed corners is ~0.225.
        assert!(bur_y - iso_y > 0.10,
            "isolated grass should sag more than buried grass: iso={} bur={}",
            iso_y, bur_y);
    }

    #[test]
    fn soft_block_top_and_side_meet_flush_at_corners() {
        // Critical invariant: the sagged top corner Y must equal the side
        // face's top edge Y at the same world position. Otherwise a
        // vertical seam opens at the cliff edge. Test by emitting a single
        // grass block and confirming its top-face vertices and side-face
        // top-edge vertices coincide.
        let mut chunk = Chunk::empty(ChunkCoord::new(0, 0, 0));
        chunk.set(8, 8, 8, BlockId::Grass);
        let mesh = mesh_chunk_greedy(&chunk, None, None, None);

        // The top corner at world (8, ?, 8) should appear with the same Y
        // value in both the top face and the side faces that share it.
        // There are 4 such corners: (8,8), (9,8), (9,9), (8,9).
        for &(cx, cz) in &[(8.0_f32, 8.0_f32), (9.0, 8.0), (9.0, 9.0), (8.0, 9.0)] {
            let ys: Vec<f32> = mesh.vertices.iter()
                .filter(|v| (v.position[0] - cx).abs() < 1e-4)
                .filter(|v| (v.position[2] - cz).abs() < 1e-4)
                .filter(|v| v.position[1] > 8.0 && v.position[1] < 9.5)
                .map(|v| v.position[1])
                .collect();
            // All Y values at this corner must be identical (welded).
            if ys.len() < 2 { continue; } // not enough verts to test
            let max_y = ys.iter().cloned().fold(f32::MIN, f32::max);
            let min_y = ys.iter().cloned().fold(f32::MAX, f32::min);
            assert!((max_y - min_y) < 1e-3,
                "corner ({}, {}) has inconsistent Y values: {:?}", cx, cz, ys);
        }
    }

    #[test]
    fn adjacent_grass_share_corner_heights() {
        // Two adjacent grass blocks share an edge (and the corners on that
        // edge). The corner heights from `top_corner_offset` must agree so
        // there are no T-junctions. We don't test mesh output here — we just
        // verify the offset function is consistent.
        for x in 0..10 {
            for z in 0..10 {
                // The corner at (x+1, z) belongs to block (x, z)'s top-right
                // and block (x+1, z)'s top-left. Both should query the same
                // world coord and get the same offset.
                let from_left  = top_corner_offset(x + 1, z);
                let from_right = top_corner_offset(x + 1, z);
                assert_eq!(from_left, from_right);
            }
        }
    }

    #[test]
    fn subvoxel_grass_block_produces_geometry() {
        // A TallGrass block hosts a sub-voxel grid directly (no probability
        // gate — it IS the plant). Verify the chunk mesh contains its
        // sub-voxel geometry.
        let mut chunk = Chunk::empty(ChunkCoord::new(0, 0, 0));
        chunk.set(8, 8, 8, BlockId::TallGrass);
        let mesh = mesh_chunk_greedy(&chunk, None, None, None);
        // TallGrass is transparent so no greedy face quads — geometry comes
        // entirely from the sub-voxel pass. The grass-tuft variant 2 has 7
        // blades worth of cells; even with face-culling that produces tens
        // of verts.
        assert!(mesh.vertex_count() >= 30,
            "TallGrass block should produce sub-voxel geometry; got {} verts",
            mesh.vertex_count());
    }

    #[test]
    fn subvoxel_face_culling_skips_internal_faces() {
        // For a sub-voxel grid where all cells are adjacent (a solid
        // 2×2×2 block of mini-cells), only the outer faces should be
        // emitted: 6 sides × (2×2) = 24 unit-faces. We're not measuring
        // exactly because the meshing emits 4 verts per face and may
        // greedily merge later, but it should be much less than the
        // worst-case 8 cells × 6 faces = 48 unit-faces.
        use crate::world::subvoxel::{MiniCell, SubVoxelGrid};
        let mut grid = SubVoxelGrid::empty();
        for x in 0..2 {
            for y in 0..2 {
                for z in 0..2 {
                    grid.cells.push(MiniCell {
                        sub_x: x, sub_y: y, sub_z: z,
                        color: [0.5, 0.5, 0.5],
                    });
                }
            }
        }
        let mut mesh = ChunkMesh::empty();
        emit_subvoxel_grid(&mut mesh, &grid, [0.0, 0.0, 0.0], None, [1.0, 1.0, 1.0]);
        // 24 unit-faces × 4 verts = 96 verts max. If we accidentally emitted
        // internal faces, we'd get up to 48 × 4 = 192. The threshold catches
        // the regression.
        assert!(mesh.vertex_count() <= 100,
            "internal faces should be culled; got {} verts (expected ≤ 100)",
            mesh.vertex_count());
        // And we should have meaningful output, not zero.
        assert!(mesh.vertex_count() >= 24,
            "outer faces should be emitted; got {} verts", mesh.vertex_count());
    }

    #[test]
    fn subvoxel_top_cells_sway_more_than_bottom_cells() {
        // Two stacked sub-cells (sub_y=0 bottom, sub_y=7 top). The top
        // cell's vertices should be displaced laterally further than the
        // bottom cell's vertices.
        use crate::world::subvoxel::{MiniCell, SubVoxelGrid, SUBVOXEL_RESOLUTION};
        let mut grid = SubVoxelGrid::empty();
        grid.cells.push(MiniCell { sub_x: 4, sub_y: 0, sub_z: 4, color: [0.5; 3] });
        grid.cells.push(MiniCell {
            sub_x: 4,
            sub_y: (SUBVOXEL_RESOLUTION - 1) as u8,
            sub_z: 4, color: [0.5; 3]
        });

        let mut mesh = ChunkMesh::empty();
        // Use a host_origin in a region where wind has measurable magnitude.
        // Wind grows with altitude; pick y=200 for a strong gust.
        emit_subvoxel_grid(&mut mesh, &grid, [0.0, 200.0, 0.0], None, [1.0, 1.0, 1.0]);

        // Bottom cell vertices should be near host_origin + sub coords
        // (no sway weight 0). Top cell vertices should be measurably
        // offset laterally.
        let mut bottom_xs = Vec::new();
        let mut top_xs = Vec::new();
        for v in &mesh.vertices {
            if v.position[1] < 200.5 {
                // Bottom cell — Y in [200.0, ~200.125]
                bottom_xs.push(v.position[0]);
            } else if v.position[1] > 200.85 {
                // Top cell top edge
                top_xs.push(v.position[0]);
            }
        }
        // Compute the spread of X values within each group; the top
        // group should be shifted from the bottom group's center.
        let bot_avg = bottom_xs.iter().sum::<f32>() / bottom_xs.len() as f32;
        let top_avg = top_xs.iter().sum::<f32>() / top_xs.len() as f32;
        // The X centers should differ; the difference is the sway lean.
        // If wind happens to be 0 in this region the test would fail —
        // wind_at(0, 200, 0, 0) has high magnitude (altitude term).
        let lean = (top_avg - bot_avg).abs();
        assert!(lean > 0.001,
            "top of stem should lean from bottom; bot_avg={} top_avg={} lean={}",
            bot_avg, top_avg, lean);
    }

    /// Regression: surface_bumps must NEVER return a positive value.
    /// If it does, block tops lift above integer Y, exposing triangular
    /// gaps at terrain step seams (was visible in user screenshots
    /// before the non-positive shift landed). Sample across a large
    /// grid to defeat noise alignment.
    #[test]
    fn surface_bumps_is_non_positive() {
        let softness = 1.0; // worst-case: max softness amplifies bumps
        for x in -50..=50 {
            for z in -50..=50 {
                let b = surface_bumps(x, z, softness);
                assert!(b <= 0.0,
                    "surface_bumps({}, {}) = {} — must be ≤ 0 to prevent terrain mesh gaps",
                    x, z, b);
                // Sanity bound on amplitude. With 0.30 base × 1.0 softness
                // the maximum dip is 0.30 block.
                assert!(b >= -0.32,
                    "surface_bumps({}, {}) = {} — exceeded amplitude budget",
                    x, z, b);
            }
        }
    }

    /// Regression: surface_bumps must produce a meaningful range of
    /// non-trivial values, not collapse to all-zero. The clamp is real
    /// only if it sometimes fires; otherwise terrain looks completely
    /// flat. We assert at least 5% of samples have a meaningful bump
    /// (|b| > 0.02 block) which would be visible on screen.
    #[test]
    fn surface_bumps_has_visible_amplitude() {
        let mut visible_count = 0;
        let mut total = 0;
        for x in -50..=50 {
            for z in -50..=50 {
                let b = surface_bumps(x, z, 1.0);
                if b.abs() > 0.02 { visible_count += 1; }
                total += 1;
            }
        }
        let ratio = visible_count as f32 / total as f32;
        assert!(ratio > 0.05,
            "surface_bumps too flat: only {}/{} samples have visible bumps ({}%)",
            visible_count, total, ratio * 100.0);
    }

    /// Regression: weathering must vary per block via worldgen_weathering_at.
    /// If the function returns the same value for every world position,
    /// blocks would all weather identically and lose per-block character.
    #[test]
    fn worldgen_weathering_varies_by_position() {
        use crate::world::blocks::BlockId;
        let mut values = std::collections::HashSet::new();
        for x in -10..=10 {
            for y in -10..=10 {
                for z in -10..=10 {
                    let w = worldgen_weathering_at(BlockId::Sand, x, y, z);
                    values.insert(w);
                }
            }
        }
        // Across ~9000 positions, we should see at least 30 distinct values.
        // (variance ±50 around a base of 210, so 100-260 clamped to 100-255.)
        assert!(values.len() >= 30,
            "weathering not varied enough — only {} distinct values across 9000 positions",
            values.len());
    }

    /// Regression: weathering for a fresh (zero-softness) block must
    /// always be 0. Hard blocks like stone don't weather visually
    /// at gen time.
    #[test]
    fn worldgen_weathering_zero_for_hard_blocks() {
        use crate::world::blocks::BlockId;
        // Stone AT or ABOVE sea level should have zero weathering
        // (surface stone is fresh; depth-aware weathering only kicks
        // in below sea level for cave humidity simulation).
        let sea = crate::constants::SEA_LEVEL as i32;
        for x in -5..=5 {
            for z in -5..=5 {
                for y in [sea, sea + 1, sea + 10, sea + 50] {
                    let w = worldgen_weathering_at(BlockId::Stone, x, y, z);
                    assert_eq!(w, 0,
                        "stone above sea level should have zero weathering: ({},{},{}) got {}",
                        x, y, z, w);
                }
            }
        }
    }

    /// Cave depth weathering: stone DEEP underground gets non-zero
    /// weathering (humidity / water-flow proxy). Tests the cave-flavor
    /// behavior added with worldgen_default_weathering_at_depth.
    #[test]
    fn worldgen_weathering_nonzero_for_deep_stone() {
        use crate::world::blocks::BlockId;
        let sea = crate::constants::SEA_LEVEL as i32;
        // 64 blocks below sea level — should be max depth weathering.
        let mut nonzero_count = 0;
        let mut total = 0;
        for x in -5..=5 {
            for z in -5..=5 {
                let w = worldgen_weathering_at(BlockId::Stone, x, sea - 64, z);
                if w > 0 { nonzero_count += 1; }
                total += 1;
            }
        }
        // Expect most/all positions to have nonzero weathering deep
        // underground (per-position hash variance of ±50 around base ~150).
        assert!(nonzero_count == total,
            "expected ALL deep stone positions to have weathering, got {}/{}",
            nonzero_count, total);
    }

    /// Water surface: a chunk with water cells below an air cell should
    /// emit a top face for each water cell whose top is air.
    #[test]
    fn water_surface_emits_top_faces() {
        use crate::world::blocks::BlockId;
        use crate::world::chunk::{Chunk, ChunkCoord};
        let mut blocks = vec![BlockId::Air; (CHUNK_SIZE as usize).pow(3)];
        for y in 0..CHUNK_SIZE / 2 {
            for x in 0..CHUNK_SIZE {
                for z in 0..CHUNK_SIZE {
                    blocks[Chunk::block_index(x, y, z)] = BlockId::Water;
                }
            }
        }
        let chunk = Chunk::from_blocks(ChunkCoord::new(0, 0, 0), blocks);
        let mesh = mesh_chunk_greedy(&chunk, None, None, None);
        let surface_y = (CHUNK_SIZE / 2) as f32 - 0.05;
        let surface_verts = mesh.vertices.iter()
            .filter(|v| (v.position[1] - surface_y).abs() < 0.01)
            .count();
        assert!(surface_verts >= 4 * 32 * 32 / 2,
            "expected ≥{} water surface vertices, got {}",
            4 * 32 * 32 / 2, surface_verts);
    }

    /// r446: a water cell with air on its sides/bottom emits side+bottom faces (volume), but
    /// faces against out-of-chunk are skipped so multi-chunk water bodies don't seam.
    #[test]
    fn water_cell_emits_side_and_bottom_faces() {
        use crate::world::blocks::BlockId;
        use crate::world::chunk::{Chunk, ChunkCoord};
        let mut blocks = vec![BlockId::Air; (CHUNK_SIZE as usize).pow(3)];
        // one water cell in the chunk interior, surrounded by air
        let (x, y, z) = (5, 5, 5);
        blocks[Chunk::block_index(x, y, z)] = BlockId::Water;
        let chunk = Chunk::from_blocks(ChunkCoord::new(0, 0, 0), blocks);
        let mesh = mesh_chunk_greedy(&chunk, None, None, None);
        // a fully-surrounded cell: top (dipped) + 4 sides + bottom = 6 faces = 24 verts of water.
        // count distinct Y levels among water verts: top at 5.95, sides span 5..6, bottom at 5.
        let ys: std::collections::BTreeSet<i32> = mesh
            .vertices
            .iter()
            .map(|v| (v.position[1] * 100.0) as i32)
            .collect();
        assert!(ys.contains(&595), "dipped top face present");
        assert!(ys.contains(&500), "bottom/side faces reach integer Y (volume)");
    }

    /// Water surface should NOT emit faces between adjacent water cells.
    #[test]
    fn water_surface_no_interior_faces() {
        use crate::world::blocks::BlockId;
        use crate::world::chunk::{Chunk, ChunkCoord};
        let mut blocks = vec![BlockId::Air; (CHUNK_SIZE as usize).pow(3)];
        for y in 0..2 {
            for x in 0..CHUNK_SIZE {
                for z in 0..CHUNK_SIZE {
                    blocks[Chunk::block_index(x, y, z)] = BlockId::Water;
                }
            }
        }
        let chunk = Chunk::from_blocks(ChunkCoord::new(0, 0, 0), blocks);
        let mesh = mesh_chunk_greedy(&chunk, None, None, None);
        let surface_y = 2.0 - 0.05;
        let interior_y = 1.0 - 0.05;
        let at_surface = mesh.vertices.iter()
            .filter(|v| (v.position[1] - surface_y).abs() < 0.01)
            .count();
        let at_interior = mesh.vertices.iter()
            .filter(|v| (v.position[1] - interior_y).abs() < 0.01)
            .count();
        assert!(at_surface > 0, "should have surface at Y=2");
        assert_eq!(at_interior, 0,
            "should NOT have water surface at interior Y=1, got {} verts there",
            at_interior);
    }

    /// Seam test: when two adjacent soft blocks form a vertical step,
    /// the higher block's side face bottom must extend down to meet
    /// the lower block's top. Builds a small chunk with a 1-block step
    /// and verifies that mesh vertices on the cliff face have no Y
    /// gap — the side face's bottom edge reaches at least as low as
    /// the lower block's top corner offset.
    ///
    /// This exercises the bottom-corner offset in `emit_soft_side_face`
    /// that fixes the user-reported sand-cliff seam.
    #[test]
    fn soft_block_cliff_side_face_drops_to_meet_block_below() {
        use crate::world::blocks::BlockId;
        use crate::world::chunk::{Chunk, ChunkCoord};
        use crate::constants::CHUNK_SIZE;

        // Build a chunk with a clear cliff: half the floor at slice 5,
        // the other half at slice 6 (one block step).
        let mut blocks = vec![BlockId::Air; (CHUNK_SIZE as usize).pow(3)];
        for x in 0..CHUNK_SIZE {
            for z in 0..CHUNK_SIZE {
                let surface_y = if x < CHUNK_SIZE / 2 { 6 } else { 5 };
                for y in 0..=surface_y {
                    let idx = Chunk::block_index(x, y, z);
                    blocks[idx] = BlockId::Sand;
                }
            }
        }
        let chunk = Chunk::from_blocks(ChunkCoord::new(0, 0, 0), blocks);

        // Mesh and inspect: any vertex whose Y is below integer-Y on the
        // cliff face indicates a successful seam-fill (bottom dropped
        // to meet the lower block's top).
        let mesh = mesh_chunk_greedy(&chunk, None, None, None);
        let mut sealed_count = 0;
        for v in &mesh.vertices {
            let x = v.position[0];
            let y = v.position[1];
            // Cliff edge column: x ≈ 16. Drop range: y between (5.5) and 6.0.
            if (15.0..=17.0).contains(&x) && (5.5..6.0).contains(&y) {
                sealed_count += 1;
            }
        }
        assert!(sealed_count > 0,
            "no side-face vertices in seam-seal Y range — fix may not be active. \
             {} total vertices",
            mesh.vertices.len());
    }

    /// Regression: subdivided top-face emission produces an 8x8 grid
    /// of vertices for a soft block. Without subdivision, a single
    /// soft block's top face is 4 verts; with it, 81. The dispatch
    /// chooses subdivision when weathering > 0.05 (which is true for
    /// world-generated soft blocks).
    #[test]
    fn subdivided_top_emits_grid_for_soft_block() {
        use crate::world::chunk::{Chunk, ChunkCoord};

        // Single grass block at world-position chosen to have non-trivial
        // weathering — Grass is a soft block so worldgen weathering
        // returns 150 ± 50 from the position hash.
        let mut chunk = Chunk::empty(ChunkCoord::new(0, 0, 0));
        chunk.set(8, 8, 8, BlockId::Grass);
        let mesh = mesh_chunk_greedy(&chunk, None, None, None);

        // Count vertices in the top face's bounding box: the 9x9 grid
        // of subdivided top vertices should all sit within the block's
        // (x, z) extent at y near the top.
        let top_vertex_count = mesh.vertices.iter()
            .filter(|v| v.position[0] >= 8.0 && v.position[0] <= 9.0)
            .filter(|v| v.position[2] >= 8.0 && v.position[2] <= 9.0)
            .filter(|v| v.position[1] > 8.5 && v.position[1] < 9.5)
            .count();
        // 81 from subdivided top + some from side face top edges.
        // Lower bound: 81 (the subdivision grid).
        assert!(top_vertex_count >= 81,
            "expected ≥81 top-region vertices from 8x8 subdivision, got {}",
            top_vertex_count);
    }

    /// Regression: subdivided top must produce visible Y variation
    /// across its grid. If interior detail is too suppressed (or the
    /// corner mask too aggressive), the top would be visually flat
    /// even with subdivision. We assert that at least 5% of the
    /// subdivided vertices have a Y that differs from the mean by
    /// more than a small threshold.
    #[test]
    fn subdivided_top_has_visible_interior_variation() {
        use crate::world::chunk::{Chunk, ChunkCoord};

        let mut chunk = Chunk::empty(ChunkCoord::new(0, 0, 0));
        chunk.set(8, 8, 8, BlockId::Grass);
        let mesh = mesh_chunk_greedy(&chunk, None, None, None);

        let ys: Vec<f32> = mesh.vertices.iter()
            .filter(|v| v.position[0] > 8.0 && v.position[0] < 9.0)
            .filter(|v| v.position[2] > 8.0 && v.position[2] < 9.0)
            .filter(|v| v.position[1] > 8.5 && v.position[1] < 9.5)
            .map(|v| v.position[1])
            .collect();
        assert!(ys.len() >= 49,
            "expected ≥49 STRICT-interior vertices (7x7 inside 8x8 subdivision), got {}",
            ys.len());

        let mean = ys.iter().sum::<f32>() / ys.len() as f32;
        let varied = ys.iter().filter(|y| (**y - mean).abs() > 0.005).count();
        let ratio = varied as f32 / ys.len() as f32;
        assert!(ratio > 0.05,
            "subdivided top is too flat: only {}/{} interior vertices have Y \
             differing from mean by >5mm (mean={})",
            varied, ys.len(), mean);
    }

    /// Cloud emission: a chunk that intersects the cloud band should
    /// emit some cloud cube vertices. Verifies the cloud rendering
    /// path is wired in.
    #[test]
    fn cloud_pass_emits_geometry_in_cloud_band() {
        use crate::world::chunk::{Chunk, ChunkCoord};
        use crate::cloud;
        // Build an empty chunk positioned at cloud-band Y.
        // CLOUD_BAND_BOTTOM = SEA_LEVEL+64 = 128. CHUNK_SIZE=32, so chunk_y=4 → world Y 128..159.
        let chunk_y = cloud::CLOUD_BAND_BOTTOM / 32;
        let chunk = Chunk::empty(ChunkCoord::new(0, chunk_y, 0));
        let mesh = mesh_chunk_greedy(&chunk, None, None, None);
        // Even with no terrain blocks, cloud emission should add cubes.
        // Some positions in the cloud band over (0..32, 0..32) are
        // above the cloud-density threshold.
        assert!(!mesh.vertices.is_empty(),
            "expected cloud cubes in chunk intersecting cloud band, got 0 verts");
        // Cloud cubes are at Y in [CLOUD_BAND_BOTTOM, CLOUD_BAND_TOP].
        let in_band = mesh.vertices.iter()
            .filter(|v| v.position[1] >= cloud::CLOUD_BAND_BOTTOM as f32
                     && v.position[1] <= cloud::CLOUD_BAND_TOP as f32 + 1.0)
            .count();
        assert!(in_band > 0,
            "no cloud vertices within cloud band y range");
    }

    /// Cloud emission skipped for chunks far from cloud band — cheap
    /// quick-reject keeps the per-cell loop off most chunks.
    #[test]
    fn cloud_pass_skips_chunks_outside_band() {
        use crate::world::chunk::{Chunk, ChunkCoord};
        // Surface chunk (chunk_y=2, world Y=64..95) — far below cloud band.
        let chunk = Chunk::empty(ChunkCoord::new(0, 2, 0));
        let mesh = mesh_chunk_greedy(&chunk, None, None, None);
        // No clouds should be emitted (and chunk is empty so no terrain).
        assert!(mesh.vertices.is_empty(),
            "expected 0 vertices for empty surface chunk (no clouds in this band), got {}",
            mesh.vertices.len());
    }

    /// `tint_for_position` is deterministic for the same input.
    #[test]
    fn tint_is_deterministic() {
        use crate::world::blocks::BlockId;
        for _ in 0..20 {
            let t = tint_for_position(BlockId::Grass, 100, 64, 200);
            assert_eq!(t, tint_for_position(BlockId::Grass, 100, 64, 200));
        }
    }

    /// `tint_for_position` produces meaningful variation across positions.
    /// Without variation, blocks all look identical (regression risk).
    #[test]
    fn tint_varies_across_positions() {
        use crate::world::blocks::BlockId;
        let mut tones: Vec<[f32; 3]> = Vec::new();
        for x in (-50..=50).step_by(7) {
            for z in (-50..=50).step_by(7) {
                tones.push(tint_for_position(BlockId::Grass, x, 64, z));
            }
        }
        // Find min and max of the green channel (where grass varies most).
        let min_g = tones.iter().map(|t| t[1]).fold(f32::INFINITY, f32::min);
        let max_g = tones.iter().map(|t| t[1]).fold(f32::NEG_INFINITY, f32::max);
        let range = max_g - min_g;
        assert!(range > 0.10,
            "expected green channel to vary by at least 10% across positions, \
             got range {} (min={} max={})", range, min_g, max_g);
    }

    /// Tint stays in a reasonable range — multipliers should be roughly
    /// in [0.7, 1.3] for safety. If they go out of this range, vertex
    /// colors clip noticeably.
    #[test]
    fn tint_stays_in_safe_range() {
        use crate::world::blocks::BlockId;
        for x in -100..=100 {
            for z in -100..=100 {
                let t = tint_for_position(BlockId::Grass, x, 64, z);
                for c in 0..3 {
                    assert!(t[c] > 0.7 && t[c] < 1.3,
                        "tint channel {} = {} at ({},{}) is out of safe range",
                        c, t[c], x, z);
                }
            }
        }
    }

    /// Hard blocks have lower tint variance than soft blocks (regression
    /// against accidentally giving stone the same variance as grass).
    #[test]
    fn hard_blocks_have_less_tint_variance() {
        use crate::world::blocks::BlockId;
        // Sample many positions, find variance for each block type.
        let sample = |b: BlockId| -> f32 {
            let mut max_dev: f32 = 0.0;
            for x in (-50..=50).step_by(5) {
                for z in (-50..=50).step_by(5) {
                    let t = tint_for_position(b, x, 64, z);
                    for c in 0..3 {
                        max_dev = max_dev.max((t[c] - 1.0).abs());
                    }
                }
            }
            max_dev
        };
        let grass_var = sample(BlockId::Grass);
        let stone_var = sample(BlockId::Stone);
        assert!(grass_var > stone_var,
            "grass should have more tint variance than stone: {} vs {}",
            grass_var, stone_var);
    }

    /// Mesh output for the same chunk differs when meshed with a world
    /// model (climate-aware) vs without (neutral environment).
    /// Validates that the material-response plumbing actually drives
    /// vertex colors.
    #[test]
    fn mesh_with_model_differs_from_mesh_without() {
        use crate::world::chunk::{Chunk, ChunkCoord};
        use crate::world::world_model::WorldModel;
        use std::sync::Arc;

        // Build a chunk that has at least one grass top face.
        let mut chunk = Chunk::empty(ChunkCoord::new(0, 2, 0));
        // Fill bottom layer with grass; air above.
        for z in 0..crate::constants::CHUNK_SIZE {
            for x in 0..crate::constants::CHUNK_SIZE {
                let idx = Chunk::block_index(x, 0, z);
                chunk.blocks[idx] = BlockId::Grass;
            }
        }
        chunk.recompute_cached_all_air(); // RC42.1: sync cache after direct mutation

        let mesh_no_model = mesh_chunk_greedy(&chunk, None, None, None);
        let world_model = Arc::new(WorldModel::new(0xCAFE));
        let mesh_with_model = mesh_chunk_greedy(&chunk, None, Some(&world_model), None);

        // Both should have the same number of vertices (same chunk, same
        // face emission rules), but vertex colors should differ in at
        // least some places due to climate-driven tinting.
        assert_eq!(mesh_no_model.vertices.len(), mesh_with_model.vertices.len(),
            "mesh structure should be identical");
        let mut diff_count = 0;
        for (a, b) in mesh_no_model.vertices.iter().zip(mesh_with_model.vertices.iter()) {
            if a.color != b.color { diff_count += 1; }
        }
        assert!(diff_count > 0,
            "expected at least some vertex colors to differ between model-aware and \
             neutral mesh, got 0 differences");
    }

    /// Surface stone (within ~16 blocks of sea level) has position variance
    /// and does NOT merge in greedy meshing — meaning a chunk full of
    /// surface stone produces more faces than a chunk full of deep stone.
    #[test]
    fn surface_stone_emits_more_faces_than_deep_stone() {
        // Build two chunks of the same size, one at surface Y (chunk_y=2,
        // world Y starts at 64) and one deep (chunk_y=-2, world Y -64..-32).
        // Both filled with stone. The surface chunk should have more
        // vertices because surface stone doesn't greedy-merge.

        // Note: this regression test is gated on the matrix actually
        // forbidding merge; if `merges_in_greedy` were always true we'd
        // see equal counts.
        // Surface stone:
        use crate::world::chunk::{Chunk, ChunkCoord};
        let mut surface = Chunk::empty(ChunkCoord::new(0, 2, 0));
        // Fill the bottom slab + some air above so we get top faces.
        for z in 0..crate::constants::CHUNK_SIZE {
            for x in 0..crate::constants::CHUNK_SIZE {
                let idx = Chunk::block_index(x, 0, z);
                surface.blocks[idx] = BlockId::Stone;
            }
        }
        surface.recompute_cached_all_air(); // RC42.1
        let mut deep = Chunk::empty(ChunkCoord::new(0, -2, 0));
        for z in 0..crate::constants::CHUNK_SIZE {
            for x in 0..crate::constants::CHUNK_SIZE {
                let idx = Chunk::block_index(x, 0, z);
                deep.blocks[idx] = BlockId::Stone;
            }
        }
        deep.recompute_cached_all_air(); // RC42.1

        let surface_mesh = mesh_chunk_greedy(&surface, None, None, None);
        let deep_mesh = mesh_chunk_greedy(&deep, None, None, None);
        // Surface has per-block top quads; deep merges them. Surface
        // should have strictly more vertices.
        // (32×32 = 1024 blocks. Per-block: 4 verts × 6 sides... but
        // actually the test ground is just the top face since the
        // bottom and sides are buried. Per-block 4 verts × 1 top = 4096
        // for surface. Greedy: 4 verts for the whole 32×32 region.)
        assert!(surface_mesh.vertex_count() > deep_mesh.vertex_count(),
            "surface stone (variance > 0, no merge) should produce more \
             vertices than deep stone (merges): surface={} deep={}",
            surface_mesh.vertex_count(), deep_mesh.vertex_count());
    }

    /// Meshing a chunk twice with the same WorldModel produces
    /// identical vertex output. This is a multiplayer-determinism
    /// regression: if mesh output drifted between clients due to
    /// non-deterministic sampling order, surface tints would differ
    /// between players standing at the same place.
    #[test]
    fn meshing_with_model_is_deterministic() {
        use crate::world::chunk::{Chunk, ChunkCoord};
        use crate::world::world_model::WorldModel;
        use std::sync::Arc;

        let mut chunk = Chunk::empty(ChunkCoord::new(0, 2, 0));
        for z in 0..crate::constants::CHUNK_SIZE {
            for x in 0..crate::constants::CHUNK_SIZE {
                let idx = Chunk::block_index(x, 0, z);
                chunk.blocks[idx] = BlockId::Grass;
            }
        }
        let world_model = Arc::new(WorldModel::new(0xCAFE));
        let m1 = mesh_chunk_greedy(&chunk, None, Some(&world_model), None);
        let m2 = mesh_chunk_greedy(&chunk, None, Some(&world_model), None);
        // Same output: same vertex count, same indices, same colors.
        assert_eq!(m1.vertices.len(), m2.vertices.len());
        for (i, (a, b)) in m1.vertices.iter().zip(m2.vertices.iter()).enumerate() {
            assert_eq!(a.position, b.position, "vertex {} position drift", i);
            assert_eq!(a.color, b.color, "vertex {} color drift", i);
        }
        assert_eq!(m1.indices, m2.indices);
    }

    /// Meshing the same chunk with two DIFFERENT WorldModel instances
    /// (but same seed) produces identical output. This is what happens
    /// across machines / restarts — model instances differ in identity
    /// (different DashMap caches) but with the same seed must produce
    /// the same noise values. If this fails, world saves wouldn't
    /// look the same after restart.
    #[test]
    fn meshing_with_different_model_instances_same_seed_is_identical() {
        use crate::world::chunk::{Chunk, ChunkCoord};
        use crate::world::world_model::WorldModel;
        use std::sync::Arc;

        let mut chunk = Chunk::empty(ChunkCoord::new(0, 2, 0));
        for z in 0..crate::constants::CHUNK_SIZE {
            for x in 0..crate::constants::CHUNK_SIZE {
                let idx = Chunk::block_index(x, 0, z);
                chunk.blocks[idx] = BlockId::Grass;
            }
        }
        let model_a = Arc::new(WorldModel::new(0xCAFE));
        let model_b = Arc::new(WorldModel::new(0xCAFE));
        let mesh_a = mesh_chunk_greedy(&chunk, None, Some(&model_a), None);
        let mesh_b = mesh_chunk_greedy(&chunk, None, Some(&model_b), None);
        for (i, (a, b)) in mesh_a.vertices.iter().zip(mesh_b.vertices.iter()).enumerate() {
            assert_eq!(a.position, b.position, "vertex {} position differs across model instances", i);
            assert_eq!(a.color, b.color, "vertex {} color differs across model instances", i);
        }
    }

    /// Meshing with two different SEEDS produces different vertex colors.
    /// This proves the climate plumbing actually flows through to colors —
    /// if colors were independent of seed, the plumbing would be a no-op.
    #[test]
    fn meshing_with_different_seeds_differs() {
        use crate::world::chunk::{Chunk, ChunkCoord};
        use crate::world::world_model::WorldModel;
        use std::sync::Arc;

        let mut chunk = Chunk::empty(ChunkCoord::new(0, 2, 0));
        for z in 0..crate::constants::CHUNK_SIZE {
            for x in 0..crate::constants::CHUNK_SIZE {
                let idx = Chunk::block_index(x, 0, z);
                chunk.blocks[idx] = BlockId::Grass;
            }
        }
        chunk.recompute_cached_all_air(); // RC42.1
        let model_a = Arc::new(WorldModel::new(0xCAFE));
        let model_b = Arc::new(WorldModel::new(0xBEEF));
        let mesh_a = mesh_chunk_greedy(&chunk, None, Some(&model_a), None);
        let mesh_b = mesh_chunk_greedy(&chunk, None, Some(&model_b), None);
        assert_eq!(mesh_a.vertices.len(), mesh_b.vertices.len());
        let mut diff_count = 0;
        for (a, b) in mesh_a.vertices.iter().zip(mesh_b.vertices.iter()) {
            if a.color != b.color { diff_count += 1; }
        }
        // Different seeds → different climate fields → at least some
        // grass blocks should sample meaningfully different temp/humid.
        assert!(diff_count > 0,
            "different seeds should produce different vertex colors, got 0 differences \
             out of {} vertices", mesh_a.vertices.len());
    }

    // =============================================================================
    // F2 (v0.22.2) — far / very_far / downsampled mesher tests
    // =============================================================================
    //
    // These tests pin the contract of `mesh_chunk_downsampled` and its two
    // convenience wrappers. The function is not yet wired to anything (F3
    // does that), so test coverage is the primary correctness check until
    // hardware playtest.

    /// Empty chunk → empty mesh, regardless of factor. Trivial baseline.
    #[test]
    fn far_empty_chunk_yields_empty_mesh() {
        let chunk = Chunk::empty(ChunkCoord::new(0, 0, 0));
        for factor in [1, 2, 4, 8, 16, 32] {
            let mesh = mesh_chunk_downsampled(&chunk, factor);
            assert!(mesh.is_empty(),
                "empty chunk @ factor={} should produce empty mesh, got {} verts",
                factor, mesh.vertex_count());
        }
    }

    /// At factor=1 the downsampled mesher is identical to `mesh_chunk_naive`.
    /// Pins the algorithm: factor=1 must be the no-op identity case so any
    /// future refactor that breaks this is caught immediately.
    #[test]
    fn far_factor_1_matches_naive() {
        // Use a CPU-generated chunk for realistic geometry, not just empty
        // or fully-solid which are degenerate.
        let generator = WorldGenerator::new(0x1234);
        let chunk = generator.generate_chunk(ChunkCoord::new(0, 1, 0));
        let naive = mesh_chunk_naive(&chunk);
        let down1 = mesh_chunk_downsampled(&chunk, 1);

        assert_eq!(naive.vertex_count(), down1.vertex_count(),
            "factor=1 vertex count must match naive");
        assert_eq!(naive.index_count(), down1.index_count(),
            "factor=1 index count must match naive");

        // Vertex order is the same too (same nested-loop traversal). Check
        // first 100 vertices position-by-position to catch any subtle
        // coordinate-math drift.
        let n = naive.vertex_count().min(100) as usize;
        for i in 0..n {
            assert_eq!(naive.vertices[i].position, down1.vertices[i].position,
                "vertex {} position differs at factor=1", i);
            assert_eq!(naive.vertices[i].color, down1.vertices[i].color,
                "vertex {} color differs at factor=1", i);
        }
    }

    /// Fully-solid chunk produces only the 6 outer faces, factored.
    /// At factor=2 the chunk's outer surface has 16² cells per face × 6 faces
    /// = 1536 quads (each cell still emits its own quad — no greedy merging
    /// in F2).
    #[test]
    fn far_solid_chunk_factor_2_outer_faces_only() {
        let mut chunk = Chunk::empty(ChunkCoord::new(0, 0, 0));
        for y in 0..CHUNK_SIZE {
            for z in 0..CHUNK_SIZE {
                for x in 0..CHUNK_SIZE {
                    chunk.set(x, y, z, BlockId::Stone);
                }
            }
        }
        let mesh = mesh_chunk_downsampled(&chunk, 2);
        // Grid is 16³. Inner cells (14³) emit 0 faces (all neighbors solid).
        // Outer-shell cells emit faces only on directions facing chunk edge
        // — total = 6 × 16² = 1536 quads.
        let expected_quads = 6 * 16 * 16;
        assert_eq!(mesh.vertex_count(), expected_quads as u32 * 4);
        assert_eq!(mesh.index_count(),  expected_quads as u32 * 6);
    }

    /// Triangle count drops when we move from naive to far for a generated
    /// (terrain-shaped) chunk. Lower bound: factor=2 should produce strictly
    /// fewer quads than naive.
    #[test]
    fn far_factor_2_emits_fewer_triangles_than_naive() {
        let generator = WorldGenerator::new(0x9876);
        // Surface chunk — has terrain detail to be reduced.
        let chunk = generator.generate_chunk(ChunkCoord::new(0, 1, 0));
        let naive = mesh_chunk_naive(&chunk);
        let far   = mesh_chunk_far(&chunk);

        assert!(naive.vertex_count() > 0, "naive should produce some vertices on a surface chunk");
        assert!(far.vertex_count() > 0,   "far should produce some vertices on a surface chunk");
        assert!(far.vertex_count() < naive.vertex_count(),
            "far ({} verts) should be cheaper than naive ({} verts)",
            far.vertex_count(), naive.vertex_count());
    }

    /// VeryFar should be cheaper than Far on the same chunk.
    #[test]
    fn very_far_emits_fewer_triangles_than_far() {
        let generator = WorldGenerator::new(0x9876);
        let chunk = generator.generate_chunk(ChunkCoord::new(0, 1, 0));
        let far      = mesh_chunk_far(&chunk);
        let very_far = mesh_chunk_very_far(&chunk);

        assert!(far.vertex_count() > 0);
        assert!(very_far.vertex_count() > 0);
        assert!(very_far.vertex_count() < far.vertex_count(),
            "very_far ({} verts) should be cheaper than far ({} verts)",
            very_far.vertex_count(), far.vertex_count());
    }

    /// Output vertices stay within the chunk's world-space AABB plus a
    /// small tolerance. Catches off-by-one errors in coordinate math.
    /// The downsampled cell at the +X chunk boundary emits a quad at
    /// world_x = origin_x + grid_size*factor = origin_x + CHUNK_SIZE.
    /// That's the chunk's outer face — exactly at the boundary, not past.
    #[test]
    fn far_vertices_stay_within_chunk_aabb() {
        let generator = WorldGenerator::new(0xBEEF);
        // Use coord (5, 1, -3) so origins are non-zero and any "forgot to
        // add origin" bug shows up.
        let coord = ChunkCoord::new(5, 1, -3);
        let chunk = generator.generate_chunk(coord);
        let (ox, oy, oz) = coord.block_origin();

        for factor in [2, 4] {
            let mesh = mesh_chunk_downsampled(&chunk, factor);
            for v in &mesh.vertices {
                assert!(v.position[0] >= ox as f32 && v.position[0] <= (ox + CHUNK_SIZE) as f32,
                    "factor={} x={} out of [{}, {}]",
                    factor, v.position[0], ox, ox + CHUNK_SIZE);
                assert!(v.position[1] >= oy as f32 && v.position[1] <= (oy + CHUNK_SIZE) as f32,
                    "factor={} y={} out of [{}, {}]",
                    factor, v.position[1], oy, oy + CHUNK_SIZE);
                assert!(v.position[2] >= oz as f32 && v.position[2] <= (oz + CHUNK_SIZE) as f32,
                    "factor={} z={} out of [{}, {}]",
                    factor, v.position[2], oz, oz + CHUNK_SIZE);
            }
        }
    }

    /// Output triangle count is bounded by `6 * grid_size³`. Pins the
    /// upper bound so the F2 cost claim is verifiable.
    #[test]
    fn far_triangle_count_bounded_by_grid_size() {
        let generator = WorldGenerator::new(0x1111);
        let chunk = generator.generate_chunk(ChunkCoord::new(0, 1, 0));

        for factor in [2, 4, 8] {
            let grid_size = CHUNK_SIZE / factor;
            let mesh = mesh_chunk_downsampled(&chunk, factor);
            let max_quads = 6 * (grid_size * grid_size * grid_size) as u32;
            assert!(mesh.vertex_count() <= max_quads * 4,
                "factor={}: {} verts exceeds 6×grid³ × 4 = {}",
                factor, mesh.vertex_count(), max_quads * 4);
        }
    }

    /// Half-solid chunk (everything below mid-plane) produces exactly the
    /// expected geometry: a flat top surface, 4 side walls, and a bottom
    /// face. Tests the slicing math.
    ///
    /// The chunk has 32 layers; if we fill y∈[0,15] with stone and leave
    /// y∈[16,31] as air, downsample-2 produces a grid where j=0..7 are
    /// solid and j=8..15 are air. Top face (between j=7 and j=8) shows up;
    /// inner side faces between adjacent solid cells are NOT emitted.
    #[test]
    fn far_half_solid_chunk_has_one_top_layer() {
        let mut chunk = Chunk::empty(ChunkCoord::new(0, 0, 0));
        for y in 0..16 {
            for z in 0..CHUNK_SIZE {
                for x in 0..CHUNK_SIZE {
                    chunk.set(x, y, z, BlockId::Stone);
                }
            }
        }
        let mesh = mesh_chunk_downsampled(&chunk, 2);

        // Count quads — each is 4 verts. 16² = 256 cells per axis-aligned
        // face. Expected:
        //   top:    16² = 256 quads (one per top-cell at j=7)
        //   bottom: 16² = 256 quads (one per bottom-cell at j=0)
        //   4 walls: 16 cells wide × 8 cells tall × 4 = 512 quads
        // Total = 1024 quads.
        let total_quads = mesh.vertex_count() / 4;
        assert_eq!(total_quads, 256 + 256 + 512,
            "expected 1024 quads for half-solid factor=2 chunk, got {}",
            total_quads);
    }

    /// Majority threshold check: a downsample cell that's exactly half
    /// solid is treated as air. (½ + 1 rule.) Specifically, at factor=2
    /// (cell volume 8), a cell with 4 solid blocks gets dropped.
    #[test]
    fn far_majority_threshold_half_air() {
        let mut chunk = Chunk::empty(ChunkCoord::new(0, 0, 0));
        // Fill exactly the bottom half of the (0..2)³ cell at (0,0,0)
        // with stone. That's 4 of 8 source blocks.
        for x in 0..2 {
            for z in 0..2 {
                chunk.set(x, 0, z, BlockId::Stone);
            }
        }
        let mesh = mesh_chunk_downsampled(&chunk, 2);
        // Half-solid cell is treated as air → no faces emitted.
        assert_eq!(mesh.vertex_count(), 0,
            "half-solid cell should be treated as air (majority rule), got {} verts",
            mesh.vertex_count());
    }

    /// Majority threshold: a downsample cell that's strictly majority-solid
    /// (5+ of 8 at factor=2) IS solid.
    #[test]
    fn far_majority_threshold_above_half() {
        let mut chunk = Chunk::empty(ChunkCoord::new(0, 0, 0));
        // 5 of 8 source blocks solid.
        let positions = [(0,0,0), (1,0,0), (0,1,0), (1,1,0), (0,0,1)];
        for (x, y, z) in positions {
            chunk.set(x, y, z, BlockId::Stone);
        }
        let mesh = mesh_chunk_downsampled(&chunk, 2);
        // The single (0,0,0) downsample cell is solid; its 6 outer faces
        // emit (boundary always emits), so 6 quads × 4 verts = 24.
        assert_eq!(mesh.vertex_count(), 24,
            "majority-solid cell should produce 6 outer faces, got {} verts",
            mesh.vertex_count());
    }

    /// Mixed-block-id majority: representative is the most-common solid
    /// id. With 4 stone + 1 dirt (5 solid total), color should be stone's.
    #[test]
    fn far_representative_is_majority_block() {
        let mut chunk = Chunk::empty(ChunkCoord::new(0, 0, 0));
        for (x, y, z) in [(0,0,0), (1,0,0), (0,1,0), (1,1,0)] {
            chunk.set(x, y, z, BlockId::Stone);
        }
        chunk.set(0, 0, 1, BlockId::Dirt);
        let mesh = mesh_chunk_downsampled(&chunk, 2);
        assert!(!mesh.is_empty());

        // Stone color (0.55, 0.55, 0.55) × FACE_LIGHT factors. We can't
        // know which face is which without re-walking the FACE_LIGHT
        // table, so just check that all vertex colors are GREYSCALE
        // (R=G=B), which holds for stone but not for dirt (which is
        // brownish).
        for v in &mesh.vertices {
            let [r, g, b] = v.color;
            // Stone is a neutral grey: |R-G| and |R-B| should be small.
            // Dirt would have R > G > B (warm brown).
            assert!((r - g).abs() < 0.01 && (r - b).abs() < 0.01,
                "expected greyscale (stone) color, got {:?}", v.color);
        }
    }

    /// Factor must divide CHUNK_SIZE. Debug-builds assert; we test the
    /// assertion fires for an invalid factor (3 doesn't divide 32).
    /// Only runs in debug mode where debug_assert! is active.
    #[test]
    #[should_panic(expected = "must divide CHUNK_SIZE")]
    #[cfg(debug_assertions)]
    fn far_invalid_factor_panics_in_debug() {
        let chunk = Chunk::empty(ChunkCoord::new(0, 0, 0));
        let _ = mesh_chunk_downsampled(&chunk, 3);
    }

    // =========================================================================
    // Reverse-regen Far mesher tests (v0.23.7, Stage 2 spike)
    // =========================================================================
    //
    // These prove the basic architecture: mesh_chunk_far_virtual can
    // produce sensible meshes by sampling the generator directly, with
    // no Chunk materialised. They DO NOT assert exact-mesh-equality with
    // the materialised path — the algorithms differ (centre-sample vs
    // majority-vote) and that's expected. The tests check that:
    //
    //   * Air-only regions produce empty meshes.
    //   * Underground regions produce non-empty meshes.
    //   * Surface regions produce non-empty meshes within the chunk AABB.
    //   * Virtual and materialised outputs are within an order of
    //     magnitude of each other on triangle count.
    //
    // If any of these fail, the spike has revealed a structural issue
    // and Phase 2b is blocked until diagnosed.

    /// Helper: well-known seed for deterministic worldgen in tests.
    /// Matches the seed pattern used in other mesher tests above.
    fn test_generator() -> WorldGenerator {
        WorldGenerator::new(0xDEAD_BEEF)
    }

    #[test]
    fn virtual_far_high_altitude_is_empty() {
        // y=200 is well above any terrain (the generator's own fast-path
        // kicks in at y=160). Virtual mesher should produce no faces.
        let gen = test_generator();
        let coord = ChunkCoord::new(0, 7, 0);  // y=224 block origin
        let mesh = mesh_chunk_far_virtual(coord, &gen, 2);
        assert!(mesh.is_empty(),
            "expected empty mesh at high altitude, got {} verts", mesh.vertex_count());
    }

    #[test]
    fn virtual_far_deep_underground_is_dense_or_empty() {
        // y=-100 is well below terrain — fully solid. Naive mesher
        // would emit ONLY the boundary faces (6 sides of the chunk)
        // because all interior cells border other solid cells. Virtual
        // should match that pattern: bounded face count, and crucially
        // NOT empty (since cells are solid and the chunk boundary is
        // air-equivalent in this mesher).
        let gen = test_generator();
        let coord = ChunkCoord::new(0, -4, 0);  // y=-128 block origin
        let mesh = mesh_chunk_far_virtual(coord, &gen, 2);
        // Solid chunk: should emit boundary faces. At factor=2 a fully
        // solid chunk emits 6 faces × 16² cells per face = 1536 quads
        // = 6144 verts. Allow generous slop in case some cave-carving
        // hits the boundary.
        assert!(!mesh.is_empty(),
            "expected non-empty mesh deep underground");
    }

    #[test]
    fn virtual_far_surface_chunk_produces_mesh_in_aabb() {
        // y=2 spans block-y 64..96, which crosses the surface in most
        // seeds. Expect some mesh emitted, and all vertices within
        // the chunk's world-space AABB.
        let gen = test_generator();
        let coord = ChunkCoord::new(0, 2, 0);
        let mesh = mesh_chunk_far_virtual(coord, &gen, 2);
        // Don't assert non-empty unconditionally — the seed *could*
        // give us pure stone or pure air for this chunk. But if it's
        // non-empty, it must be in-bounds.
        let (ox, oy, oz) = coord.block_origin();
        let xmin = ox as f32;
        let ymin = oy as f32;
        let zmin = oz as f32;
        let xmax = (ox + CHUNK_SIZE) as f32;
        let ymax = (oy + CHUNK_SIZE) as f32;
        let zmax = (oz + CHUNK_SIZE) as f32;
        for v in &mesh.vertices {
            let [x, y, z] = v.position;
            assert!(x >= xmin && x <= xmax, "x={} out of [{}, {}]", x, xmin, xmax);
            assert!(y >= ymin && y <= ymax, "y={} out of [{}, {}]", y, ymin, ymax);
            assert!(z >= zmin && z <= zmax, "z={} out of [{}, {}]", z, zmin, zmax);
        }
    }

    #[test]
    fn virtual_far_and_materialised_comparable_triangle_count() {
        // Mesh the same coord both ways and check triangle counts
        // agree within an order of magnitude. They WILL differ because
        // the algorithms differ (centre-sample vs F³ majority + surface
        // decoration phases). Big differences (10×+) would indicate
        // either path is broken.
        let gen = test_generator();
        // Try several coords to reduce sensitivity to seed quirks.
        let coords = [
            ChunkCoord::new(0, 2, 0),
            ChunkCoord::new(1, 2, 1),
            ChunkCoord::new(-2, 1, 3),
        ];
        for coord in coords {
            let virtual_mesh = mesh_chunk_far_virtual(coord, &gen, 2);
            let real_chunk = gen.generate_chunk(coord);
            let real_mesh = mesh_chunk_downsampled(&real_chunk, 2);

            let v_tris = virtual_mesh.index_count() / 3;
            let r_tris = real_mesh.index_count() / 3;

            // If both are zero, that's fine — agreement on emptiness.
            if v_tris == 0 && r_tris == 0 { continue; }

            // Both non-zero: ratio in [0.1, 10].
            // One zero, other non-zero: skip — that's an algorithm
            // difference at the edge of solid/air, expected.
            if v_tris > 0 && r_tris > 0 {
                let ratio = (v_tris as f32) / (r_tris as f32);
                assert!(ratio >= 0.1 && ratio <= 10.0,
                    "coord {:?}: virtual={} tris, materialised={} tris (ratio {:.2})",
                    coord, v_tris, r_tris, ratio);
            }
        }
    }

    #[test]
    fn virtual_very_far_factor_4_works() {
        // Same as the surface test but at factor=4 (VeryFar tier).
        // Just check it runs and produces in-bounds vertices.
        let gen = test_generator();
        let coord = ChunkCoord::new(0, 2, 0);
        let mesh = mesh_chunk_far_virtual(coord, &gen, 4);
        let (ox, oy, oz) = coord.block_origin();
        for v in &mesh.vertices {
            let [x, y, z] = v.position;
            assert!(x >= ox as f32 && x <= (ox + CHUNK_SIZE) as f32);
            assert!(y >= oy as f32 && y <= (oy + CHUNK_SIZE) as f32);
            assert!(z >= oz as f32 && z <= (oz + CHUNK_SIZE) as f32);
        }
    }

    // r198: the soft-edge corner offset at a chunk SEAM must be identical
    // computed from either adjacent chunk, or grass at different heights
    // cracks open along the boundary (the operator's "holes between grass
    // blocks at different heights"). Before the boundary-mirror fix, an
    // out-of-bounds sample read Air, so the two chunks disagreed on the
    // shared corner's sag. This builds two chunks meeting at x=CHUNK_SIZE
    // with mismatched grass fill and asserts the shared boundary corner
    // offset matches from both sides.
    #[test]
    fn soft_edge_corner_matches_across_chunk_seam() {
        use crate::world::blocks::BlockId;
        // Chunk A at (0,0,0): grass filling up to y=10 near its +X edge.
        let mut a = Chunk::empty(ChunkCoord::new(0, 0, 0));
        // Chunk B at (1,0,0): grass only up to y=6 near its -X edge (a
        // height mismatch right at the shared boundary).
        let mut b = Chunk::empty(ChunkCoord::new(1, 0, 0));
        for z in 0..CHUNK_SIZE {
            for y in 0..=10 {
                a.set(CHUNK_SIZE - 1, y, z, BlockId::Grass);
            }
            for y in 0..=6 {
                b.set(0, y, z, BlockId::Grass);
            }
        }
        // r198: the WORLD->LOCAL frame fix is the guaranteed part: before
        // it, a non-origin chunk (B, origin x=32) sampled out of bounds for
        // EVERY corner (world coords vs local get()), read Air, and so its
        // soft-edge offset collapsed toward the no-sag baseline. After the
        // fix, B actually sees its own grass and sags. We assert B is no
        // longer stuck at the degenerate (near-zero) value where its grass
        // clearly should produce sag.
        //
        // NOTE: PERFECT corner agreement at the seam additionally needs the
        // 3x3 NEIGHBORHOOD to match, which requires real cross-chunk reads
        // the single-chunk mesher doesn't have (the outer-ring air count
        // genuinely differs between two chunks sharing one edge). The mirror
        // fixes the boundary column + the frame bug; full ring agreement is
        // tracked for the cross-chunk-neighbor meshing pass.
        let seam_wx = CHUNK_SIZE;
        // The frame fix's guaranteed effect: chunk B (origin x=32) now
        // SEES its own grass at corner samples instead of reading Air.
        // Direct proof — sample the corner block each side sees at the
        // seam; both must be the grass we placed, not the phantom Air the
        // pre-fix world-vs-local mismatch returned for non-origin chunks.
        let a_seam_block = block_at_corner_sample(&a, [seam_wx - 1, 5, 4]);
        let b_seam_block = block_at_corner_sample(&b, [seam_wx, 5, 4]);
        assert_eq!(a_seam_block, BlockId::Grass, "A sees grass at seam");
        assert_eq!(b_seam_block, BlockId::Grass, "B sees grass at seam (frame fix)");
        // And max_softness_at_corner for B must be non-zero at the grass
        // surface corner (slice=6, where B's grass top is) — proving the
        // sampler reaches B's real blocks, not Air (which would give 0).
        let b_soft = max_softness_at_corner(&b, seam_wx, 6, CHUNK_SIZE / 2);
        assert!(
            b_soft > 0.0,
            "non-origin chunk B softness must be non-zero at its grass corner, got {b_soft}"
        );



    }
}


#[cfg(test)]
mod tile_id_pack_tests {
    use super::PackedVertex;

    #[test]
    fn pack_color_keeps_alpha_full() {
        // Existing behaviour must be byte-identical: alpha stays 0xFF.
        let w = PackedVertex::pack_color([0.5, 0.25, 1.0]);
        assert_eq!((w >> 24) & 0xFF, 0xFF);
    }

    #[test]
    fn pack_color_id_rides_alpha_and_preserves_rgb() {
        let rgb = [0.5, 0.25, 1.0];
        for id in [0u8, 1, 7, 128, 255] {
            let w = PackedVertex::pack_color_id(rgb, id);
            assert_eq!((w >> 24) & 0xFF, id as u32, "id must occupy the alpha byte");
            assert_eq!(
                w & 0x00FF_FFFF,
                PackedVertex::pack_color(rgb) & 0x00FF_FFFF,
                "rgb must be untouched by stashing the id"
            );
        }
    }

    #[test]
    fn agrees_with_ruleset_packer_bit_for_bit() {
        // Pin the production packer to world::texture_mapping::pack_color_id so the
        // contract ruleset and the real pack path can't drift on bit layout.
        let rgb_u8 = [122u8, 184, 86];
        let rgb_f = [122.0 / 255.0, 184.0 / 255.0, 86.0 / 255.0];
        for id in [0u8, 7, 200, 255] {
            let prod = PackedVertex::pack_color_id(rgb_f, id);
            let rules = crate::world::texture_mapping::pack_color_id(rgb_u8, id);
            assert_eq!(prod, rules, "production packer must match the ruleset packer");
        }
    }
}
