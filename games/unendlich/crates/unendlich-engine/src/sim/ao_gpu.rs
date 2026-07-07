//! Kernel: **voxel ambient-occlusion bake** (mesher-facing) for the
//! [`crate::sim::gpu_bench`] harness.
//!
//! Computes the canonical Minecraft-style *corner AO* for every exposed face of
//! every solid voxel: each face has four vertices, and each vertex darkens based
//! on the two edge-adjacent voxels and the one corner voxel sampled in the air
//! layer just outside the face. AO level `3` = unoccluded (bright), `0` = a
//! concave corner (darkest). The mesher multiplies vertex light by this.
//!
//! GPU-ready form for the harness: one logical thread per voxel face (or per
//! voxel emitting its six faces), each reading only a fixed local neighbourhood
//! of the occupancy grid and writing its four AO bytes — no host branching, no
//! cross-thread state. The CPU reference here is the bit-exact correctness gate
//! and the thing the benchmark times.
//!
//! ## GPU DISPATCH CONTRACT
//!   bindings:
//!     0: grid SSBO — [u32 solid; nx*ny*nz]  (1 = solid occluder, 0 = air)
//!     1: out  SSBO — [FaceAo; nx*ny*nz*6]   (face f of cell c at c*6 + f)
//!   uniforms: nx, ny, nz
//!   dispatch: one invocation per (cell, face); face index f = axis*2 + (sign>0).
//!     thread: if !solid[cell] → exposed=0. else sample the air-side neighbour
//!     along the face normal; if solid → exposed=0 (hidden); else exposed=1 and
//!     for each of the 4 vertices sample {side1, side2, corner} in the air layer
//!     and write `vertex_ao`. Out-of-bounds samples read as air. Integer-only ⇒
//!     GPU output equals [`ao_bake_cpu`] bit-for-bit.

use crate::sim::gpu_bench::{bench, BenchResult};

/// Unit axis vectors; index 0=x, 1=y, 2=z.
const UNIT: [[i32; 3]; 3] = [[1, 0, 0], [0, 1, 0], [0, 0, 1]];

/// The two axes perpendicular to normal axis `a`.
#[inline]
fn perp(a: usize) -> [usize; 2] {
    match a {
        0 => [1, 2],
        1 => [0, 2],
        _ => [0, 1],
    }
}

/// Per-vertex corner-AO: if both edge sides are solid the vertex is a concave
/// corner (level 0, darkest); otherwise brightness drops by each occluder.
/// Range 0..=3.
#[inline]
pub fn vertex_ao(side1: bool, side2: bool, corner: bool) -> u8 {
    if side1 && side2 {
        0
    } else {
        3 - (side1 as u8 + side2 as u8 + corner as u8)
    }
}

/// AO result for one face of one voxel. `ao` is the four vertex levels in the
/// fixed order `(sp1,sp2) = (-,-), (-,+), (+,-), (+,+)` where `sp1`/`sp2` index
/// the two axes perpendicular to the face normal. `exposed == false` means the
/// face is hidden (or the cell is air) and `ao` is meaningless (all zero).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FaceAo {
    pub exposed: bool,
    pub ao: [u8; 4],
}

/// A flat 3D voxel occupancy grid (row-major, x fastest) — the SoA buffer the
/// GPU kernel binds directly.
#[derive(Clone, Debug)]
pub struct VoxelGrid {
    pub nx: usize,
    pub ny: usize,
    pub nz: usize,
    /// `true` = solid occluder. Length `nx*ny*nz`.
    pub solid: Vec<bool>,
}

impl VoxelGrid {
    pub fn empty(nx: usize, ny: usize, nz: usize) -> Self {
        VoxelGrid { nx, ny, nz, solid: vec![false; nx * ny * nz] }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.nx * self.ny * self.nz
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[inline]
    pub fn idx(&self, x: usize, y: usize, z: usize) -> usize {
        (z * self.ny + y) * self.nx + x
    }

    /// Occupancy with out-of-bounds reading as air (no occluder at world edges).
    #[inline]
    pub fn occ(&self, x: i32, y: i32, z: i32) -> bool {
        if x < 0 || y < 0 || z < 0 {
            return false;
        }
        let (x, y, z) = (x as usize, y as usize, z as usize);
        if x >= self.nx || y >= self.ny || z >= self.nz {
            return false;
        }
        self.solid[self.idx(x, y, z)]
    }
}

/// Compute the AO of one face of voxel `(cx,cy,cz)`: normal axis `a` with sign
/// `s` (+1 / -1). Returns `(exposed, [u8;4])`. Caller guarantees the cell is
/// solid. A face is exposed iff the neighbour along the normal is air.
fn face_ao(grid: &VoxelGrid, cx: i32, cy: i32, cz: i32, a: usize, s: i32) -> (bool, [u8; 4]) {
    let n = UNIT[a];
    // air-side base = cell + one step along the normal
    let (bx, by, bz) = (cx + s * n[0], cy + s * n[1], cz + s * n[2]);
    if grid.occ(bx, by, bz) {
        return (false, [0; 4]); // neighbour solid → face hidden
    }
    let p = perp(a);
    let u = UNIT[p[0]];
    let v = UNIT[p[1]];
    let mut ao = [0u8; 4];
    let mut k = 0;
    for sp1 in [-1i32, 1] {
        for sp2 in [-1i32, 1] {
            let side1 = grid.occ(bx + sp1 * u[0], by + sp1 * u[1], bz + sp1 * u[2]);
            let side2 = grid.occ(bx + sp2 * v[0], by + sp2 * v[1], bz + sp2 * v[2]);
            let corner = grid.occ(
                bx + sp1 * u[0] + sp2 * v[0],
                by + sp1 * u[1] + sp2 * v[1],
                bz + sp1 * u[2] + sp2 * v[2],
            );
            ao[k] = vertex_ao(side1, side2, corner);
            k += 1;
        }
    }
    (true, ao)
}

/// Bake AO for every face of every voxel. Output length is `grid.len() * 6`;
/// face `f` of cell `c` lives at `c * 6 + f`, where `f = axis*2 + (sign>0)`
/// (so 0=-X, 1=+X, 2=-Y, 3=+Y, 4=-Z, 5=+Z). Faces of air cells and hidden faces
/// have `exposed == false`.
pub fn ao_bake_cpu(grid: &VoxelGrid) -> Vec<FaceAo> {
    let (nx, ny, nz) = (grid.nx, grid.ny, grid.nz);
    let mut out = vec![FaceAo::default(); grid.len() * 6];
    for z in 0..nz {
        for y in 0..ny {
            for x in 0..nx {
                let c = grid.idx(x, y, z);
                if !grid.solid[c] {
                    continue; // air cell → all six faces stay default (not exposed)
                }
                for f in 0..6usize {
                    let a = f / 2;
                    let s = if f % 2 == 1 { 1 } else { -1 };
                    let (exposed, ao) = face_ao(grid, x as i32, y as i32, z as i32, a, s);
                    out[c * 6 + f] = FaceAo { exposed, ao };
                }
            }
        }
    }
    out
}

/// Benchmark the AO bake over `iters` runs. Feed to
/// [`BenchResult::metric_line`] with `name = "voxel_ao"`.
pub fn bench_ao(grid: &VoxelGrid, iters: usize) -> BenchResult {
    bench(iters, || {
        let baked = ao_bake_cpu(grid);
        std::hint::black_box(&baked);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 5³ grid with the given solid cells.
    fn grid_with(solids: &[(usize, usize, usize)]) -> VoxelGrid {
        let mut g = VoxelGrid::empty(5, 5, 5);
        for &(x, y, z) in solids {
            let i = g.idx(x, y, z);
            g.solid[i] = true;
        }
        g
    }

    // face index helper: axis a, sign s -> f
    fn fi(a: usize, s: i32) -> usize {
        a * 2 + if s > 0 { 1 } else { 0 }
    }

    #[test]
    fn vertex_ao_formula() {
        assert_eq!(vertex_ao(true, true, false), 0); // concave corner
        assert_eq!(vertex_ao(true, true, true), 0);
        assert_eq!(vertex_ao(false, false, false), 3); // open
        assert_eq!(vertex_ao(true, false, false), 2);
        assert_eq!(vertex_ao(true, false, true), 1);
        assert_eq!(vertex_ao(false, false, true), 2);
    }

    #[test]
    fn lone_voxel_all_bright() {
        let g = grid_with(&[(2, 2, 2)]);
        let baked = ao_bake_cpu(&g);
        let c = g.idx(2, 2, 2);
        for f in 0..6 {
            let fa = baked[c * 6 + f];
            assert!(fa.exposed, "face {f} should be exposed");
            assert_eq!(fa.ao, [3, 3, 3, 3], "face {f}");
        }
    }

    #[test]
    fn diagonal_occluder_darkens_two_vertices() {
        // occluder diagonally above-beside (+X,+Y) darkens the +X-side verts of +Y face.
        let g = grid_with(&[(2, 2, 2), (3, 3, 2)]);
        let baked = ao_bake_cpu(&g);
        let c = g.idx(2, 2, 2);
        // +Y face: verts order (X-,Z-),(X-,Z+),(X+,Z-),(X+,Z+)
        assert_eq!(baked[c * 6 + fi(1, 1)].ao, [3, 3, 2, 2]);
    }

    #[test]
    fn concave_corner_is_zero() {
        let g = grid_with(&[(2, 2, 2), (3, 3, 2), (2, 3, 3)]);
        let baked = ao_bake_cpu(&g);
        let c = g.idx(2, 2, 2);
        // (X+,Z+) vertex of +Y face is the last of the four
        assert_eq!(baked[c * 6 + fi(1, 1)].ao[3], 0);
    }

    #[test]
    fn neighbour_hides_face() {
        let g = grid_with(&[(2, 2, 2), (3, 2, 2)]);
        let baked = ao_bake_cpu(&g);
        let c = g.idx(2, 2, 2);
        assert!(!baked[c * 6 + fi(0, 1)].exposed, "+X face hidden by neighbour");
        assert!(baked[c * 6 + fi(0, -1)].exposed, "-X face open");
    }

    #[test]
    fn output_length_and_bench() {
        let g = grid_with(&[(2, 2, 2)]);
        assert_eq!(ao_bake_cpu(&g).len(), g.len() * 6);
        let br = bench_ao(&g, 3);
        assert_eq!(br.samples.len(), 3);
        assert!(br.metric_line("voxel_ao", Some(1.0)).contains("voxel_ao n=3"));
    }
}
