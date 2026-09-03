//! SMOOTH MESH PLANNING — who meshes what, and who owns the seam.
//!
//! `smooth.rs` can mesh any region; `lod.rs` decides how coarse each chunk
//! should be. This is the function between them, and it exists because of one
//! rule that has to be decided in exactly one place:
//!
//! **At an LOD boundary, the FINER chunk owns the overlap.**
//!
//! `isosurface`'s seam work establishes that two chunks at different strides
//! agree once the finer one meshes one coarse footprint PAST the shared plane
//! — it then owns the cell its neighbour owns, computes the identical vertex
//! from the identical samples, and emits the quads bridging the boundary. That
//! only works if precisely one side does it. Both sides overlapping means
//! doubled coplanar geometry; neither means the hole is back.
//!
//! Finer-owns is the right assignment for two reasons beyond arbitrariness.
//! The fine chunk is the one that can afford the extra cells (its neighbour
//! being coarse is exactly the statement that detail is cheap here and dear
//! there), and the fine chunk is the one already sampling at a stride that
//! divides the coarse one, so borrowing the coarse grid is a whole-number
//! step rather than a resample.
//!
//! ## What this does not do
//!
//! It does not decide WHEN to re-mesh. A chunk's plan depends on its
//! neighbours' LODs, so a neighbour coarsening invalidates this chunk's mesh
//! even though nothing about this chunk changed — a dependency the block
//! mesher never had, since `cull_boundary_face` handled seams locally. That
//! invalidation belongs to whatever owns the mesh cache. [`Plan::depends_on`]
//! gives it the list.

use isosurface::Seams;

use crate::constants::CHUNK_SIZE;
use crate::world::chunk::ChunkCoord;
use crate::world::lod::{desired_lod_for, lod_cell_stride};

/// Face order matches `isosurface::Seams`: `[-x, +x, -y, +y, -z, +z]`.
pub const FACE_DIRS: [[i32; 3]; 6] =
    [[-1, 0, 0], [1, 0, 0], [0, -1, 0], [0, 1, 0], [0, 0, -1], [0, 0, 1]];

/// Everything the extractor needs for one chunk, and nothing else.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Plan {
    pub coord: ChunkCoord,
    /// Sample stride, from this chunk's LOD tier.
    pub stride: i32,
    /// First block sampled, in world blocks. Moves NEGATIVE on a face where
    /// this chunk owns the overlap into a lower neighbour.
    pub origin: [i32; 3],
    /// Lattice samples per axis, at `stride`.
    pub dims: [i32; 3],
    /// Neighbour strides, for the boundary cells that borrow a coarse grid.
    pub seams: Seams,
    /// Faces where this chunk meshes past the boundary. Informational — the
    /// effect is already in `origin` and `dims` — but a renderer wants to know
    /// which of its geometry is another chunk's ground.
    pub overlaps: [bool; 6],
}

impl Plan {
    /// The chunks whose LOD this plan depends on. If any of their tiers
    /// change, this chunk's mesh is stale even though its blocks are not.
    pub fn depends_on(&self) -> [ChunkCoord; 6] {
        let c = self.coord;
        FACE_DIRS.map(|d| ChunkCoord { x: c.x + d[0], y: c.y + d[1], z: c.z + d[2] })
    }
    /// Does this chunk mesh past any boundary.
    pub fn any_overlap(&self) -> bool {
        self.overlaps.iter().any(|b| *b)
    }
}

fn stride_of(chunk: ChunkCoord, player: ChunkCoord, render_distance: i32) -> i32 {
    lod_cell_stride(desired_lod_for(chunk, player, render_distance)) as i32
}

/// Build the extraction plan for one chunk.
pub fn plan_for(chunk: ChunkCoord, player: ChunkCoord, render_distance: i32) -> Plan {
    let k = stride_of(chunk, player, render_distance);
    let base = [chunk.x * CHUNK_SIZE, chunk.y * CHUNK_SIZE, chunk.z * CHUNK_SIZE];

    // One extra sample per axis so the last cell is complete — the shared
    // lattice point with the next chunk.
    let span = CHUNK_SIZE / k;
    let mut dims = [span + 1, span + 1, span + 1];
    let mut origin = base;
    let mut seams = Seams::NONE;
    let mut overlaps = [false; 6];

    for (face, d) in FACE_DIRS.iter().enumerate() {
        let n = ChunkCoord { x: chunk.x + d[0], y: chunk.y + d[1], z: chunk.z + d[2] };
        let nk = stride_of(n, player, render_distance);
        seams.0[face] = nk;
        if nk <= k {
            continue;
        }
        // Coarser neighbour: we are the finer side, so we own the overlap and
        // reach one of ITS cells past the plane. `nk / k` of our cells.
        let extra = nk / k;
        let axis = face / 2;
        dims[axis] += extra;
        if face % 2 == 0 {
            // low face: start earlier
            origin[axis] -= extra * k;
        }
        overlaps[face] = true;
    }

    Plan { coord: chunk, stride: k, origin, dims, seams, overlaps }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(x: i32, y: i32, z: i32) -> ChunkCoord {
        ChunkCoord { x, y, z }
    }

    /// A chunk far enough out to be coarser than the player's own tier, found
    /// rather than assumed so the test survives a bands retune.
    fn a_boundary_pair(player: ChunkCoord, rd: i32) -> (ChunkCoord, ChunkCoord) {
        for x in 0..rd * 2 {
            let a = c(player.x + x, player.y, player.z);
            let b = c(player.x + x + 1, player.y, player.z);
            if stride_of(b, player, rd) > stride_of(a, player, rd) {
                return (a, b);
            }
        }
        panic!("no LOD boundary within {rd} chunks; bands may have changed");
    }

    #[test]
    fn a_chunk_with_equal_neighbours_meshes_exactly_its_own_ground() {
        let p = c(0, 0, 0);
        let plan = plan_for(p, p, 16);
        assert_eq!(plan.origin, [0, 0, 0]);
        let span = CHUNK_SIZE / plan.stride;
        assert_eq!(plan.dims, [span + 1; 3], "an interior chunk grew");
        assert!(!plan.any_overlap());
    }

    #[test]
    fn exactly_one_side_of_a_seam_owns_the_overlap() {
        // The rule this module exists for. Both sides overlapping doubles the
        // geometry; neither leaves the hole the seam work closed.
        let p = c(0, 0, 0);
        let rd = 16;
        let (fine, coarse) = a_boundary_pair(p, rd);
        let pf = plan_for(fine, p, rd);
        let pc = plan_for(coarse, p, rd);
        // fine's +x face and coarse's -x face are the same plane
        assert!(pf.overlaps[1], "the finer chunk did not take the overlap");
        assert!(!pc.overlaps[0], "the coarser chunk also took it");
    }

    #[test]
    fn the_overlap_reaches_exactly_one_of_the_neighbours_cells() {
        let p = c(0, 0, 0);
        let rd = 16;
        let (fine, coarse) = a_boundary_pair(p, rd);
        let pf = plan_for(fine, p, rd);
        let ratio = stride_of(coarse, p, rd) / pf.stride;
        let span = CHUNK_SIZE / pf.stride;
        assert_eq!(pf.dims[0], span + 1 + ratio, "overlap is not one coarse cell");
        // reaching FORWARD must not move the origin
        assert_eq!(pf.origin[0], fine.x * CHUNK_SIZE);
    }

    #[test]
    fn an_overlap_on_a_low_face_moves_the_origin_back_instead() {
        // Same amount of extra ground, the other direction. Getting this wrong
        // meshes the wrong side and leaves the seam open while looking right.
        let p = c(0, 0, 0);
        let rd = 16;
        let (fine, _) = a_boundary_pair(p, rd);
        // mirror: a chunk whose -x neighbour is coarser
        let mirrored = c(p.x - (fine.x - p.x) - 1, p.y, p.z);
        let pm = plan_for(mirrored, p, rd);
        if pm.overlaps[0] {
            assert!(pm.origin[0] < mirrored.x * CHUNK_SIZE, "low-face overlap did not reach back");
            assert!(pm.dims[0] > CHUNK_SIZE / pm.stride + 1);
        }
    }

    #[test]
    fn the_seams_a_chunk_declares_are_its_neighbours_actual_strides() {
        let p = c(0, 0, 0);
        let rd = 16;
        let (fine, _) = a_boundary_pair(p, rd);
        let plan = plan_for(fine, p, rd);
        for (face, d) in FACE_DIRS.iter().enumerate() {
            let n = c(fine.x + d[0], fine.y + d[1], fine.z + d[2]);
            assert_eq!(
                plan.seams.0[face],
                stride_of(n, p, rd),
                "face {face} declared the wrong neighbour stride"
            );
        }
    }

    #[test]
    fn a_plan_names_the_neighbours_it_will_go_stale_with() {
        // The dependency the block mesher never had: cull_boundary_face
        // handled seams locally, so a chunk's mesh depended only on its own
        // blocks. Here a neighbour coarsening invalidates this mesh.
        let p = c(3, 0, -2);
        let plan = plan_for(p, c(0, 0, 0), 16);
        let deps = plan.depends_on();
        assert_eq!(deps.len(), 6);
        assert!(!deps.contains(&p), "a chunk depends on itself");
        for d in deps {
            assert_eq!((d.x - p.x).abs() + (d.y - p.y).abs() + (d.z - p.z).abs(), 1);
        }
    }

    #[test]
    fn every_plan_in_a_neighbourhood_is_sane() {
        // Sweep rather than spot-check: no zero dims, no stride below one, and
        // an overlap only ever where the neighbour really is coarser.
        let p = c(0, 0, 0);
        let rd = 12;
        for x in -rd..=rd {
            for z in -rd..=rd {
                let ch = c(x, 0, z);
                let plan = plan_for(ch, p, rd);
                assert!(plan.stride >= 1);
                assert!(plan.dims.iter().all(|d| *d >= 2), "{ch:?} -> {:?}", plan.dims);
                for (face, d) in FACE_DIRS.iter().enumerate() {
                    let n = c(ch.x + d[0], ch.y + d[1], ch.z + d[2]);
                    let coarser = stride_of(n, p, rd) > plan.stride;
                    assert_eq!(plan.overlaps[face], coarser, "{ch:?} face {face}");
                }
            }
        }
    }
}
