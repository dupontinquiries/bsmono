//! CPU reference port of `chunk_mesh_octree.comp`. Bit-equivalent
//! algorithm running on CPU so we can unit-test the meshing logic
//! without spinning up Vulkan.
//!
//! The lesson from v0.24.27 was that a probe verifying the
//! *substrate* (octree contents on GPU vs CPU) does NOT verify a
//! *consumer's* output. v0.24.27 shipped three times before catching
//! a one-line bug in the consumer (chunk-local vertex positions
//! emitted instead of world coords). Each iteration took an hour of
//! build-zip-test latency.
//!
//! This module exists so future bugs of that class fail in `cargo
//! test`, not in the playtest cycle.
//!
//! ## Reference algorithm
//!
//! Mirrors `shaders/chunk_mesh_octree.comp` exactly:
//! - Iterate every cell `(x, y, z)` in 0..32³.
//! - Sample self block_id via `OctreeArray::sample`.
//! - If air/water, skip.
//! - Sample 6 neighbors with open-boundary semantics (out-of-chunk
//!   → air).
//! - For each non-solid neighbor, emit a quad on that face.
//! - Vertices in WORLD coords (chunk origin added).
//! - Colors per `color_for_block` table.
//!
//! ## Falsification
//!
//! `tests/` below cover:
//! - Empty octree (all air) → empty mesh.
//! - Fully-solid octree (all stone) → 6 outer-face shells, 32×32×6
//!   quads, vertices clamped to chunk world AABB.
//! - Single-block octree (one stone block at known coord) → 6 quads
//!   centered on that block.
//! - Checkerboard pattern → known quad count, no internal merging
//!   (we don't do greedy).
//! - World-coord regression: chunk at `(1, 2, -1)` must emit
//!   vertices in x∈[32,64], y∈[64,96], z∈[-32,0]. This is the
//!   specific regression for v0.24.27-fix3.
//!
//! ## Not covered here
//!
//! Bumpy-top emission (Ship B-prep) is in a separate `_bumpy` module
//! once the basic version is locked in. Same testing discipline.

use crate::render::mesher::{ChunkMesh, Vertex};
use crate::world::blocks::BlockId;
use crate::world::chunk::ChunkCoord;
use crate::world::octree::OctreeArray;

/// Mirror of the GLSL `is_solid_opaque` predicate.
pub fn is_solid_opaque(block: BlockId) -> bool {
    // r539: subvoxel-only blocks (leaves + crops) render entirely as subvoxel
    // decoration; emitting a solid cube hides that detail (the "solid block over
    // leaves" bug). Matches the CPU mesher's is_meshable and the GLSL update.
    !matches!(block, BlockId::Air | BlockId::Water) && !block.is_subvoxel_only()
}

/// v0.24.28-bumpy-foundation: which block-ids get the 4×4 bumpy top
/// face. Soft / natural materials only. Mirrors the GLSL
/// `is_weatherable` predicate.
///
/// Note: this is a static per-block-id classification for Ship
/// B-prep. The full Ship B replaces it with a per-block weathering
/// value read from a parallel GPU instance buffer (where individual
/// blocks can have different weathering levels even within the same
/// block-id, including freshly-placed = 0).
pub fn is_weatherable(block: BlockId) -> bool {
    matches!(block as u8,
        2  | // Grass
        3  | // Dirt
        12 | // Sand
        13 | // Gravel
        32   // Snow
    )
}

/// Mirror of the GLSL `bumpy_height` function. Deterministic from
/// world coords. Returns a non-positive bump amount (max 0, min
/// roughly -0.4 blocks).
///
/// Non-positive is by design: bumps only DIP DOWN, never up. This
/// prevents per-block bumps from creating "step seams" at terrain
/// breaks (where one column is 1 block tall and the neighbor is 2
/// blocks; an upward bump on the lower would poke through the side
/// of the upper, exposing a thin slice of internal material).
///
/// Adjacent blocks agree at shared edges by construction because
/// the function only depends on world coordinates, and adjacent
/// blocks' shared-edge vertices have identical world coords.
pub fn bumpy_height(wx: f32, wz: f32) -> f32 {
    let n1 = hash2d(wx * 0.5, wz * 0.5);
    let n2 = hash2d(wx * 1.5, wz * 1.5);
    let n = n1 * 0.7 + n2 * 0.3;
    -n * 0.35
}

/// 2D hash returning value in [0, 1). Deterministic, no smoothing.
/// Mirrors the GLSL helper of the same name bit-for-bit so the CPU
/// reference can serve as a falsification gate against the GPU
/// output. (GLSL `fract(x)` is `x - floor(x)`, which is NOT the same
/// as Rust's `f32::fract` for negative inputs — we use the explicit
/// `x - x.floor()` form.)
///
/// v0.24.38-bumpy-asym: distinct per-axis constants so the hash
/// isn't symmetric under x↔z swap. Must mirror the GLSL hash
/// exactly — see shader for rationale.
fn hash2d(x: f32, z: f32) -> f32 {
    let glsl_fract = |v: f32| v - v.floor();
    let px = glsl_fract(x * 123.34);
    let pz = glsl_fract(z * 174.85);
    let dot_p = px * (px + 45.32) + pz * (pz + 67.91);
    let combined = px * pz * 0.7 + dot_p;
    glsl_fract(combined).abs()
}

/// Mirror of the GLSL `color_for_block` table.
pub fn color_for_block(block: BlockId) -> [f32; 3] {
    // r544: full mirror of the GLSL color_for_block table (chunk_mesh_octree.comp).
    // Kept in sync so the table is honest; color is not part of the VERIFY geometry
    // parity check, but this protects any future color-aware comparison.
    match block as u8 {
        1  => [0.50, 0.50, 0.50],
        2  => [0.30, 0.65, 0.20],
        3  => [0.55, 0.40, 0.25],
        4  => [0.45, 0.45, 0.45],
        7  => [0.15, 0.15, 0.15],
        11 => [0.95, 0.45, 0.10],
        12 => [0.85, 0.80, 0.55],
        13 => [0.50, 0.48, 0.45],
        14 => [0.85, 0.75, 0.30],
        15 => [0.70, 0.60, 0.50],
        16 => [0.30, 0.30, 0.30],
        17 => [0.40, 0.30, 0.20],
        18 => [0.20, 0.55, 0.18],
        19 => [0.85, 0.85, 0.80],
        20 => [0.40, 0.65, 0.35],
        21 => [0.20, 0.55, 0.20],
        22 => [0.65, 0.65, 0.75],
        23 => [0.30, 0.55, 0.30],
        24 => [0.78, 0.45, 0.25],
        25 => [0.95, 0.30, 0.40],
        26 => [0.40, 0.70, 0.30],
        27 => [0.30, 0.60, 0.30],
        28 => [0.55, 0.40, 0.20],
        29 => [0.30, 0.55, 0.25],
        30 => [0.65, 0.50, 0.50],
        31 => [0.75, 0.85, 0.95],
        32 => [0.95, 0.95, 0.97],
        33 => [0.85, 0.30, 0.30],
        34 => [0.85, 0.85, 0.85],
        35 => [0.55, 0.65, 0.55],
        36 => [0.85, 0.83, 0.78],
        37 => [0.92, 0.92, 0.89],
        38 => [0.18, 0.17, 0.20],
        39 => [0.55, 0.52, 0.50],
        40 => [0.72, 0.55, 0.48],
        41 => [0.30, 0.32, 0.38],
        42 => [0.94, 0.94, 0.96],
        43 => [0.50, 0.50, 0.52],
        44 => [0.38, 0.36, 0.40],
        45 => [0.93, 0.90, 0.78],
        46 => [0.85, 0.55, 0.18],
        47 => [0.78, 0.65, 0.95],
        48 => [0.843, 0.784, 0.627],
        49 => [0.098, 0.078, 0.137],
        50 => [0.745, 0.510, 0.353],
        51 => [0.922, 0.922, 0.894],
        56 => [0.45, 0.85, 0.95],
        57 => [0.667, 0.529, 0.353],
        58 => [0.490, 0.490, 0.490],
        59 => [0.627, 0.314, 0.255],
        61 => [0.784, 0.882, 0.922],
        62 => [0.784, 0.784, 0.804],
        63 => [0.902, 0.784, 0.353],
        64 => [0.176, 0.176, 0.196],
        65 => [0.588, 0.431, 0.275],
        66 => [0.863, 0.510, 0.157],
        80 => [0.95, 0.95, 0.97],
        81 => [0.420, 0.502, 0.380],
        82 => [0.580, 0.580, 0.600],
        83 => [0.878, 0.800, 0.580],
        84 => [0.620, 0.341, 0.239],
        85 => [0.478, 0.471, 0.459],
        86 => [0.322, 0.220, 0.129],
        87 => [0.220, 0.212, 0.239],
        _  => [0.80, 0.20, 0.80],   // magenta — unhandled id (matches GLSL default)
    }
}

/// Open-boundary sample. Returns Air for out-of-chunk coords.
fn sample_or_air(octree: &OctreeArray, x: i32, y: i32, z: i32) -> BlockId {
    if !(0..32).contains(&x) || !(0..32).contains(&y) || !(0..32).contains(&z) {
        BlockId::Air
    } else {
        octree.sample(x, y, z)
    }
}

/// v0.24.30-cross-chunk: neighbor octrees for cross-boundary
/// sampling. `None` means "not resident" — caller falls back to
/// open-boundary semantics for that direction. Order matches the
/// shader's ChunkMeta neighbor fields.
#[derive(Default, Clone, Copy)]
pub struct Neighbors<'a> {
    pub px: Option<&'a OctreeArray>,
    pub nx: Option<&'a OctreeArray>,
    pub py: Option<&'a OctreeArray>,
    pub ny: Option<&'a OctreeArray>,
    pub pz: Option<&'a OctreeArray>,
    pub nz: Option<&'a OctreeArray>,
}

/// Cross-chunk-aware sample. Exactly one axis may be out of range
/// per call (the 6 face checks add ±1 to one axis at a time).
fn sample_or_neighbor(
    own: &OctreeArray,
    neighbors: &Neighbors<'_>,
    x: i32, y: i32, z: i32,
) -> BlockId {
    if (0..32).contains(&x) && (0..32).contains(&y) && (0..32).contains(&z) {
        return own.sample(x, y, z);
    }
    // Out-of-bounds on exactly one axis. Identify which neighbor.
    let (neighbor, lx, ly, lz) = if x == 32 {
        (neighbors.px, 0, y, z)
    } else if x == -1 {
        (neighbors.nx, 31, y, z)
    } else if y == 32 {
        (neighbors.py, x, 0, z)
    } else if y == -1 {
        (neighbors.ny, x, 31, z)
    } else if z == 32 {
        (neighbors.pz, x, y, 0)
    } else if z == -1 {
        (neighbors.nz, x, y, 31)
    } else {
        // Diagonal or more-than-one-axis out: not supported here.
        // Mirror the shader's behaviour: open-boundary fallback.
        return BlockId::Air;
    };
    match neighbor {
        Some(n) => n.sample(lx, ly, lz),
        None => BlockId::Air,
    }
}

/// CPU reference for the GPU octree mesher.
///
/// Output is bit-equivalent to what `chunk_mesh_octree.comp`
/// produces given the same input, modulo floating-point ordering
/// (the GPU dispatches threads in parallel so vertex order differs;
/// the SET of vertices and indices is the same).
pub fn mesh_chunk_octree_cpu_reference(
    coord: ChunkCoord,
    octree: &OctreeArray,
) -> ChunkMesh {
    // v0.24.30-cross-chunk: this open-boundary form is now a
    // convenience wrapper. Production code (and the shader) uses
    // the neighbor-aware variant.
    mesh_chunk_octree_cpu_reference_with_neighbors(coord, octree, &Neighbors::default())
}

/// v0.24.30-cross-chunk: cross-chunk-aware mesher. When a neighbor
/// is supplied for a face direction, that face is sampled across
/// the chunk boundary instead of falling back to open-boundary
/// (which treats out-of-chunk as air).
///
/// The shader's behavior is identical: ChunkMeta carries 6
/// neighbor octree bases (or `u32::MAX` for "open"); this function
/// uses `Option<&OctreeArray>` per direction.
pub fn mesh_chunk_octree_cpu_reference_with_neighbors(
    coord: ChunkCoord,
    octree: &OctreeArray,
    neighbors: &Neighbors<'_>,
) -> ChunkMesh {
    let (ox, oy, oz) = coord.block_origin();
    let ox_f = ox as f32;
    let oy_f = oy as f32;
    let oz_f = oz as f32;

    let mut vertices: Vec<Vertex> = Vec::with_capacity(4096);
    let mut indices: Vec<u32> = Vec::with_capacity(6144);

    // Iterate in the same order the GLSL would (y outer, z middle,
    // x inner) so vertex order matches when the GPU runs single-
    // threaded. Doesn't affect correctness when multi-threaded —
    // both produce the same SET of vertices.
    for y in 0..32_i32 {
        for z in 0..32_i32 {
            for x in 0..32_i32 {
                let self_block = octree.sample(x, y, z);
                if !is_solid_opaque(self_block) {
                    continue;
                }

                let n_px = sample_or_neighbor(octree, neighbors, x + 1, y,     z);
                let n_nx = sample_or_neighbor(octree, neighbors, x - 1, y,     z);
                let n_py = sample_or_neighbor(octree, neighbors, x,     y + 1, z);
                let n_ny = sample_or_neighbor(octree, neighbors, x,     y - 1, z);
                let n_pz = sample_or_neighbor(octree, neighbors, x,     y,     z + 1);
                let n_nz = sample_or_neighbor(octree, neighbors, x,     y,     z - 1);

                let color = color_for_block(self_block);

                // World-coord base. The v0.24.27-fix3 regression was
                // forgetting to add the chunk origin here.
                let px = ox_f + x as f32;
                let py = oy_f + y as f32;
                let pz = oz_f + z as f32;

                let c000 = [px,       py,       pz];
                let c100 = [px + 1.0, py,       pz];
                let c010 = [px,       py + 1.0, pz];
                let c110 = [px + 1.0, py + 1.0, pz];
                let c001 = [px,       py,       pz + 1.0];
                let c101 = [px + 1.0, py,       pz + 1.0];
                let c011 = [px,       py + 1.0, pz + 1.0];
                let c111 = [px + 1.0, py + 1.0, pz + 1.0];

                if !is_solid_opaque(n_px) {
                    emit_quad(&mut vertices, &mut indices, c100, c110, c111, c101, color);
                }
                if !is_solid_opaque(n_nx) {
                    emit_quad(&mut vertices, &mut indices, c000, c001, c011, c010, color);
                }
                if !is_solid_opaque(n_py) {
                    // v0.24.28-bumpy-foundation: weatherable blocks
                    // get a 4×4 sub-grid of quads on the top face,
                    // with each sub-vertex displaced by
                    // bumpy_height(world_x, world_z). Non-weatherable
                    // blocks emit a normal flat quad.
                    if is_weatherable(self_block) {
                        emit_bumpy_top_quad(
                            &mut vertices, &mut indices,
                            px, py + 1.0, pz, color,
                        );
                    } else {
                        emit_quad(&mut vertices, &mut indices, c010, c011, c111, c110, color);
                    }
                }
                if !is_solid_opaque(n_ny) {
                    emit_quad(&mut vertices, &mut indices, c000, c100, c101, c001, color);
                }
                if !is_solid_opaque(n_pz) {
                    emit_quad(&mut vertices, &mut indices, c001, c101, c111, c011, color);
                }
                if !is_solid_opaque(n_nz) {
                    emit_quad(&mut vertices, &mut indices, c000, c010, c110, c100, color);
                }
            }
        }
    }

    ChunkMesh { vertices, indices, ..ChunkMesh::empty() }
}

fn emit_quad(
    vertices: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    p0: [f32; 3], p1: [f32; 3], p2: [f32; 3], p3: [f32; 3],
    color: [f32; 3],
) {
    let base = vertices.len() as u32;
    vertices.push(Vertex::new(p0, color));
    vertices.push(Vertex::new(p1, color));
    vertices.push(Vertex::new(p2, color));
    vertices.push(Vertex::new(p3, color));
    indices.extend_from_slice(&[
        base, base + 1, base + 2,
        base, base + 2, base + 3,
    ]);
}

/// v0.24.28-bumpy-foundation: bumpy top-face emission.
///
/// Emits a 4×4 grid of sub-quads on a top face (+Y face). The face
/// is at world position `(px, py, pz)` extending +1 in X and +Z.
/// Sub-vertices live on a 5×5 grid in `(u, v) ∈ [0, 1]²` space,
/// with `world_y = py + bumpy_height(world_x, world_z)`.
///
/// Total emission per face: 25 vertices, 16 sub-quads = 32 triangles
/// (96 indices). Vs. flat: 4 vertices, 2 triangles (6 indices).
/// Inflation: 6.25× vertices, 16× triangles.
///
/// Adjacent blocks agree at shared edges because each sub-vertex's
/// position is a pure function of world coordinates — both
/// neighbors compute the same `bumpy_height` for the same edge.
fn emit_bumpy_top_quad(
    vertices: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    px: f32, py_top: f32, pz: f32,
    color: [f32; 3],
) {
    const GRID_N: i32 = 4;          // 4 sub-quads per side
    const GRID_V: i32 = GRID_N + 1; // 5 vertex positions per side

    let base = vertices.len() as u32;

    // Emit 5×5 = 25 vertices in (i, j) row-major order.
    for i in 0..GRID_V {
        for j in 0..GRID_V {
            let u = i as f32 / GRID_N as f32;
            let v = j as f32 / GRID_N as f32;
            let wx = px + u;
            let wz = pz + v;
            let bump = bumpy_height(wx, wz);
            vertices.push(Vertex::new([wx, py_top + bump, wz], color));
        }
    }

    // Emit 16 sub-quads, each as 2 triangles with CCW winding viewed
    // from +Y.
    for qi in 0..GRID_N {
        for qj in 0..GRID_N {
            let v00 = base + (qi      * GRID_V + qj)     as u32;
            let v10 = base + ((qi + 1) * GRID_V + qj)     as u32;
            let v11 = base + ((qi + 1) * GRID_V + (qj + 1)) as u32;
            let v01 = base + (qi      * GRID_V + (qj + 1)) as u32;
            // CCW from +Y looking down: v00 -> v10 -> v11 -> v01.
            indices.extend_from_slice(&[
                v00, v10, v11,
                v00, v11, v01,
            ]);
        }
    }
}

/// Output-sanity check applied to ANY mesh (CPU reference or GPU
/// output). Used by the v0.24.28 output-sanity probe to flag
/// suspect meshes without needing a CPU↔GPU compare.
///
/// Returns Ok(()) if all invariants hold, Err(message) otherwise.
pub fn check_mesh_invariants(
    mesh: &ChunkMesh,
    coord: ChunkCoord,
) -> Result<(), String> {
    let (ox, oy, oz) = coord.block_origin();
    let min_x = ox as f32;
    let max_x = (ox + 32) as f32;
    let min_y = oy as f32;
    let max_y = (oy + 32) as f32;
    let min_z = oz as f32;
    let max_z = (oz + 32) as f32;

    // Vertex AABB.
    for (i, v) in mesh.vertices.iter().enumerate() {
        for c in v.position.iter() {
            if !c.is_finite() {
                return Err(format!(
                    "vertex {} non-finite position {:?}", i, v.position
                ));
            }
        }
        let [vx, vy, vz] = v.position;
        if vx < min_x || vx > max_x ||
           vy < min_y || vy > max_y ||
           vz < min_z || vz > max_z
        {
            return Err(format!(
                "vertex {} at ({}, {}, {}) outside chunk AABB \
                 ({}..{}, {}..{}, {}..{}) for coord {:?}",
                i, vx, vy, vz,
                min_x, max_x, min_y, max_y, min_z, max_z, coord,
            ));
        }
    }

    // Index/vertex ratio: each quad emits 6 indices. For meshes that
    // SHARE vertices between adjacent quads (bumpy 5×5 = 25 verts, 16
    // quads = 96 indices), the verts-to-indices ratio differs from the
    // "4 unique verts per quad" pattern. Skip the strict multiple-of-4
    // check; rely on "all indices in range" below to catch real bugs.
    //
    // RC42.1: relaxed from "verts.len() % 4 == 0 AND indices.len() ==
    // (verts.len() / 4) * 6". That check assumed independent-quad
    // emission, which the bumpy emitter doesn't follow. The remaining
    // invariants (AABB, finite, indices-in-range) are still enforced.
    if mesh.indices.len() % 6 != 0 {
        return Err(format!(
            "index count {} is not a multiple of 6 (each quad emits 6 indices)",
            mesh.indices.len()
        ));
    }

    // All indices in range.
    let n = mesh.vertices.len() as u32;
    for (i, &idx) in mesh.indices.iter().enumerate() {
        if idx >= n {
            return Err(format!(
                "index {} at position {} references nonexistent vertex (count={})",
                idx, i, n
            ));
        }
    }

    Ok(())
}

// ───────────────────────────────────────────────────────────────
// v0.24.37-mesh-cmp: gated runtime equivalence comparator
// ───────────────────────────────────────────────────────────────

/// Result of comparing a GPU-produced mesh against the CPU reference
/// for the same chunk + neighbors. The check is loose by design:
/// strict vertex-set equality fails because the two implementations
/// emit verts in different orders (CPU is sequential by cell index;
/// GPU dispatches one thread per cell, so order depends on workgroup
/// scheduling). Instead we compare invariants that any correct
/// implementation must agree on.
#[derive(Debug, Clone)]
pub struct MeshCompareResult {
    pub vertex_count_gpu: usize,
    pub vertex_count_cpu: usize,
    pub index_count_gpu: usize,
    pub index_count_cpu: usize,
    pub aabb_gpu: Option<([f32; 3], [f32; 3])>,
    pub aabb_cpu: Option<([f32; 3], [f32; 3])>,
}

impl MeshCompareResult {
    /// Vertex counts must match exactly — both implementations emit
    /// the same number of verts per face direction per cell (4 for
    /// flat, 25 for bumpy top). A diff here means one side is
    /// emitting faces the other isn't.
    pub fn vertex_count_matches(&self) -> bool {
        self.vertex_count_gpu == self.vertex_count_cpu
    }

    /// Index counts likewise (6 per flat quad, 96 per bumpy top).
    pub fn index_count_matches(&self) -> bool {
        self.index_count_gpu == self.index_count_cpu
    }

    /// AABB corners within `tolerance` on every axis. The
    /// `0.5` default catches v0.24.27-fix3-class bugs (all verts at
    /// world origin) without false-positives from bumpy-height
    /// floating-point variance (which stays under 0.4 magnitude).
    pub fn aabb_close(&self, tolerance: f32) -> bool {
        match (self.aabb_gpu, self.aabb_cpu) {
            (Some((g_mn, g_mx)), Some((c_mn, c_mx))) => {
                let max_diff = |a: [f32; 3], b: [f32; 3]| {
                    (0..3).map(|i| (a[i] - b[i]).abs())
                        .fold(0.0_f32, f32::max)
                };
                max_diff(g_mn, c_mn) <= tolerance
                    && max_diff(g_mx, c_mx) <= tolerance
            }
            (None, None) => true,  // both empty
            _ => false,             // one empty, other not — clear mismatch
        }
    }

    /// All loose checks pass.
    pub fn matches_loosely(&self, aabb_tolerance: f32) -> bool {
        self.vertex_count_matches()
            && self.index_count_matches()
            && self.aabb_close(aabb_tolerance)
    }
}

fn mesh_aabb(verts: &[Vertex]) -> Option<([f32; 3], [f32; 3])> {
    if verts.is_empty() {
        return None;
    }
    let mut mn = verts[0].position;
    let mut mx = verts[0].position;
    for v in &verts[1..] {
        for i in 0..3 {
            mn[i] = mn[i].min(v.position[i]);
            mx[i] = mx[i].max(v.position[i]);
        }
    }
    Some((mn, mx))
}

/// Compare two `ChunkMesh`es — typically GPU output against CPU
/// reference. Caller decides what to do with the result; see
/// `MeshCompareResult::matches_loosely` for the default loose-check.
pub fn compare_meshes(gpu: &ChunkMesh, cpu: &ChunkMesh) -> MeshCompareResult {
    MeshCompareResult {
        vertex_count_gpu: gpu.vertices.len(),
        vertex_count_cpu: cpu.vertices.len(),
        index_count_gpu: gpu.indices.len(),
        index_count_cpu: cpu.indices.len(),
        aabb_gpu: mesh_aabb(&gpu.vertices),
        aabb_cpu: mesh_aabb(&cpu.vertices),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::chunk::Chunk;

    /// Test helper: chunk fully filled with one block type.
    fn filled_chunk(coord: ChunkCoord, block: BlockId) -> Chunk {
        let blocks = vec![block; 32 * 32 * 32];
        Chunk::from_blocks(coord, blocks)
    }

    /// Test helper: empty (all-air) chunk.
    fn empty_chunk(coord: ChunkCoord) -> Chunk {
        Chunk::empty(coord)
    }

    /// Empty (all-air) octree must produce empty mesh.
    #[test]
    fn empty_octree_emits_no_geometry() {
        let chunk = empty_chunk(ChunkCoord::new(0, 0, 0));
        let octree = OctreeArray::from_chunk(&chunk);
        let coord = ChunkCoord::new(0, 0, 0);
        let mesh = mesh_chunk_octree_cpu_reference(coord, &octree);
        assert_eq!(mesh.vertices.len(), 0, "expected no vertices for all-air chunk");
        assert_eq!(mesh.indices.len(), 0, "expected no indices for all-air chunk");
        assert!(check_mesh_invariants(&mesh, coord).is_ok());
    }

    /// Fully-solid chunk with open boundary emits exactly 6 outer
    /// face shells: 32x32 quads per face × 6 faces.
    #[test]
    fn fully_solid_octree_emits_outer_shell() {
        let chunk = filled_chunk(ChunkCoord::new(0, 0, 0), BlockId::Stone);
        let octree = OctreeArray::from_chunk(&chunk);
        let coord = ChunkCoord::new(0, 0, 0);
        let mesh = mesh_chunk_octree_cpu_reference(coord, &octree);

        let expected_quads = 32 * 32 * 6;
        assert_eq!(mesh.vertices.len(), expected_quads * 4,
            "expected {} verts (32×32 quads on each of 6 outer faces)", expected_quads * 4);
        assert_eq!(mesh.indices.len(), expected_quads * 6);
        assert!(check_mesh_invariants(&mesh, coord).is_ok());
    }

    /// Vertices for a non-origin chunk must be in WORLD coords.
    /// This is the v0.24.27-fix3 regression test.
    #[test]
    fn world_coord_regression_chunk_1_2_neg1() {
        let coord = ChunkCoord::new(1, 2, -1);
        // A single stone block at chunk-local (5, 6, 7) in this chunk's coord.
        let mut chunk = empty_chunk(coord);
        chunk.set(5, 6, 7, BlockId::Stone);
        let octree = OctreeArray::from_chunk(&chunk);

        let mesh = mesh_chunk_octree_cpu_reference(coord, &octree);

        // Chunk origin: (32, 64, -32). Block at local (5, 6, 7) is at
        // world (37, 70, -25). Its 8 cube corners span:
        //   x: 37..38, y: 70..71, z: -25..-24
        let expected_min = (37.0_f32, 70.0_f32, -25.0_f32);
        let expected_max = (38.0_f32, 71.0_f32, -24.0_f32);

        assert!(!mesh.vertices.is_empty(), "expected at least one face");
        for v in &mesh.vertices {
            let [vx, vy, vz] = v.position;
            assert!(vx >= expected_min.0 && vx <= expected_max.0,
                "vertex x={} out of expected world range [{}, {}] — \
                 v0.24.27-fix3 regression!",
                vx, expected_min.0, expected_max.0);
            assert!(vy >= expected_min.1 && vy <= expected_max.1,
                "vertex y={} out of expected world range [{}, {}]",
                vy, expected_min.1, expected_max.1);
            assert!(vz >= expected_min.2 && vz <= expected_max.2,
                "vertex z={} out of expected world range [{}, {}]",
                vz, expected_min.2, expected_max.2);
        }
        assert!(check_mesh_invariants(&mesh, coord).is_ok());
    }

    /// A single isolated solid block emits exactly 6 quads.
    #[test]
    fn single_isolated_block_emits_six_quads() {
        let coord = ChunkCoord::new(0, 0, 0);
        let mut chunk = empty_chunk(coord);
        chunk.set(10, 10, 10, BlockId::Stone);
        let octree = OctreeArray::from_chunk(&chunk);
        let mesh = mesh_chunk_octree_cpu_reference(coord, &octree);
        assert_eq!(mesh.vertices.len(), 6 * 4, "expected 6 quads = 24 verts");
        assert_eq!(mesh.indices.len(), 6 * 6, "expected 6 quads = 36 indices");
    }

    /// Two adjacent solid blocks share a face, so only 10 faces emit
    /// (6 + 6 - 2 hidden internal faces).
    #[test]
    fn two_adjacent_blocks_share_one_face() {
        let coord = ChunkCoord::new(0, 0, 0);
        let mut chunk = empty_chunk(coord);
        chunk.set(10, 10, 10, BlockId::Stone);
        chunk.set(11, 10, 10, BlockId::Stone);
        let octree = OctreeArray::from_chunk(&chunk);
        let mesh = mesh_chunk_octree_cpu_reference(coord, &octree);
        // 6 + 6 outer faces - 2 hidden internal faces = 10 exposed.
        assert_eq!(mesh.vertices.len(), 10 * 4);
        assert_eq!(mesh.indices.len(), 10 * 6);
    }

    /// Air block at origin chunk has invariants checked even with
    /// trivial mesh.
    #[test]
    fn invariant_checker_accepts_empty_mesh() {
        let mesh = ChunkMesh::empty();
        assert!(check_mesh_invariants(&mesh, ChunkCoord::new(0, 0, 0)).is_ok());
    }

    /// Invariant checker rejects vertex outside AABB (catches the
    /// v0.24.27-fix3 regression class of bug at runtime).
    #[test]
    fn invariant_checker_rejects_out_of_aabb_vertex() {
        let coord = ChunkCoord::new(5, 0, 0); // origin x=160
        let mesh = ChunkMesh {
            vertices: vec![
                Vertex::new([0.5, 0.5, 0.5], [1.0, 0.0, 0.0]),  // way outside
                Vertex::new([0.5, 0.5, 1.5], [1.0, 0.0, 0.0]),
                Vertex::new([0.5, 1.5, 1.5], [1.0, 0.0, 0.0]),
                Vertex::new([0.5, 1.5, 0.5], [1.0, 0.0, 0.0]),
            ],
            indices: vec![0, 1, 2, 0, 2, 3],
            ..ChunkMesh::empty()
        };
        let err = check_mesh_invariants(&mesh, coord)
            .expect_err("expected AABB rejection");
        assert!(err.contains("outside chunk AABB"),
            "error message should mention AABB: {}", err);
    }

    /// Invariant checker rejects out-of-range index.
    #[test]
    fn invariant_checker_rejects_bad_index() {
        let coord = ChunkCoord::new(0, 0, 0);
        let mesh = ChunkMesh {
            vertices: vec![
                Vertex::new([0.0, 0.0, 0.0], [0.0; 3]),
                Vertex::new([1.0, 0.0, 0.0], [0.0; 3]),
                Vertex::new([1.0, 1.0, 0.0], [0.0; 3]),
                Vertex::new([0.0, 1.0, 0.0], [0.0; 3]),
            ],
            indices: vec![0, 1, 2, 0, 2, 99],  // 99 is out of range
            ..ChunkMesh::empty()
        };
        assert!(check_mesh_invariants(&mesh, coord).is_err());
    }

    /// Invariant checker rejects out-of-range indices. (Pre-RC42.1 this
    /// test rejected non-multiple-of-4 vertex counts, but that check was
    /// dropped because the bumpy emitter legitimately produces shared-
    /// vertex grids — e.g. 25 verts for a 5×5 bumpy top — which violate
    /// the strict "4 verts per quad" rule. Out-of-range index detection
    /// is the remaining catch-all for malformed meshes.)
    #[test]
    fn invariant_checker_rejects_bad_vertex_count() {
        let coord = ChunkCoord::new(0, 0, 0);
        let mesh = ChunkMesh {
            vertices: vec![
                Vertex::new([0.0, 0.0, 0.0], [0.0; 3]),
                Vertex::new([1.0, 0.0, 0.0], [0.0; 3]),
                Vertex::new([1.0, 1.0, 0.0], [0.0; 3]),
                Vertex::new([0.0, 1.0, 0.0], [0.0; 3]),
            ],
            // Index 99 is out of range (only 4 vertices exist).
            indices: vec![0, 1, 2, 99, 2, 3],
            ..ChunkMesh::empty()
        };
        assert!(check_mesh_invariants(&mesh, coord).is_err());
    }

    // ---- Bumpy-emission tests (Ship B-prep) ----

    /// `bumpy_height` is deterministic from world coords.
    #[test]
    fn bumpy_height_is_deterministic() {
        let a = bumpy_height(5.25, 7.75);
        let b = bumpy_height(5.25, 7.75);
        assert_eq!(a, b);
    }

    /// `bumpy_height` is always <= 0 (bumps dip down only, never up).
    #[test]
    fn bumpy_height_never_positive() {
        for x in -50..50 {
            for z in -50..50 {
                let h = bumpy_height(x as f32 * 0.25, z as f32 * 0.25);
                assert!(h <= 0.0, "bumpy_height({}, {}) = {} should be <= 0",
                    x, z, h);
                assert!(h >= -0.5, "bumpy_height({}, {}) = {} below -0.5 (too deep)",
                    x, z, h);
            }
        }
    }

    /// Adjacent blocks share a sub-vertex at their boundary, and
    /// `bumpy_height` returns the same value for the same world
    /// coords. This is the seam-free property.
    #[test]
    fn adjacent_blocks_agree_at_shared_edge() {
        // Block at (5, 5) and block at (6, 5) share the edge x = 6.
        // Both blocks' bumpy top-face sub-vertices along that edge
        // compute bumpy_height(6.0, z) for various z. Both must
        // return identical values.
        for j in 0..=4 {
            let v = j as f32 / 4.0;
            let z_shared = 5.0 + v;

            // From block (5, 5): right edge of its top face at u=1
            let h_left = bumpy_height(6.0, z_shared);
            // From block (6, 5): left edge of its top face at u=0
            let h_right = bumpy_height(6.0, z_shared);
            assert_eq!(h_left, h_right, "edge mismatch at z={}", z_shared);
        }
    }

    /// `is_weatherable` correctly classifies common block types.
    #[test]
    fn weatherable_predicate() {
        assert!(is_weatherable(BlockId::Grass));
        assert!(is_weatherable(BlockId::Dirt));
        assert!(is_weatherable(BlockId::Sand));
        assert!(!is_weatherable(BlockId::Stone));
        assert!(!is_weatherable(BlockId::Bedrock));
        assert!(!is_weatherable(BlockId::Air));
    }

    /// Single grass block emits 5 flat faces + 1 bumpy 4×4 grid.
    /// Flat face = 4 verts, bumpy = 25 verts. Total = 5*4 + 25 = 45.
    #[test]
    fn single_grass_block_emits_bumpy_top() {
        let coord = ChunkCoord::new(0, 0, 0);
        let mut chunk = empty_chunk(coord);
        chunk.set(10, 10, 10, BlockId::Grass);
        let octree = OctreeArray::from_chunk(&chunk);
        let mesh = mesh_chunk_octree_cpu_reference(coord, &octree);

        // 5 flat faces * 4 verts + 1 bumpy face * 25 verts = 45 verts
        assert_eq!(mesh.vertices.len(), 5 * 4 + 25,
            "expected 45 verts (5 flat + 1 bumpy top)");
        // 5 flat * 6 indices + 1 bumpy * 96 indices = 30 + 96 = 126
        assert_eq!(mesh.indices.len(), 5 * 6 + 16 * 6,
            "expected 126 indices (5 flat * 6 + 16 sub-quads * 6)");
        assert!(check_mesh_invariants(&mesh, coord).is_ok());
    }

    /// Single stone block (non-weatherable) emits 6 flat faces.
    #[test]
    fn single_stone_block_emits_flat() {
        let coord = ChunkCoord::new(0, 0, 0);
        let mut chunk = empty_chunk(coord);
        chunk.set(10, 10, 10, BlockId::Stone);
        let octree = OctreeArray::from_chunk(&chunk);
        let mesh = mesh_chunk_octree_cpu_reference(coord, &octree);

        assert_eq!(mesh.vertices.len(), 6 * 4, "expected 24 verts (all flat)");
        assert_eq!(mesh.indices.len(), 6 * 6, "expected 36 indices");
        assert!(check_mesh_invariants(&mesh, coord).is_ok());
    }

    /// Two adjacent grass blocks: each emits 5 flat side faces
    /// (one of each block's side faces is occluded by the neighbor)
    /// and a bumpy top. Their bumpy tops share a row of vertices
    /// (the shared edge) but each emits its own 5×5 grid.
    #[test]
    fn two_grass_blocks_bumpy_tops_in_world_aabb() {
        let coord = ChunkCoord::new(2, 1, -1); // arbitrary non-origin
        let mut chunk = empty_chunk(coord);
        chunk.set(5, 5, 5, BlockId::Grass);
        chunk.set(6, 5, 5, BlockId::Grass);  // adjacent +X
        let octree = OctreeArray::from_chunk(&chunk);
        let mesh = mesh_chunk_octree_cpu_reference(coord, &octree);

        assert!(check_mesh_invariants(&mesh, coord).is_ok(),
            "bumpy mesh must satisfy AABB and consistency invariants \
             even for non-origin chunks (this catches v0.24.27-fix3 \
             regression in the bumpy emission path specifically)");

        // 10 outer faces total (6+6 minus 2 shared internal).
        // Of those 10, exactly 2 are bumpy tops (one per block,
        // both Grass = weatherable). The other 8 are flat (sides
        // and bottoms).
        // Verts: 8 flat * 4 + 2 bumpy * 25 = 32 + 50 = 82.
        assert_eq!(mesh.vertices.len(), 8 * 4 + 2 * 25,
            "expected 8 flat side/bottom faces + 2 bumpy tops");
    }

    /// Verify bumpy emission writes vertices into the correct WORLD
    /// AABB. This is the specific regression test for the bumpy
    /// path of v0.24.27-fix3.
    #[test]
    fn bumpy_world_coord_regression() {
        let coord = ChunkCoord::new(1, 2, -1); // origin (32, 64, -32)
        let mut chunk = empty_chunk(coord);
        chunk.set(5, 6, 7, BlockId::Grass);
        let octree = OctreeArray::from_chunk(&chunk);
        let mesh = mesh_chunk_octree_cpu_reference(coord, &octree);

        // World position of the block: (37, 70, -25)
        // Top face at world y = 71 (with negative bump making it slightly less).
        // Sub-vertices in x [37, 38], y [70.65, 71], z [-25, -24].
        for v in &mesh.vertices {
            let [vx, vy, vz] = v.position;
            assert!((37.0..=38.0).contains(&vx),
                "vertex x={} out of expected [37, 38]", vx);
            assert!((70.0..=71.0).contains(&vy),
                "vertex y={} out of expected [70, 71] — bumpy_height \
                 should keep Y in [py+1+min_bump, py+1]", vy);
            assert!((-25.0..=-24.0).contains(&vz),
                "vertex z={} out of expected [-25, -24]", vz);
        }
        assert!(check_mesh_invariants(&mesh, coord).is_ok());
    }

    /// `hash2d` produces a value in [0, 1] for any input. This
    /// pins the contract for the GLSL-equivalent function.
    #[test]
    fn hash2d_returns_unit_interval() {
        for x in -20..20 {
            for z in -20..20 {
                let h = hash2d(x as f32 * 0.3, z as f32 * 0.7);
                assert!((0.0..=1.0).contains(&h),
                    "hash2d({}, {}) = {} not in [0, 1]", x, z, h);
            }
        }
    }

    /// Asymmetric input order should produce different hash values
    /// (the function should NOT be symmetric in (x, z)).
    /// v0.24.38-bumpy-asym: hash now uses distinct per-axis
    /// constants so this test asserts asymmetry. The earlier
    /// symmetric form was documented as a known weakness; that
    /// weakness is now fixed.
    #[test]
    fn hash2d_uses_both_axes_meaningfully() {
        let h_ab = hash2d(0.3, 0.7);
        let h_ba = hash2d(0.7, 0.3);
        assert!(
            (h_ab - h_ba).abs() > 0.01,
            "hash2d must differ under x↔z swap; got h(0.3, 0.7)={} \
             h(0.7, 0.3)={} (diff={})",
            h_ab, h_ba, (h_ab - h_ba).abs()
        );
    }

    // ---- Cross-chunk neighbor tests (v0.24.30) ----

    /// Stone block at chunk-local (31, 10, 10) in chunk (0,0,0).
    /// Adjacent chunk (1,0,0) has stone at chunk-local (0, 10, 10).
    /// Without neighbor lookup, BOTH chunks emit a face at the
    /// world-x=32 plane (one +X face from chunk 0, one -X face
    /// from chunk 1 — double-emission, z-fights).
    /// WITH neighbor lookup, chunk 0 sees chunk 1's stone via the
    /// +X neighbor and DOESN'T emit its +X face.
    #[test]
    fn neighbor_lookup_eliminates_seam_face() {
        let coord_a = ChunkCoord::new(0, 0, 0);
        let coord_b = ChunkCoord::new(1, 0, 0);

        let mut chunk_a = empty_chunk(coord_a);
        chunk_a.set(31, 10, 10, BlockId::Stone);
        let octree_a = OctreeArray::from_chunk(&chunk_a);

        let mut chunk_b = empty_chunk(coord_b);
        chunk_b.set(0, 10, 10, BlockId::Stone);
        let octree_b = OctreeArray::from_chunk(&chunk_b);

        // Mesh chunk A without neighbors (open boundary): expect 6 faces.
        let mesh_open = mesh_chunk_octree_cpu_reference(coord_a, &octree_a);
        assert_eq!(mesh_open.vertices.len(), 6 * 4,
            "open-boundary: stone block at chunk edge emits all 6 faces");

        // Mesh chunk A WITH +X neighbor as chunk B: expect 5 faces.
        let neighbors = Neighbors {
            px: Some(&octree_b),
            ..Default::default()
        };
        let mesh_closed = mesh_chunk_octree_cpu_reference_with_neighbors(
            coord_a, &octree_a, &neighbors,
        );
        assert_eq!(mesh_closed.vertices.len(), 5 * 4,
            "neighbor-aware: stone block with solid +X neighbor \
             emits only 5 faces (no +X seam face)");
    }

    /// Symmetric test: chunk B (the neighbor) also shouldn't emit
    /// its -X face when chunk A has a stone block at the boundary.
    #[test]
    fn neighbor_lookup_symmetric() {
        let coord_a = ChunkCoord::new(0, 0, 0);
        let coord_b = ChunkCoord::new(1, 0, 0);

        let mut chunk_a = empty_chunk(coord_a);
        chunk_a.set(31, 10, 10, BlockId::Stone);
        let octree_a = OctreeArray::from_chunk(&chunk_a);

        let mut chunk_b = empty_chunk(coord_b);
        chunk_b.set(0, 10, 10, BlockId::Stone);
        let octree_b = OctreeArray::from_chunk(&chunk_b);

        let neighbors_b = Neighbors {
            nx: Some(&octree_a),
            ..Default::default()
        };
        let mesh_b = mesh_chunk_octree_cpu_reference_with_neighbors(
            coord_b, &octree_b, &neighbors_b,
        );
        assert_eq!(mesh_b.vertices.len(), 5 * 4,
            "chunk B's stone block with solid -X neighbor emits 5 faces");
    }

    /// Neighbor lookup at world boundary: a stone block at the +Y
    /// face of its chunk, with the +Y neighbor being all-air,
    /// should still emit its +Y face (just like open-boundary).
    #[test]
    fn neighbor_lookup_air_neighbor_still_emits() {
        let coord_a = ChunkCoord::new(0, 0, 0);
        let coord_b = ChunkCoord::new(0, 1, 0); // +Y neighbor

        let mut chunk_a = empty_chunk(coord_a);
        chunk_a.set(10, 31, 10, BlockId::Stone);
        let octree_a = OctreeArray::from_chunk(&chunk_a);

        let chunk_b = empty_chunk(coord_b);  // all air
        let octree_b = OctreeArray::from_chunk(&chunk_b);

        let neighbors = Neighbors {
            py: Some(&octree_b),
            ..Default::default()
        };
        let mesh = mesh_chunk_octree_cpu_reference_with_neighbors(
            coord_a, &octree_a, &neighbors,
        );
        assert_eq!(mesh.vertices.len(), 6 * 4,
            "stone block with all-air +Y neighbor still emits all 6 faces \
             (the +Y face IS exposed)");
    }

    /// Bumpy interaction with cross-chunk: grass block at chunk
    /// boundary with grass neighbor across the seam. The +X face
    /// is occluded, so no bumpy emission on that side. Top face
    /// (+Y) is still exposed and still bumpy.
    #[test]
    fn cross_chunk_grass_top_face_still_bumpy() {
        let coord_a = ChunkCoord::new(0, 0, 0);
        let coord_b = ChunkCoord::new(1, 0, 0);

        let mut chunk_a = empty_chunk(coord_a);
        chunk_a.set(31, 10, 10, BlockId::Grass);
        let octree_a = OctreeArray::from_chunk(&chunk_a);

        let mut chunk_b = empty_chunk(coord_b);
        chunk_b.set(0, 10, 10, BlockId::Grass);  // adjacent across seam
        let octree_b = OctreeArray::from_chunk(&chunk_b);

        let neighbors = Neighbors {
            px: Some(&octree_b),
            ..Default::default()
        };
        let mesh = mesh_chunk_octree_cpu_reference_with_neighbors(
            coord_a, &octree_a, &neighbors,
        );
        // 4 flat faces (-X, -Y, +Z, -Z) + 1 bumpy top = 4*4 + 25 = 41
        // (the +X face is occluded by the cross-chunk neighbor)
        assert_eq!(mesh.vertices.len(), 4 * 4 + 25,
            "expected 4 flat + 1 bumpy (cross-chunk neighbor occludes +X face)");
    }
}
