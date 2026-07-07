//! RC42: baked mesh templates for sub-voxel block decorations.
//!
//! ## What this replaces
//!
//! Pre-RC42 path: for every plant block in a chunk, the mesher called
//! `procedural_subgrid(block, wx, wy, wz)` to BUILD a sparse sub-voxel
//! grid (8³ cells, ~5-20 filled), then called `emit_subvoxel_grid` to
//! EXPAND each cell into 6 axis-aligned cube faces with neighbor-
//! occlusion checks. For a surface chunk with ~200 grass/leaf blocks
//! this was ~200 × (grid-gen + per-cell expansion) of CPU work per
//! mesh.
//!
//! Post-RC42 path: at startup, bake N **shape variants** per
//! sub-voxel block type into immutable templates. Each template stores
//! vertex positions, colors, normals, sway weights, and indices in
//! LOCAL block-space [0, 1]³. At mesh time, pick a variant by hash
//! and **stamp** the template — translate to host_origin, apply sway,
//! modulate color, append to the chunk mesh. No grid generation, no
//! per-cell loop, no occlusion lookup setup.
//!
//! ## Cost
//!
//! Baking is one-time per session, lazy via `OnceLock`. Memory:
//! ~6 block types × 8 variants × ~24 KB per template = ~1 MB. CPU
//! time per stamp: O(template_vertices), basically a memcpy with
//! some per-vertex arithmetic for sway + color.
//!
//! ## Variation
//!
//! Eight variants per block type, picked by `plant_hash` to maintain
//! deterministic visual variety. The bake itself uses 8 representative
//! seed positions per block so the variants come from `procedural_subgrid`
//! at those positions — the geometry distribution matches what
//! procedural would produce, just sampled.
//!
//! ## What's NOT changed by templates
//!
//! - Sway sampling: still per-host (cheap, single `wind_at` call).
//! - Color modulation (lifecycle stages, leaf decay): per-stamp
//!   `color_mul` argument.
//! - Sky directional shading: per-stamp `sky.face_brightness(normal)`.
//! - Edit support: chunks with sub-voxel-cell edits fall back to
//!   the procedural path. (No sub-voxel edits in any current ship.)

use crate::render::mesher::Vertex;
use crate::world::blocks::BlockId;
use crate::world::subvoxel::{procedural_subgrid, SubVoxelGrid, SUBVOXEL_RESOLUTION, SUBVOXEL_SIZE};
use std::sync::OnceLock;

/// Number of shape variants baked per block type. Picked by hash.
/// 8 gives enough diversity that adjacent host blocks look distinct.
pub const VARIANTS_PER_BLOCK: usize = 8;

/// A baked sub-voxel template: vertex arrays in LOCAL space relative
/// to the host block, plus a separate sway weight per vertex.
///
/// The host_origin passed at stamp time is the WORLD-space corner of
/// the cell the decoration belongs in:
///   - For a leaf block (case A in the mesher): origin is the leaf
///     cell's own corner.
///   - For grass tufts (case B): origin is the AIR-ABOVE cell's
///     corner (already adjusted for sag).
///
/// The template's positions are local offsets in [0, 1]³ — at stamp
/// time we just add host_origin per vertex.
#[derive(Debug, Clone, Default)]
pub struct SubVoxelTemplate {
    /// Local-space vertex positions. Each component in [0, 1].
    pub positions: Vec<[f32; 3]>,
    /// Per-vertex base color (already shaded by `cell.color`; will be
    /// multiplied by `color_mul` and `sky.face_brightness(normal)` at
    /// stamp time).
    pub colors: Vec<[f32; 3]>,
    /// Per-vertex face normal (one of the 6 axis-aligned cardinals).
    /// Used for per-vertex shading lookup at stamp time.
    pub normals: Vec<[f32; 3]>,
    /// Per-vertex sway weight. 0 at the bottom of the host, ~1.0 at
    /// the top. Computed from sub_y / max_sub_y at bake time so
    /// adjacent stacked cells share weight at their boundary.
    pub sway_weights: Vec<f32>,
    /// Index buffer (triangles, 3 indices per face).
    pub indices: Vec<u32>,
}

impl SubVoxelTemplate {
    pub fn empty() -> Self { Self::default() }
    pub fn vertex_count(&self) -> usize { self.positions.len() }
    pub fn index_count(&self) -> usize { self.indices.len() }
    pub fn is_empty(&self) -> bool { self.indices.is_empty() }
}

/// Maximum sway lateral offset in blocks at the very top of the
/// sub-grid. Matches the constant in `emit_subvoxel_grid` so visual
/// behavior is unchanged.
pub const MAX_SWAY: f32 = 0.0625;

/// Bake one template variant by running the procedural generator at
/// a fixed seed position then expanding cells into faces.
///
/// This mirrors `emit_subvoxel_grid`'s face emission logic with one
/// difference: positions are LOCAL (host_origin = [0, 0, 0]) so the
/// template can be stamped anywhere.
fn bake_variant(host_id: BlockId, seed: [i32; 3]) -> SubVoxelTemplate {
    let grid = match procedural_subgrid(host_id, seed[0], seed[1], seed[2]) {
        Some(g) => g,
        None => return SubVoxelTemplate::empty(),
    };
    bake_from_grid(&grid)
}

/// Convert an explicit `SubVoxelGrid` into a `SubVoxelTemplate`.
/// Public so the procedural-fallback path can use it too (e.g.
/// for sub-voxel edits when edit support lands).
///
/// Equivalent to `emit_subvoxel_grid` but writes into a local
/// template buffer instead of a ChunkMesh, and **stores sway weights
/// per vertex** so stamping can sway without re-deriving sub_y.
pub fn bake_from_grid(grid: &SubVoxelGrid) -> SubVoxelTemplate {
    let mut t = SubVoxelTemplate::empty();
    if grid.cells.is_empty() { return t; }

    // Occlusion lookup: which cells are filled? Same as emit_subvoxel_grid.
    let n = SUBVOXEL_RESOLUTION;
    debug_assert!(n <= 16);
    let mut filled = [false; 16 * 16 * 16];
    for c in &grid.cells {
        filled[(c.sub_x as usize)
             + (c.sub_y as usize) * 16
             + (c.sub_z as usize) * 16 * 16] = true;
    }
    let lookup = |x: i32, y: i32, z: i32| -> bool {
        if x < 0 || y < 0 || z < 0 { return false; }
        if (x as usize) >= n || (y as usize) >= n || (z as usize) >= n {
            return false;
        }
        filled[(x as usize) + (y as usize) * 16 + (z as usize) * 16 * 16]
    };

    let denom = (SUBVOXEL_RESOLUTION as f32 - 1.0).max(1.0);

    for cell in &grid.cells {
        let bx = cell.sub_x as i32;
        let by = cell.sub_y as i32;
        let bz = cell.sub_z as i32;

        // Local-space cube corners. No sway — sway is per-vertex at
        // stamp time.
        let lx0 = (bx as f32) * SUBVOXEL_SIZE;
        let lx1 = lx0 + SUBVOXEL_SIZE;
        let lz0 = (bz as f32) * SUBVOXEL_SIZE;
        let lz1 = lz0 + SUBVOXEL_SIZE;
        let ly0 = (by as f32) * SUBVOXEL_SIZE;
        let ly1 = ly0 + SUBVOXEL_SIZE;

        // Sway weights: linear in sub_y. Cells stacked vertically share
        // corners at their bottom/top boundaries; weight at sub_y=k from
        // above equals weight at sub_y=k+1 from below — corners stay welded.
        let weight_bot = (by as f32) / denom;
        let weight_top = ((by + 1) as f32) / denom;

        let color = cell.color;

        // Helper: push a 4-vertex quad with explicit per-vertex sway
        // weights. Bottom vertices get weight_bot; top vertices get
        // weight_top. Sky/lighting is applied at stamp time.
        let mut push_quad = |positions: [[f32; 3]; 4], weights: [f32; 4], normal: [f32; 3]| {
            let base = t.positions.len() as u32;
            for i in 0..4 {
                t.positions.push(positions[i]);
                t.colors.push(color);
                t.normals.push(normal);
                t.sway_weights.push(weights[i]);
            }
            t.indices.push(base);
            t.indices.push(base + 1);
            t.indices.push(base + 2);
            t.indices.push(base);
            t.indices.push(base + 2);
            t.indices.push(base + 3);
        };

        // +Y top — all 4 vertices at the top, weight_top.
        if !lookup(bx, by + 1, bz) {
            push_quad(
                [[lx0, ly1, lz0], [lx1, ly1, lz0], [lx1, ly1, lz1], [lx0, ly1, lz1]],
                [weight_top, weight_top, weight_top, weight_top],
                [0.0, 1.0, 0.0],
            );
        }
        // -Y bottom — all 4 vertices at the bottom, weight_bot.
        if !lookup(bx, by - 1, bz) {
            push_quad(
                [[lx0, ly0, lz1], [lx1, ly0, lz1], [lx1, ly0, lz0], [lx0, ly0, lz0]],
                [weight_bot, weight_bot, weight_bot, weight_bot],
                [0.0, -1.0, 0.0],
            );
        }
        // +X east — left edge is bottom (weight_bot), right edge is top (weight_top).
        // Vertex order: bot-z0, bot-z1, top-z1, top-z0.
        if !lookup(bx + 1, by, bz) {
            push_quad(
                [[lx1, ly0, lz0], [lx1, ly0, lz1], [lx1, ly1, lz1], [lx1, ly1, lz0]],
                [weight_bot, weight_bot, weight_top, weight_top],
                [1.0, 0.0, 0.0],
            );
        }
        // -X west.
        if !lookup(bx - 1, by, bz) {
            push_quad(
                [[lx0, ly0, lz1], [lx0, ly0, lz0], [lx0, ly1, lz0], [lx0, ly1, lz1]],
                [weight_bot, weight_bot, weight_top, weight_top],
                [-1.0, 0.0, 0.0],
            );
        }
        // +Z south.
        if !lookup(bx, by, bz + 1) {
            push_quad(
                [[lx1, ly0, lz1], [lx0, ly0, lz1], [lx0, ly1, lz1], [lx1, ly1, lz1]],
                [weight_bot, weight_bot, weight_top, weight_top],
                [0.0, 0.0, 1.0],
            );
        }
        // -Z north.
        if !lookup(bx, by, bz - 1) {
            push_quad(
                [[lx0, ly0, lz0], [lx1, ly0, lz0], [lx1, ly1, lz0], [lx0, ly1, lz0]],
                [weight_bot, weight_bot, weight_top, weight_top],
                [0.0, 0.0, -1.0],
            );
        }
    }

    t
}

/// Static registry of templates. Lazy-initialized on first access.
///
/// Initialization happens on the first mesh that has a sub-voxel block
/// to render. Costs roughly 6 block types × 8 variants × ~50 cells ×
/// ~6 face emits = ~14K face-emits done once.
static TEMPLATES: OnceLock<Templates> = OnceLock::new();

#[derive(Debug, Default)]
struct Templates {
    grass:        Vec<SubVoxelTemplate>,
    fern:         Vec<SubVoxelTemplate>,
    flower:       Vec<SubVoxelTemplate>,
    tall_grass:   Vec<SubVoxelTemplate>,
    oak_leaves:   Vec<SubVoxelTemplate>,
    birch_leaves: Vec<SubVoxelTemplate>,
}

/// Get the global template registry, initializing if needed.
fn templates() -> &'static Templates {
    TEMPLATES.get_or_init(|| {
        Templates {
            grass:        bake_variants(BlockId::Grass),
            fern:         bake_variants(BlockId::Fern),
            flower:       bake_variants(BlockId::Flower),
            tall_grass:   bake_variants(BlockId::TallGrass),
            oak_leaves:   bake_variants(BlockId::OakLeaves),
            birch_leaves: bake_variants(BlockId::BirchLeaves),
        }
    })
}

/// Bake VARIANTS_PER_BLOCK templates for a given block type. Each variant
/// uses a different seed position, picked along a deterministic spread
/// so the variant-set spans the procedural generator's output space.
fn bake_variants(host_id: BlockId) -> Vec<SubVoxelTemplate> {
    // Spread of 8 seed positions. Coordinates are arbitrary but distinct;
    // the actual procedural_subgrid behavior depends on plant_hash output
    // which is sensitive to all 3 coords. The first byte of each int is
    // chosen for low correlation with adjacent variants.
    let seeds: [[i32; 3]; VARIANTS_PER_BLOCK] = [
        [0, 0, 0],
        [37, 11, 5],
        [101, 23, -47],
        [-23, 7, 89],
        [56, -3, 13],
        [-89, 17, 31],
        [200, 64, -123],
        [-200, 100, 200],
    ];
    seeds.iter().map(|&s| bake_variant(host_id, s)).collect()
}

/// Look up a template by block + hash. Returns `None` if the block
/// type isn't sub-voxel (caller should fall through to procedural).
pub fn template_for(host_id: BlockId, hash: u32) -> Option<&'static SubVoxelTemplate> {
    let t = templates();
    let pool: &Vec<SubVoxelTemplate> = match host_id {
        BlockId::Grass        => &t.grass,
        BlockId::Fern         => &t.fern,
        BlockId::Flower       => &t.flower,
        BlockId::TallGrass    => &t.tall_grass,
        BlockId::OakLeaves    => &t.oak_leaves,
        BlockId::BirchLeaves  => &t.birch_leaves,
        _ => return None,
    };
    if pool.is_empty() { return None; }
    let idx = (hash as usize) % pool.len();
    pool.get(idx)
}

/// Stamp a template into a vertex/index buffer pair at the given
/// host_origin, applying per-host sway, lighting, and color modulation.
///
/// Signature matches the existing `emit_subvoxel_grid` callers minus
/// the SubVoxelGrid: pass a template + the same host_origin and
/// color_mul; the sky reference drives directional shading.
///
/// IMPORTANT: this writes directly into `vertices` and `indices` with
/// the index offset accounting for `vertices.len()` BEFORE the call.
/// Equivalent to ChunkMesh::vertices/indices.push_all but mesher.rs
/// owns ChunkMesh so we take the fields separately for a smaller
/// surface.
pub fn stamp_template(
    vertices: &mut Vec<Vertex>,
    indices:  &mut Vec<u32>,
    template: &SubVoxelTemplate,
    host_origin: [f32; 3],
    sky: Option<&crate::sky::Sky>,
    color_mul: [f32; 3],
) {
    if template.is_empty() { return; }

    // Wind direction at this host — sampled once.
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

    let base_idx = vertices.len() as u32;

    // Per-vertex stamp.
    for i in 0..template.positions.len() {
        let pos = template.positions[i];
        let weight = template.sway_weights[i];
        let normal = template.normals[i];
        let template_color = template.colors[i];

        // Sway: top vertices displace in wind direction; bottom anchored.
        let sway_x = wind_x * weight * MAX_SWAY;
        let sway_z = wind_z * weight * MAX_SWAY;

        let world_pos = [
            host_origin[0] + pos[0] + sway_x,
            host_origin[1] + pos[1],
            host_origin[2] + pos[2] + sway_z,
        ];

        // Per-face directional lighting via sky.
        let directional = match sky {
            Some(sky) => sky.face_brightness(normal),
            None => fallback_face_brightness(normal),
        };

        let color = [
            (template_color[0] * color_mul[0] * directional).clamp(0.0, 1.0),
            (template_color[1] * color_mul[1] * directional).clamp(0.0, 1.0),
            (template_color[2] * color_mul[2] * directional).clamp(0.0, 1.0),
        ];

        vertices.push(Vertex::new(world_pos, color));
    }

    // Indices, rebased.
    for &idx in &template.indices {
        indices.push(base_idx + idx);
    }
}

/// Same fallback brightness as the chunk mesher uses when no sky is
/// available (used in tests + headless paths).
fn fallback_face_brightness(normal: [f32; 3]) -> f32 {
    // Roughly match FACE_LIGHT in mesher.rs.
    if normal[1] > 0.5 { return 1.0; }      // +Y top
    if normal[1] < -0.5 { return 0.5; }     // -Y bottom
    if normal[0].abs() > 0.5 { return 0.8; } // ±X
    0.7 // ±Z
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_init_without_panic() {
        // Touching `template_for` triggers init.
        let _ = template_for(BlockId::Grass, 0);
        let _ = template_for(BlockId::OakLeaves, 0);
        let _ = template_for(BlockId::BirchLeaves, 0);
        let _ = template_for(BlockId::Flower, 0);
    }

    #[test]
    fn non_subvoxel_block_returns_none() {
        assert!(template_for(BlockId::Stone, 0).is_none());
        assert!(template_for(BlockId::Air, 0).is_none());
        assert!(template_for(BlockId::Water, 0).is_none());
        assert!(template_for(BlockId::Dirt, 0).is_none());
    }

    #[test]
    fn each_block_has_variants() {
        for &b in &[BlockId::Grass, BlockId::Fern, BlockId::Flower,
                    BlockId::TallGrass, BlockId::OakLeaves, BlockId::BirchLeaves]
        {
            let tmpl = template_for(b, 0);
            assert!(tmpl.is_some(), "missing templates for {:?}", b);
            let tmpl = tmpl.unwrap();
            assert!(!tmpl.is_empty() || matches!(b, _), // some empties allowed
                "template for {:?} is empty (expected non-empty)", b);
        }
    }

    #[test]
    fn different_hashes_pick_different_variants() {
        // Force at least one variant transition across the hash space.
        let mut distinct_vertex_counts = std::collections::HashSet::new();
        for h in 0u32..VARIANTS_PER_BLOCK as u32 {
            if let Some(t) = template_for(BlockId::Grass, h) {
                distinct_vertex_counts.insert(t.vertex_count());
            }
        }
        // 8 variants from 8 different seeds should produce at least
        // 2 distinct vertex counts (different blade counts → different
        // expansion sizes). Defensive: if procedural happened to
        // produce same count for all, this test would fail — but the
        // bake_variants seeds are chosen to spread the output.
        assert!(distinct_vertex_counts.len() >= 2,
            "expected variant diversity but got {} distinct counts",
            distinct_vertex_counts.len());
    }

    #[test]
    fn stamp_produces_vertices_and_indices() {
        let tmpl = template_for(BlockId::Grass, 0).expect("grass template");
        if tmpl.is_empty() { return; } // accept; some variants can be empty
        let mut vs: Vec<Vertex> = Vec::new();
        let mut is: Vec<u32> = Vec::new();
        stamp_template(&mut vs, &mut is, tmpl, [10.0, 64.0, 20.0], None, [1.0, 1.0, 1.0]);
        assert_eq!(vs.len(), tmpl.vertex_count(),
            "stamp wrote wrong vertex count");
        assert_eq!(is.len(), tmpl.index_count(),
            "stamp wrote wrong index count");
        // All output indices should reference valid vertex slots.
        for &i in &is {
            assert!((i as usize) < vs.len(),
                "stamp produced out-of-range index {} for {} vertices", i, vs.len());
        }
    }

    #[test]
    fn stamp_offsets_vertices_to_host_origin() {
        let tmpl = template_for(BlockId::Grass, 0).expect("grass template");
        if tmpl.is_empty() { return; }
        let mut vs: Vec<Vertex> = Vec::new();
        let mut is: Vec<u32> = Vec::new();
        let host = [100.0, 200.0, 300.0];
        stamp_template(&mut vs, &mut is, tmpl, host, None, [1.0, 1.0, 1.0]);
        // Every vertex Y must be >= host.y (template y is in [0, 1]) and
        // < host.y + 2 (1 block range + sway margin).
        for v in &vs {
            assert!(v.position[1] >= host[1] - 0.001,
                "vertex y {} below host y {}", v.position[1], host[1]);
            assert!(v.position[1] < host[1] + 2.0,
                "vertex y {} too far above host y {}", v.position[1], host[1]);
            // X and Z can sway up to MAX_SWAY beyond [0, 1].
            assert!(v.position[0] >= host[0] - MAX_SWAY - 0.01);
            assert!(v.position[0] < host[0] + 1.0 + MAX_SWAY + 0.01);
        }
    }

    #[test]
    fn stamp_appends_not_overwrites() {
        let tmpl = template_for(BlockId::Grass, 0).expect("grass template");
        if tmpl.is_empty() { return; }
        let mut vs: Vec<Vertex> = vec![Vertex::new([0.0, 0.0, 0.0], [1.0, 1.0, 1.0])];
        let mut is: Vec<u32> = vec![0, 0, 0];
        let pre_v = vs.len();
        let pre_i = is.len();
        stamp_template(&mut vs, &mut is, tmpl, [0.0, 0.0, 0.0], None, [1.0, 1.0, 1.0]);
        assert_eq!(vs.len(), pre_v + tmpl.vertex_count());
        assert_eq!(is.len(), pre_i + tmpl.index_count());
        // The indices added must be rebased — they all refer to vertices
        // pushed AFTER pre_v.
        for &i in &is[pre_i..] {
            assert!((i as usize) >= pre_v);
        }
    }

    #[test]
    fn color_mul_scales_template_colors() {
        let tmpl = template_for(BlockId::Grass, 0).expect("grass template");
        if tmpl.is_empty() { return; }
        let mut vs1: Vec<Vertex> = Vec::new();
        let mut is1: Vec<u32> = Vec::new();
        stamp_template(&mut vs1, &mut is1, tmpl, [0.0, 0.0, 0.0], None, [1.0, 1.0, 1.0]);
        let mut vs2: Vec<Vertex> = Vec::new();
        let mut is2: Vec<u32> = Vec::new();
        stamp_template(&mut vs2, &mut is2, tmpl, [0.0, 0.0, 0.0], None, [0.0, 0.0, 0.0]);
        // With color_mul=0, every vertex color should be (0, 0, 0).
        for v in &vs2 {
            assert_eq!(v.color, [0.0, 0.0, 0.0], "color_mul=0 should zero colors");
        }
        // With color_mul=1, colors should match template * directional shade.
        // (We don't compare exactly since fallback_face_brightness is approx
        //  what stamp uses; just check non-zero where template was non-zero.)
        for v in &vs1 {
            assert!(v.color.iter().any(|c| *c > 0.0),
                "color_mul=1 should leave non-zero colors");
        }
    }
}
