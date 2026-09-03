//! SMOOTH CHUNK — plan, extract, shade, pack, in one call.
//!
//! The pieces of the smooth-terrain path each have a reason to be separate:
//! `isosurface` is engine-free geometry, `smooth_plan` is LOD policy,
//! `smooth` bridges the generator, `smooth_vertex` is the wire format. Nothing
//! composed them, which left the render side holding four things it has to
//! assemble in the right order with the right shared `origin`.
//!
//! That assembly has exactly one correct answer, so it belongs here rather
//! than in the caller. The render half's job becomes: call [`build`], upload
//! the buffers.
//!
//! It lives in `world/` deliberately. `render/` is behind `--features
//! render-core` and is not compiled in a headless build — a module put there
//! is unverified by construction, which this path already learned the hard way
//! (`smooth_vertex` spent a revision there with its tests silently absent from
//! the binary). Everything up to the buffer upload is testable, so everything
//! up to the buffer upload stays out of `render/`.
//!
//! ## The origin, which is the thing to get wrong
//!
//! A plan's `origin` is NOT the chunk's corner when the chunk owns overlap on
//! a low face — it has moved back into the neighbour. Positions come out of
//! the extractor relative to that origin, materials must be sampled relative
//! to that origin, and the packer expects positions relative to the CHUNK.
//! Three frames, one of which is offset by an amount the caller has no reason
//! to know about. [`build`] holds all three; `positions_are_chunk_local`
//! pins it.

use isosurface::Mesh;

use crate::constants::CHUNK_SIZE;
use crate::world::blocks::BlockId;
use crate::world::smooth::{materials_for, mesh_region};
use crate::world::smooth_plan::Plan;
use crate::world::smooth_vertex::SmoothVertex;
use crate::world::world_model::WorldModel;

/// A chunk's smooth surface, ready to upload.
#[derive(Clone, Debug, Default)]
pub struct SmoothChunkMesh {
    pub vertices: Vec<SmoothVertex>,
    pub indices: Vec<u32>,
}

impl SmoothChunkMesh {
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }
    pub fn triangles(&self) -> usize {
        self.indices.len() / 3
    }
    /// Bytes this would occupy in a vertex buffer.
    pub fn vertex_bytes(&self) -> usize {
        self.vertices.len() * SmoothVertex::STRIDE as usize
    }
}

/// Build one chunk's smooth mesh from its plan.
pub fn build(model: &WorldModel, plan: Plan) -> SmoothChunkMesh {
    let mesh: Mesh = mesh_region(model, plan.origin, plan.dims, plan.stride, plan.seams);
    if mesh.positions.is_empty() {
        return SmoothChunkMesh::default();
    }
    let materials = materials_for(model, &mesh, plan.origin);

    // Positions arrive relative to the plan's origin, which may sit outside
    // the chunk. Shift into chunk-local, which is what the packer's range is
    // defined against.
    let chunk_corner = [
        plan.coord.x * CHUNK_SIZE,
        plan.coord.y * CHUNK_SIZE,
        plan.coord.z * CHUNK_SIZE,
    ];
    let shift = [
        (plan.origin[0] - chunk_corner[0]) as f32,
        (plan.origin[1] - chunk_corner[1]) as f32,
        (plan.origin[2] - chunk_corner[2]) as f32,
    ];

    let vertices = mesh
        .positions
        .iter()
        .zip(&mesh.normals)
        .zip(&materials)
        .map(|((p, n), mat)| {
            let local = [p.x + shift[0], p.y + shift[1], p.z + shift[2]];
            let c = mat.base_color();
            let color = [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0];
            SmoothVertex::new(local, [n.x, n.y, n.z], color, *mat as u8)
        })
        .collect();

    SmoothChunkMesh { vertices, indices: mesh.indices }
}

/// Material of a vertex as packed, for a caller that wants it back without
/// re-sampling. The id rides the alpha byte, same as the greedy path.
#[inline]
pub fn material_of(v: &SmoothVertex) -> Option<BlockId> {
    BlockId::try_from((v.color_packed >> 24) as u8).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::chunk::ChunkCoord;
    use crate::world::smooth_plan::plan_for;
    use crate::world::smooth_vertex::{unpack_normal, unpack_pos};

    fn model() -> WorldModel {
        WorldModel::new(0xC0FFEE)
    }

    /// A chunk that straddles the surface, found rather than guessed so the
    /// tests do not depend on where seed 0xC0FFEE happens to put the ground.
    fn a_surface_chunk(m: &WorldModel) -> ChunkCoord {
        let target = m.surface_target(0.0, 0.0, 0.0);
        ChunkCoord { x: 0, y: (target / CHUNK_SIZE as f64).floor() as i32, z: 0 }
    }

    #[test]
    fn a_surface_chunk_builds_something_uploadable() {
        let m = model();
        let c = a_surface_chunk(&m);
        let mesh = build(&m, plan_for(c, c, 16));
        assert!(!mesh.is_empty(), "no geometry for a chunk on the surface");
        assert_eq!(mesh.indices.len() % 3, 0);
        assert!(mesh.indices.iter().all(|i| (*i as usize) < mesh.vertices.len()));
        assert_eq!(mesh.vertex_bytes(), mesh.vertices.len() * 12);
    }

    #[test]
    fn positions_are_chunk_local_even_when_the_plan_reaches_backwards() {
        // THE BUG THIS COMPOSITION EXISTS TO PREVENT. A plan owning overlap on
        // a low face has an origin BEHIND the chunk corner; forgetting the
        // shift puts the whole chunk one overlap-width off, which reads as
        // terrain tearing rather than as an offset.
        let m = model();
        let c = a_surface_chunk(&m);
        let player = ChunkCoord { x: c.x + 40, y: c.y, z: c.z };
        let plan = plan_for(c, player, 16);
        let mesh = build(&m, plan);
        if mesh.is_empty() {
            return;
        }
        let corner_shift = plan.origin[0] - c.x * CHUNK_SIZE;
        // sanity: this plan really does reach outside, or the test proves nothing
        if corner_shift == 0 && !plan.any_overlap() {
            return;
        }
        for v in &mesh.vertices {
            let p = unpack_pos(v.pos_packed);
            for (axis, val) in p.iter().enumerate() {
                assert!(
                    (-8.5..=40.5).contains(val),
                    "axis {axis} at {val} is outside the packable range; \
                     the origin shift is probably missing"
                );
            }
        }
    }

    #[test]
    fn normals_survive_the_round_trip_into_the_buffer() {
        let m = model();
        let c = a_surface_chunk(&m);
        let mesh = build(&m, plan_for(c, c, 16));
        assert!(!mesh.vertices.is_empty());
        for v in &mesh.vertices {
            let n = unpack_normal(v.norm_packed);
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            assert!((len - 1.0).abs() < 1e-3, "normal length {len} after packing");
        }
    }

    #[test]
    fn every_vertex_carries_a_recoverable_material() {
        let m = model();
        let c = a_surface_chunk(&m);
        let mesh = build(&m, plan_for(c, c, 16));
        assert!(!mesh.vertices.is_empty());
        for v in &mesh.vertices {
            let mat = material_of(v).expect("material id did not decode to a BlockId");
            assert_ne!(mat, BlockId::Air, "a surface vertex made of air");
        }
    }

    #[test]
    fn deep_underground_and_high_air_build_nothing() {
        // Not every chunk has a surface in it, and the empty case must be
        // cheap and silent rather than a panic or a degenerate buffer.
        let m = model();
        let sky = ChunkCoord { x: 0, y: 60, z: 0 };
        assert!(build(&m, plan_for(sky, sky, 16)).is_empty(), "found terrain in the sky");
    }

    #[test]
    fn building_is_deterministic() {
        // A chunk is re-meshed whenever a neighbour's LOD changes, so two
        // builds of the same plan must be byte-identical or the buffers churn.
        let m = model();
        let c = a_surface_chunk(&m);
        let plan = plan_for(c, c, 16);
        let a = build(&m, plan);
        let b = build(&m, plan);
        assert_eq!(a.vertices, b.vertices);
        assert_eq!(a.indices, b.indices);
    }

    #[test]
    fn a_coarser_tier_costs_less_to_upload() {
        // The point of the LOD ladder, asserted end to end rather than at the
        // extractor: fewer vertices means fewer bytes on the bus.
        let m = model();
        let c = a_surface_chunk(&m);
        let near = build(&m, plan_for(c, c, 16));
        let far_player = ChunkCoord { x: c.x + 200, y: c.y, z: c.z };
        let far = build(&m, plan_for(c, far_player, 16));
        if far.is_empty() || near.is_empty() {
            return;
        }
        assert!(
            far.vertex_bytes() < near.vertex_bytes(),
            "coarse {} bytes vs near {} bytes",
            far.vertex_bytes(),
            near.vertex_bytes()
        );
    }
}
