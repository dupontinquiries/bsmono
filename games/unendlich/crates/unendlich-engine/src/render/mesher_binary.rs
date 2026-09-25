//! The chunk mesher on `world::bitchunk`.
//!
//! Same output as `mesher::mesh_chunk_greedy_with_neighbors` — literally
//! the same emit functions (`emit_quad`, `emit_jittered_top`,
//! `emit_subdivided_top`, `emit_soft_side_face`, `decorate_chunk`, water,
//! lava) — with the two expensive scans replaced:
//!
//! 1. **Visibility.** The greedy mesher asks `should_emit_face(inner,
//!    neighbour)` for every cell of every slice of every axis: 6 × 32³
//!    LUT probes plus a bounds-checked neighbour read each. Here the
//!    chunk is scanned ONCE into bit sets (opaque occluders; each
//!    transparent meshable type; the special-emit cells) and each
//!    direction's exposed faces are one bit-op per column
//!    (`bitchunk::face_mask_between`). The engine's rule
//!    (`BlockId::produces_face_against`) is reproduced exactly: a meshable
//!    cube shows a face where the neighbour is not an opaque occluder,
//!    except that a transparent type does not face itself.
//! 2. **Merging.** Runs are found with trailing-zeros / mask tricks on
//!    the face bits (`bitchunk::greedy_mask`) instead of the
//!    `Option<BlockId>` mask walk. The merge key is the block id, or a
//!    unique key when `material_response::block_can_merge` says no —
//!    the same two rules the greedy mesher applies.
//!
//! Cells the greedy mesher hands to a special emitter (tops that
//! `wants_top_jitter`, sides with `softness() > 0`) go to the SAME
//! special emitter here, per cell, after the bit visibility test — so the
//! picture is unchanged and the parity test can assert coverage equality
//! on plain chunks. The rectangles differ (this merges along v then u; the
//! greedy along u then v), the covered cells do not.
//!
//! Selected by `UNENDLICH_MESHER=binary` in the meshing pool. Not the
//! default until it has been seen on hardware.

use crate::render::mesher::{
    self, AxisConfig, ChunkMesh, NeighborChunks, AXIS_CONFIGS, face,
};
use crate::world::bitchunk::{
    face_mask_between, for_each_cell, greedy_mask, BitChunk, Face, NeighbourLayer, Quad, N, NN,
};
use crate::world::blocks::BlockId;
use crate::world::chunk::Chunk;

/// What one mesh pass did, for the diag line and the parity test.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BinaryMeshStats {
    /// Exposed cube faces found by the bit test (all six directions).
    pub faces: u32,
    /// Faces merged into plain quads.
    pub plain_quads: u32,
    /// Faces routed to a special emitter (jittered/subdivided top, soft side).
    pub special_faces: u32,
    /// Vertices the cube faces produced (plain + special), before decoration.
    pub face_vertices: u32,
    /// Vertices added by `decorate_chunk` + water + lava. On a grass
    /// surface chunk this is ~124 per grass block — half the mesh.
    pub decoration_vertices: u32,
}

/// Which `AXIS_CONFIGS` entry a `bitchunk::Face` maps to, and whether the
/// (u, v) axes have to be swapped between the two conventions.
/// `bitchunk`: ±X → (u=y, v=z); ±Y → (u=x, v=z); ±Z → (u=x, v=y).
/// `mesher`:   ±X → (u=z, v=y); ±Y → (u=x, v=z); ±Z → (u=y, v=x).
fn cfg_for(face_dir: Face) -> (&'static AxisConfig, bool) {
    let want = match face_dir {
        Face::PosY => face::TOP,
        Face::NegY => face::BOTTOM,
        Face::PosX => face::EAST,
        Face::NegX => face::WEST,
        Face::PosZ => face::SOUTH,
        Face::NegZ => face::NORTH,
    };
    let cfg = AXIS_CONFIGS.iter().find(|c| c.face == want).expect("every face has a config");
    let swap = matches!(face_dir, Face::PosX | Face::NegX | Face::PosZ | Face::NegZ);
    (cfg, swap)
}

/// The neighbour chunk beyond `face_dir`, if loaded.
fn neighbour_for(n: &NeighborChunks, face_dir: Face) -> Option<&Chunk> {
    match face_dir {
        Face::PosX => n.pos_x.as_deref(),
        Face::NegX => n.neg_x.as_deref(),
        Face::PosY => n.pos_y.as_deref(),
        Face::NegY => n.neg_y.as_deref(),
        Face::PosZ => n.pos_z.as_deref(),
        Face::NegZ => n.neg_z.as_deref(),
    }
}

/// The 32×32 boundary layer of `neighbour` that touches this chunk on
/// `face_dir` (i.e. the neighbour's opposite face), as the row/bit layout
/// `face_mask_between` expects, filtered by `pred`. A neighbour that isn't
/// loaded is open (`None`), matching `block_at_with_neighbors` → Air.
fn neighbour_layer(neighbour: Option<&Chunk>, face_dir: Face, pred: impl Fn(BlockId) -> bool) -> NeighbourLayer {
    let Some(nc) = neighbour else { return NeighbourLayer(None) };
    let n = N as i32;
    let mut out = [0u32; N];
    match face_dir {
        // ±X: row = y, bit = z; the touching plane is the neighbour's x=0 (for +X) or x=31 (for -X).
        Face::PosX | Face::NegX => {
            let x = if face_dir == Face::PosX { 0 } else { n - 1 };
            for y in 0..n { for z in 0..n {
                if pred(nc.blocks[Chunk::block_index(x, y, z)]) { out[y as usize] |= 1 << z; }
            }}
        }
        // ±Y: row = x, bit = z.
        Face::PosY | Face::NegY => {
            let y = if face_dir == Face::PosY { 0 } else { n - 1 };
            for x in 0..n { for z in 0..n {
                if pred(nc.blocks[Chunk::block_index(x, y, z)]) { out[x as usize] |= 1 << z; }
            }}
        }
        // ±Z: row = x, bit = y.
        Face::PosZ | Face::NegZ => {
            let z = if face_dir == Face::PosZ { 0 } else { n - 1 };
            for x in 0..n { for y in 0..n {
                if pred(nc.blocks[Chunk::block_index(x, y, z)]) { out[x as usize] |= 1 << y; }
            }}
        }
    }
    NeighbourLayer(Some(out))
}

fn or_layers(a: NeighbourLayer, b: NeighbourLayer) -> NeighbourLayer {
    match (a.0, b.0) {
        (None, None) => NeighbourLayer(None),
        (Some(x), None) | (None, Some(x)) => NeighbourLayer(Some(x)),
        (Some(x), Some(y)) => {
            let mut o = [0u32; N];
            for i in 0..N { o[i] = x[i] | y[i]; }
            NeighbourLayer(Some(o))
        }
    }
}

/// The scan: one pass over the chunk into the sets the six directions need.
struct Sets {
    /// Meshable cubes that occlude (not transparent).
    opaque: BitChunk,
    /// Meshable cubes that are transparent (glass, ice, leaves…), one set
    /// per type present — a transparent type does not face itself.
    transparent: Vec<(BlockId, BitChunk)>,
    /// Cells whose TOP face goes to the jittered/subdivided emitter.
    jitter_top: BitChunk,
    /// Cells whose SIDE faces go to the soft-side emitter.
    soft_side: BitChunk,
    any_jitter: bool,
    any_soft: bool,
}

fn scan(chunk: &Chunk) -> Sets {
    let mut s = Sets {
        opaque: BitChunk::empty(),
        transparent: Vec::new(),
        jitter_top: BitChunk::empty(),
        soft_side: BitChunk::empty(),
        any_jitter: false,
        any_soft: false,
    };
    let n = N as i32;
    for y in 0..n {
        for z in 0..n {
            for x in 0..n {
                let b = chunk.blocks[Chunk::block_index(x, y, z)];
                if !mesher::is_meshable(b) { continue; }
                let (xu, yu, zu) = (x as usize, y as usize, z as usize);
                if b.is_transparent() {
                    match s.transparent.iter_mut().find(|(t, _)| *t == b) {
                        Some((_, set)) => set.set(xu, yu, zu),
                        None => {
                            let mut set = BitChunk::empty();
                            set.set(xu, yu, zu);
                            s.transparent.push((b, set));
                        }
                    }
                } else {
                    s.opaque.set(xu, yu, zu);
                }
                if b.wants_top_jitter() { s.jitter_top.set(xu, yu, zu); s.any_jitter = true; }
                if b.softness() > 0.0 { s.soft_side.set(xu, yu, zu); s.any_soft = true; }
            }
        }
    }
    s
}

/// Which faces bypass the special emitters and become plain merged
/// quads. `UNENDLICH_MESHER_PLAIN_TOPS=1` / `UNENDLICH_MESHER_PLAIN_SIDES=1`.
/// The timing report shows why the knobs exist: on a grass/dirt surface
/// chunk the special emitters produce ~35 vertices per face (236 K
/// vertices per chunk) and ~13 of 16 ms; plain quads are 4 vertices and
/// merge. It is a visual decision, so it is a knob and not a default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BinaryOpts {
    pub plain_tops: bool,
    pub plain_sides: bool,
    /// Grass tuft style override; `None` = `TuftStyle::from_env()`.
    pub tuft_style: Option<mesher::TuftStyle>,
    /// A/B only: recompute the face brightness inside every `emit_quad`
    /// (four trig calls per quad) instead of hoisting the six values once
    /// per chunk. Output-identical; exists so `light_hoist_ab` can
    /// measure both in one process.
    pub legacy_per_quad_light: bool,
}

impl BinaryOpts {
    pub fn from_env() -> BinaryOpts {
        static V: std::sync::OnceLock<BinaryOpts> = std::sync::OnceLock::new();
        *V.get_or_init(|| {
            let on = |k: &str| std::env::var(k).map(|v| v == "1" || v.eq_ignore_ascii_case("true")).unwrap_or(false);
            let o = BinaryOpts { plain_tops: on("UNENDLICH_MESHER_PLAIN_TOPS"), plain_sides: on("UNENDLICH_MESHER_PLAIN_SIDES"), tuft_style: None, legacy_per_quad_light: false };
            if o.plain_tops || o.plain_sides {
                log::info!("binary mesher: plain_tops={} plain_sides={} (special emitters bypassed)", o.plain_tops, o.plain_sides);
            }
            o
        })
    }
}

/// Mesh `chunk` with `bitchunk` visibility + merging and the engine's own
/// emitters. Drop-in for `mesh_chunk_greedy_with_neighbors`.
pub fn mesh_chunk_binary(
    chunk: &Chunk,
    neighbors: &NeighborChunks,
    sky: Option<&crate::sky::Sky>,
    world_model: Option<&std::sync::Arc<crate::world::world_model::WorldModel>>,
    current_tick: Option<u64>,
) -> ChunkMesh {
    mesh_chunk_binary_with_stats(chunk, neighbors, sky, world_model, current_tick).0
}

pub fn mesh_chunk_binary_with_stats(
    chunk: &Chunk,
    neighbors: &NeighborChunks,
    sky: Option<&crate::sky::Sky>,
    world_model: Option<&std::sync::Arc<crate::world::world_model::WorldModel>>,
    current_tick: Option<u64>,
) -> (ChunkMesh, BinaryMeshStats) {
    mesh_chunk_binary_opts(chunk, neighbors, sky, world_model, current_tick, BinaryOpts::from_env())
}

pub fn mesh_chunk_binary_opts(
    chunk: &Chunk,
    neighbors: &NeighborChunks,
    sky: Option<&crate::sky::Sky>,
    world_model: Option<&std::sync::Arc<crate::world::world_model::WorldModel>>,
    current_tick: Option<u64>,
    opts: BinaryOpts,
) -> (ChunkMesh, BinaryMeshStats) {
    let mut mesh = ChunkMesh::empty();
    let mut stats = BinaryMeshStats::default();
    mesher::corner_cache_reset();
    mesher::env_cache_reset();
    mesher::tint_cache_reset();
    let (ox, oy, oz) = chunk.coord.block_origin();
    let origin = [ox, oy, oz];

    if !chunk.cached_all_air {
        let sets = scan(chunk);
        // Transparent types need "opaque ∪ same type" as the blocker.
        let trans_blockers: Vec<BitChunk> = sets.transparent.iter()
            .map(|(_, set)| sets.opaque.union(set)).collect();

        // Four trig calls each (Sky::sun_direction + directional_intensity),
        // constant per direction for the whole chunk.
        let lights = mesher::face_lights(sky);
        let mut quads: Vec<Quad> = Vec::with_capacity(2048);
        for face_dir in [Face::PosY, Face::NegY, Face::PosX, Face::NegX, Face::PosZ, Face::NegZ] {
            let nb = neighbour_for(neighbors, face_dir);
            let opaque_layer = neighbour_layer(nb, face_dir, |b| b.is_opaque_occluder());

            // 1. Visible faces: opaque cubes, then each transparent type.
            let mut visible = face_mask_between(&sets.opaque, &sets.opaque, face_dir, opaque_layer);
            for ((t, set), blocker) in sets.transparent.iter().zip(trans_blockers.iter()) {
                let t_layer = neighbour_layer(nb, face_dir, |b| b == *t);
                let m = face_mask_between(set, blocker, face_dir, or_layers(opaque_layer, t_layer));
                for i in 0..NN { visible[i] |= m[i]; }
            }
            let face_count: u32 = visible.iter().map(|m| m.count_ones()).sum();
            if face_count == 0 { continue; }
            stats.faces += face_count;

            // 2. Split off the special-emit cells.
            let (cfg, swap) = cfg_for(face_dir);
            let is_top = face_dir == Face::PosY;
            let is_side = matches!(face_dir, Face::PosX | Face::NegX | Face::PosZ | Face::NegZ);
            let special_set = if is_top && sets.any_jitter && !opts.plain_tops { Some(&sets.jitter_top) }
                else if is_side && sets.any_soft && !opts.plain_sides { Some(&sets.soft_side) }
                else { None };
            let mut plain = visible;
            if let Some(sp) = special_set {
                let cols = sp.cols(face_dir);
                let mut special = [0u32; NN];
                for i in 0..NN { special[i] = visible[i] & cols[i]; plain[i] = visible[i] & !cols[i]; }
                for_each_cell(&special, face_dir, |x, y, z| {
                    stats.special_faces += 1;
                    let b = chunk.blocks[Chunk::block_index(x as i32, y as i32, z as i32)];
                    if is_top {
                        let (wx, wy, wz) = (origin[0] + x as i32, origin[1] + y as i32, origin[2] + z as i32);
                        let weathering = mesher::weathering_at_world(b, wx, wy, wz);
                        // Greedy passes (slice=y, u=x, v=z) for the TOP config.
                        if weathering > 0.05 {
                            mesher::emit_subdivided_top(&mut mesh, y as i32, x as i32, z as i32, b, origin, sky, chunk, world_model, weathering);
                        } else {
                            mesher::emit_jittered_top(&mut mesh, y as i32, x as i32, z as i32, b, origin, sky, chunk, world_model);
                        }
                    } else {
                        // Side configs: slice on the face axis; (u, v) in the
                        // mesher's convention (±X: u=z, v=y; ±Z: u=y, v=x).
                        let (slice, u, v) = match face_dir {
                            Face::PosX | Face::NegX => (x, z, y),
                            _ => (z, y, x),
                        };
                        mesher::emit_soft_side_face(&mut mesh, cfg, slice as i32, u as i32, v as i32, b, origin, sky, chunk, world_model);
                    }
                });
            }

            // 3. Greedy-merge the plain faces. Key = block id, unless the
            //    material refuses to merge (then unique per cell).
            let approx_wy_for = |y: usize| -> i32 {
                if is_top || face_dir == Face::NegY { origin[1] + y as i32 } else { origin[1] + (N as i32 / 2) }
            };
            quads.clear();
            greedy_mask(&plain, face_dir, |x, y, z| {
                let b = chunk.blocks[Chunk::block_index(x as i32, y as i32, z as i32)];
                if crate::world::material_response::block_can_merge(b, approx_wy_for(y)) {
                    b as u32
                } else {
                    0x1_0000 | ((x as u32) << 10 | (y as u32) << 5 | z as u32) << 8 | b as u32
                }
            }, &mut quads);
            for q in &quads {
                // Block at the quad's origin cell, in bitchunk's (slice,u,v).
                let (x, y, z) = match face_dir {
                    Face::PosX | Face::NegX => (q.slice as usize, q.u as usize, q.v as usize),
                    Face::PosY | Face::NegY => (q.u as usize, q.slice as usize, q.v as usize),
                    Face::PosZ | Face::NegZ => (q.u as usize, q.v as usize, q.slice as usize),
                };
                let b = chunk.blocks[Chunk::block_index(x as i32, y as i32, z as i32)];
                let (u, v, w, h) = if swap {
                    (q.v as usize, q.u as usize, q.h as usize, q.w as usize)
                } else {
                    (q.u as usize, q.v as usize, q.w as usize, q.h as usize)
                };
                let lit = if opts.legacy_per_quad_light { None } else { Some(lights[cfg.face]) };
                mesher::emit_quad_lit(&mut mesh, cfg, q.slice as i32, u, v, w, h, b, origin, sky, Some(chunk), world_model, lit);
                stats.plain_quads += 1;
            }
        }
        stats.face_vertices = mesh.vertices.len() as u32;
        let tuft = opts.tuft_style.unwrap_or_else(mesher::TuftStyle::from_env);
        mesher::decorate_chunk_styled(chunk, &mut mesh, origin, sky, current_tick, tuft);
        mesher::emit_water_surface(chunk, &mut mesh, origin, sky);
        mesher::emit_lava_surface(chunk, &mut mesh, origin, sky);
        stats.decoration_vertices = mesh.vertices.len() as u32 - stats.face_vertices;
    }
    (mesh, stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::chunk::ChunkCoord;
    use std::collections::HashMap;

    fn chunk_from(coord: ChunkCoord, f: impl Fn(i32, i32, i32) -> BlockId) -> Chunk {
        let n = N as i32;
        let mut blocks = vec![BlockId::Air; N * N * N];
        for y in 0..n { for z in 0..n { for x in 0..n {
            blocks[Chunk::block_index(x, y, z)] = f(x, y, z);
        }}}
        Chunk::from_blocks(coord, blocks)
    }

    /// Coverage of a mesh: for each axis-aligned quad, which face
    /// direction and which unit cells it covers. Only valid for meshes
    /// made of plain quads (no jitter/soft/decoration).
    fn coverage(mesh: &ChunkMesh) -> HashMap<(usize, bool), Vec<u32>> {
        let mut cov: HashMap<(usize, bool), Vec<u32>> = HashMap::new();
        assert_eq!(mesh.indices.len() % 6, 0);
        for q in mesh.indices.chunks(6) {
            let p: Vec<[f32; 3]> = q.iter().map(|&i| mesh.vertices[i as usize].position).collect();
            // Constant axis.
            let axis = (0..3).find(|&a| p.iter().all(|v| (v[a] - p[0][a]).abs() < 1e-4))
                .expect("axis-aligned quad");
            // Normal sign from the first triangle's winding.
            let e1 = [p[1][0]-p[0][0], p[1][1]-p[0][1], p[1][2]-p[0][2]];
            let e2 = [p[2][0]-p[0][0], p[2][1]-p[0][1], p[2][2]-p[0][2]];
            let nrm = [e1[1]*e2[2]-e1[2]*e2[1], e1[2]*e2[0]-e1[0]*e2[2], e1[0]*e2[1]-e1[1]*e2[0]];
            let positive = nrm[axis] > 0.0;
            let (a1, a2) = match axis { 0 => (1, 2), 1 => (0, 2), _ => (0, 1) };
            let min1 = p.iter().map(|v| v[a1]).fold(f32::MAX, f32::min).round() as i32;
            let max1 = p.iter().map(|v| v[a1]).fold(f32::MIN, f32::max).round() as i32;
            let min2 = p.iter().map(|v| v[a2]).fold(f32::MAX, f32::min).round() as i32;
            let max2 = p.iter().map(|v| v[a2]).fold(f32::MIN, f32::max).round() as i32;
            let slice = p[0][axis].round() as i32;
            let e = cov.entry((axis, positive)).or_insert_with(|| vec![0u32; 64 * 64 * 64]);
            for i in min1..max1 { for j in min2..max2 {
                // Cells are in world space here; chunk at origin so 0..32
                // plus one chunk of slack for the slice coordinate.
                let key = ((slice + 16) as usize * 64 + (i + 16) as usize) * 64 + (j + 16) as usize;
                assert_eq!(e[key], 0, "double coverage at axis {} pos {} slice {} ({}, {})", axis, positive, slice, i, j);
                e[key] = 1;
            }}
        }
        cov
    }

    fn assert_same_coverage(a: &ChunkMesh, b: &ChunkMesh) {
        let ca = coverage(a);
        let cb = coverage(b);
        let keys: std::collections::BTreeSet<_> = ca.keys().chain(cb.keys()).cloned().collect();
        for k in keys {
            let empty = vec![0u32; 64 * 64 * 64];
            let x = ca.get(&k).unwrap_or(&empty);
            let y = cb.get(&k).unwrap_or(&empty);
            let nx: u32 = x.iter().sum();
            let ny: u32 = y.iter().sum();
            assert_eq!(nx, ny, "face count differs for axis {} positive {}", k.0, k.1);
            assert!(x == y, "coverage differs for axis {} positive {}", k.0, k.1);
        }
    }

    fn plain(b: BlockId) -> bool { !b.wants_top_jitter() && b.softness() == 0.0 }

    #[test]
    fn stone_is_plain_and_glass_is_transparent_here() {
        assert!(plain(BlockId::Stone));
        assert!(!BlockId::Stone.is_transparent());
        assert!(BlockId::Ice.is_transparent());
        assert!(mesher::is_meshable(BlockId::Ice));
    }

    #[test]
    fn coverage_matches_greedy_on_a_stone_heightfield() {
        let c = chunk_from(ChunkCoord::new(0, 0, 0), |x, y, z| {
            let h = 6 + ((x * 3 + z * 5) % 13);
            if y < h { BlockId::Stone } else { BlockId::Air }
        });
        let g = mesher::mesh_chunk_greedy_with_neighbors(&c, &NeighborChunks::none(), None, None, None);
        let (b, st) = mesh_chunk_binary_with_stats(&c, &NeighborChunks::none(), None, None, None);
        assert!(st.faces > 0);
        assert_eq!(st.special_faces, 0);
        assert_same_coverage(&g, &b);
    }

    #[test]
    fn coverage_matches_greedy_with_ice_and_water_and_neighbours() {
        let f = |x: i32, y: i32, z: i32| -> BlockId {
            let h = 10 + ((x + z) % 7);
            if y < h - 3 { BlockId::Stone }
            else if y < h { if (x / 4 + z / 4) % 3 == 0 { BlockId::Ice } else { BlockId::Stone } }
            else if y < 12 && x > 20 { BlockId::Water }
            else { BlockId::Air }
        };
        let c = chunk_from(ChunkCoord::new(0, 0, 0), f);
        // Neighbours: a solid slab to +X, ice to -Z, nothing elsewhere.
        let px = chunk_from(ChunkCoord::new(1, 0, 0), |_, y, _| if y < 14 { BlockId::Stone } else { BlockId::Air });
        let nz = chunk_from(ChunkCoord::new(0, 0, -1), |_, y, _| if y < 11 { BlockId::Ice } else { BlockId::Air });
        let nb = NeighborChunks {
            pos_x: Some(std::sync::Arc::new(px)),
            neg_z: Some(std::sync::Arc::new(nz)),
            ..NeighborChunks::none()
        };
        let g = mesher::mesh_chunk_greedy_with_neighbors(&c, &nb, None, None, None);
        let (b, st) = mesh_chunk_binary_with_stats(&c, &nb, None, None, None);
        assert_eq!(st.special_faces, 0);
        assert_same_coverage(&g, &b);
    }

    #[test]
    fn special_cells_are_routed_the_way_the_greedy_routes_them() {
        // Grass tops + dirt sides: every exposed grass top and dirt/grass
        // side is a special emit; count them independently.
        let f = |x: i32, y: i32, z: i32| -> BlockId {
            let h = 8 + ((x * 2 + z) % 5);
            if y + 1 == h { BlockId::Grass } else if y < h { BlockId::Dirt } else { BlockId::Air }
        };
        let c = chunk_from(ChunkCoord::new(0, 0, 0), f);
        let mut expect_special = 0u32;
        let mut expect_faces = 0u32;
        let n = N as i32;
        let at = |x: i32, y: i32, z: i32| if (0..n).contains(&x) && (0..n).contains(&y) && (0..n).contains(&z) { f(x, y, z) } else { BlockId::Air };
        for y in 0..n { for z in 0..n { for x in 0..n {
            let b = at(x, y, z);
            if !mesher::is_meshable(b) { continue; }
            for (dx, dy, dz) in [(1, 0, 0), (-1, 0, 0), (0, 1, 0), (0, -1, 0), (0, 0, 1), (0, 0, -1)] {
                if !mesher::should_emit_face(b, at(x + dx, y + dy, z + dz)) { continue; }
                expect_faces += 1;
                let top = dy == 1;
                let side = dy == 0;
                if (top && b.wants_top_jitter()) || (side && b.softness() > 0.0) { expect_special += 1; }
            }
        }}}
        let (b, st) = mesh_chunk_binary_with_stats(&c, &NeighborChunks::none(), None, None, None);
        assert_eq!(st.faces, expect_faces);
        assert_eq!(st.special_faces, expect_special);
        assert!(expect_special > 0);
        assert!(!b.vertices.is_empty());
        // And the greedy path on the same chunk emits the same number of
        // special quads' worth of vertices as ours only if the plain part
        // is equal — we can't separate them there, so just assert both are
        // non-trivial and the binary one is not larger than greedy + slack
        // (v-then-u vs u-then-v can add a few rectangles, never many).
        let g = mesher::mesh_chunk_greedy_with_neighbors(&c, &NeighborChunks::none(), None, None, None);
        assert!(b.vertices.len() <= g.vertices.len() + 64,
            "binary {} vs greedy {} vertices", b.vertices.len(), g.vertices.len());
    }

    /// `cargo test ... -- --ignored --nocapture mesher_binary::tests::timing_report`
    /// Prints µs/chunk for both meshers on three chunk shapes. Not an
    /// assertion: the ratio is the number to read.
    #[test]
    #[ignore]
    fn timing_report() {
        use std::time::Instant;
        let shapes: Vec<(&str, Box<dyn Fn(i32, i32, i32) -> BlockId>)> = vec![
            ("stone heightfield (plain faces only)", Box::new(|x, y, z| {
                let h = 6 + ((x * 3 + z * 5) % 13); if y < h { BlockId::Stone } else { BlockId::Air } })),
            ("grass/dirt/stone surface (specials)", Box::new(|x, y, z| {
                let h = 10 + ((x * 2 + z * 3) % 7);
                if y + 1 == h { BlockId::Grass } else if y + 4 > h && y < h { BlockId::Dirt } else if y < h { BlockId::Stone } else { BlockId::Air } })),
            ("underground with caves (all stone, holes)", Box::new(|x, y, z| {
                if ((x * 7 + y * 13 + z * 5) % 11) < 3 { BlockId::Air } else { BlockId::Stone } })),
        ];
        let iters = 20;
        for (name, f) in &shapes {
            let c = chunk_from(ChunkCoord::new(0, 0, 0), |x, y, z| f(x, y, z));
            let nb = NeighborChunks::none();
            let t = Instant::now();
            let mut gv = 0;
            for _ in 0..iters { gv = mesher::mesh_chunk_greedy_with_neighbors(&c, &nb, None, None, None).vertices.len(); }
            let g_us = t.elapsed().as_secs_f64() * 1e6 / iters as f64;
            let t = Instant::now();
            let mut bv = 0; let mut st = BinaryMeshStats::default();
            for _ in 0..iters { let (m, s) = mesh_chunk_binary_with_stats(&c, &nb, None, None, None); bv = m.vertices.len(); st = s; }
            let b_us = t.elapsed().as_secs_f64() * 1e6 / iters as f64;
            let plain = BinaryOpts { plain_tops: true, plain_sides: true, tuft_style: None, legacy_per_quad_light: false };
            let t = Instant::now();
            let mut pv = 0; let mut pst = BinaryMeshStats::default();
            for _ in 0..iters { let (m, s) = mesh_chunk_binary_opts(&c, &nb, None, None, None, plain); pv = m.vertices.len(); pst = s; }
            let p_us = t.elapsed().as_secs_f64() * 1e6 / iters as f64;
            let lean = BinaryOpts { plain_tops: true, plain_sides: true, tuft_style: Some(mesher::TuftStyle::Cross), legacy_per_quad_light: false };
            let t = Instant::now();
            let mut lv = 0; let mut lst = BinaryMeshStats::default();
            for _ in 0..iters { let (m, s) = mesh_chunk_binary_opts(&c, &nb, None, None, None, lean); lv = m.vertices.len(); lst = s; }
            let l_us = t.elapsed().as_secs_f64() * 1e6 / iters as f64;
            println!("{:42} greedy {:8.0} us ({} verts)   binary {:8.0} us ({} verts: {} faces + {} decoration)   faces={} plain_quads={} special={}   ratio {:.1}x   | plain-specials {:8.0} us ({} verts: {} faces + {} decoration, {} quads) ratio {:.1}x   | +cross tufts {:8.0} us ({} verts: {} faces + {} decoration) ratio {:.1}x",
                name, g_us, gv, b_us, bv, st.face_vertices, st.decoration_vertices, st.faces, st.plain_quads, st.special_faces, g_us / b_us,
                p_us, pv, pst.face_vertices, pst.decoration_vertices, pst.plain_quads, g_us / p_us,
                l_us, lv, lst.face_vertices, lst.decoration_vertices, g_us / l_us);
        }
    }

    #[test]
    fn plain_specials_cover_every_face_with_four_vertex_quads() {
        let f = |x: i32, y: i32, z: i32| -> BlockId {
            let h = 8 + ((x * 2 + z) % 5);
            if y + 1 == h { BlockId::Grass } else if y < h { BlockId::Dirt } else { BlockId::Air }
        };
        let c = chunk_from(ChunkCoord::new(0, 0, 0), f);
        let plain = BinaryOpts { plain_tops: true, plain_sides: true, tuft_style: None, legacy_per_quad_light: false };
        let (m, st) = mesh_chunk_binary_opts(&c, &NeighborChunks::none(), None, None, None, plain);
        assert_eq!(st.special_faces, 0);
        // Every cube face is now a plain 4-vertex quad; everything else in
        // the mesh is decoration (grass tufts), which the stats separate.
        assert_eq!(st.face_vertices, st.plain_quads * 4);
        assert_eq!(m.vertices.len() as u32, st.face_vertices + st.decoration_vertices);
        // The tufts are there. (They used to OUTWEIGH the terrain; since leaf and
        // tuft templates merge coplanar same-colour faces -- same look, fewer
        // vertices -- they no longer do: 27,616 tuft vs 29,160 terrain vertices
        // here. What this test checks is the plain-quad accounting above.)
        assert!(st.decoration_vertices > 0, "the grass surface carries its tufts");
    }

    #[test]
    fn cross_tufts_are_eight_vertices_and_double_sided() {
        let f = |_x: i32, y: i32, _z: i32| -> BlockId { if y < 5 { BlockId::Grass } else { BlockId::Air } };
        let c = chunk_from(ChunkCoord::new(0, 0, 0), f);
        let base = BinaryOpts { plain_tops: true, plain_sides: true, tuft_style: Some(mesher::TuftStyle::None), legacy_per_quad_light: false };
        let (m0, s0) = mesh_chunk_binary_opts(&c, &NeighborChunks::none(), None, None, None, base);
        assert_eq!(s0.decoration_vertices, 0);
        let cross = BinaryOpts { tuft_style: Some(mesher::TuftStyle::Cross), ..base };
        let (m1, s1) = mesh_chunk_binary_opts(&c, &NeighborChunks::none(), None, None, None, cross);
        assert!(s1.decoration_vertices > 0);
        assert_eq!(s1.decoration_vertices % 8, 0, "8 vertices per tuft");
        let tufts = s1.decoration_vertices / 8;
        assert_eq!(m1.indices.len() - m0.indices.len(), tufts as usize * 24, "24 indices per tuft (both windings)");
        let grid = BinaryOpts { tuft_style: Some(mesher::TuftStyle::Grid), ..base };
        let (_, s2) = mesh_chunk_binary_opts(&c, &NeighborChunks::none(), None, None, None, grid);
        assert!(s2.decoration_vertices > s1.decoration_vertices * 10,
            "grid tufts {} should dwarf cross tufts {}", s2.decoration_vertices, s1.decoration_vertices);
        // Tuft vertices sit within their block column and above the surface.
        for v in &m1.vertices[s1.face_vertices as usize..] {
            assert!(v.position[1] >= 5.0 - 0.51 && v.position[1] <= 6.0 + 0.51);
        }
    }

    /// In-process A/B of the two tint-cache shapes. Across-run timing on
    /// this hardware has a ±25 % band; interleaving them in one process
    /// with the same data measures the change itself.
    #[test]
    #[ignore]
    fn cache_shape_ab() {
        use std::time::Instant;
        let c = chunk_from(ChunkCoord::new(0, 0, 0), |x, y, z| {
            if ((x * 7 + y * 13 + z * 5) % 11) < 3 { BlockId::Air } else { BlockId::Stone } });
        // The cave chunk's ~36 K single-cell quads each do one cached
        // tint lookup; replay that access pattern directly.
        let cells: Vec<(i32, i32, i32)> = (0..32).flat_map(|y| (0..32).flat_map(move |z| (0..32).map(move |x| (x, y, z)))).collect();
        let rounds = 20;
        let (mut t_new, mut t_old) = (0.0f64, 0.0f64);
        let mut sink = 0.0f32;
        for r in 0..rounds {
            // Alternate order so drift and turbo effects hit both.
            if r % 2 == 0 {
                mesher::tint_cache_reset();
                let t = Instant::now();
                for &(x, y, z) in &cells { sink += mesher::tint_for_position_cached(BlockId::Stone, x, y, z, &c)[0]; }
                t_new += t.elapsed().as_secs_f64();
                mesher::tint_cache_reset();
                let t = Instant::now();
                for &(x, y, z) in &cells { sink += mesher::tint_for_position_cached_legacy(BlockId::Stone, x, y, z, &c)[0]; }
                t_old += t.elapsed().as_secs_f64();
            } else {
                mesher::tint_cache_reset();
                let t = Instant::now();
                for &(x, y, z) in &cells { sink += mesher::tint_for_position_cached_legacy(BlockId::Stone, x, y, z, &c)[0]; }
                t_old += t.elapsed().as_secs_f64();
                mesher::tint_cache_reset();
                let t = Instant::now();
                for &(x, y, z) in &cells { sink += mesher::tint_for_position_cached(BlockId::Stone, x, y, z, &c)[0]; }
                t_new += t.elapsed().as_secs_f64();
            }
        }
        // Every cell is a MISS here (one lookup each), so this measures the
        // miss path. Now the hit path: same cells twice, second pass hits.
        let (mut h_new, mut h_old) = (0.0f64, 0.0f64);
        for _ in 0..rounds {
            mesher::tint_cache_reset();
            for &(x, y, z) in &cells { sink += mesher::tint_for_position_cached(BlockId::Stone, x, y, z, &c)[0]; }
            let t = Instant::now();
            for &(x, y, z) in &cells { sink += mesher::tint_for_position_cached(BlockId::Stone, x, y, z, &c)[0]; }
            h_new += t.elapsed().as_secs_f64();
            mesher::tint_cache_reset();
            for &(x, y, z) in &cells { sink += mesher::tint_for_position_cached_legacy(BlockId::Stone, x, y, z, &c)[0]; }
            let t = Instant::now();
            for &(x, y, z) in &cells { sink += mesher::tint_for_position_cached_legacy(BlockId::Stone, x, y, z, &c)[0]; }
            h_old += t.elapsed().as_secs_f64();
        }
        let n = (cells.len() * rounds) as f64;
        println!("tint cache A/B over {} lookups each:\n  miss path: old {:6.1} ns  new {:6.1} ns  ({:+.1}%)\n  hit  path: old {:6.1} ns  new {:6.1} ns  ({:+.1}%)   [sink {}]",
            n, t_old * 1e9 / n, t_new * 1e9 / n, (t_new / t_old - 1.0) * 100.0,
            h_old * 1e9 / n, h_new * 1e9 / n, (h_new / h_old - 1.0) * 100.0, sink as i64);
    }

    #[test]
    fn cached_tint_matches_the_legacy_shape_exactly() {
        let c = chunk_from(ChunkCoord::new(-3, 1, 2), |_, y, _| if y < 20 { BlockId::Stone } else { BlockId::Air });
        for (x, y, z) in [(0, 0, 0), (31, 31, 31), (5, 12, 19), (17, 3, 28)] {
            mesher::tint_cache_reset();
            let a = mesher::tint_for_position_cached(BlockId::Stone, x, y, z, &c);
            mesher::tint_cache_reset();
            let b = mesher::tint_for_position_cached_legacy(BlockId::Stone, x, y, z, &c);
            assert_eq!(a, b);
            // And a second (hit) lookup returns the same value.
            assert_eq!(mesher::tint_for_position_cached(BlockId::Stone, x, y, z, &c), a);
        }
    }

    #[test]
    fn hoisted_light_is_output_identical() {
        let sky = crate::sky::Sky::default();
        let c = chunk_from(ChunkCoord::new(0, 0, 0), |x, y, z| {
            if ((x * 7 + y * 13 + z * 5) % 11) < 3 { BlockId::Air } else { BlockId::Stone } });
        let hoisted = BinaryOpts::default();
        let legacy = BinaryOpts { legacy_per_quad_light: true, ..BinaryOpts::default() };
        for sky_opt in [None, Some(&sky)] {
            let (a, _) = mesh_chunk_binary_opts(&c, &NeighborChunks::none(), sky_opt, None, None, hoisted);
            let (b, _) = mesh_chunk_binary_opts(&c, &NeighborChunks::none(), sky_opt, None, None, legacy);
            assert_eq!(a.vertices.len(), b.vertices.len());
            assert_eq!(a.indices, b.indices);
            for (va, vb) in a.vertices.iter().zip(b.vertices.iter()) {
                assert_eq!(va.position, vb.position);
                assert_eq!(va.color, vb.color, "hoisting the face light must not change a colour");
            }
        }
    }

    /// In-process A/B of hoisting the per-face brightness out of emit_quad.
    #[test]
    #[ignore]
    fn light_hoist_ab() {
        use std::time::Instant;
        let sky = crate::sky::Sky::default();
        let c = chunk_from(ChunkCoord::new(0, 0, 0), |x, y, z| {
            if ((x * 7 + y * 13 + z * 5) % 11) < 3 { BlockId::Air } else { BlockId::Stone } });
        let nb = NeighborChunks::none();
        let hoisted = BinaryOpts::default();
        let legacy = BinaryOpts { legacy_per_quad_light: true, ..BinaryOpts::default() };
        let rounds = 12;
        let (mut t_new, mut t_old) = (0.0f64, 0.0f64);
        let mut quads = 0u32;
        let run = |o: BinaryOpts| -> (f64, u32) {
            let t = Instant::now();
            let (_, s) = mesh_chunk_binary_opts(&c, &nb, Some(&sky), None, None, o);
            (t.elapsed().as_secs_f64(), s.plain_quads)
        };
        for r in 0..rounds {
            if r % 2 == 0 {
                let (a, q) = run(hoisted); t_new += a; quads = q;
                let (b, _) = run(legacy);  t_old += b;
            } else {
                let (b, _) = run(legacy);  t_old += b;
                let (a, q) = run(hoisted); t_new += a; quads = q;
            }
        }
        let per = |t: f64| t * 1e6 / rounds as f64;
        println!("face-light hoist A/B ({} plain quads/chunk, {} rounds):\n  per-quad light: {:8.0} us   hoisted: {:8.0} us   ({:+.1}%, {:.1} ns/quad saved)",
            quads, rounds, per(t_old), per(t_new),
            (t_new / t_old - 1.0) * 100.0,
            (t_old - t_new) * 1e9 / (rounds as f64 * quads as f64));
    }

    #[test]
    fn all_air_chunk_is_empty_and_fast_path_matches() {
        let c = chunk_from(ChunkCoord::new(2, 3, 4), |_, _, _| BlockId::Air);
        let g = mesher::mesh_chunk_greedy_with_neighbors(&c, &NeighborChunks::none(), None, None, None);
        let b = mesh_chunk_binary(&c, &NeighborChunks::none(), None, None, None);
        assert_eq!(g.vertices.len(), b.vertices.len());
    }
}
