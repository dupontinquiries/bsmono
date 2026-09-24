// Microvoxel engine core.
//
// Models are authored as dense voxel grids, then "meshed" into per-voxel
// instance data (offset + baked ambient occlusion + RGBA). The GPU renders
// each voxel as a tiny instanced cube — the classic microvoxel look — with
// face-normal lambert lighting, baked AO, shadows and fog.

use glam::Vec3;

/// Unit cube: 6 faces × 4 verts, positions then per-face normal.
/// Layout: [pos;3] then [normal;3] interleaved per vertex.
pub const CUBE_VERTS: [[f32; 3]; 24] = {
    let mut v = [[0.0f32; 3]; 24];
    // +X
    v[0] = [0.5, -0.5, -0.5]; v[1] = [0.5, 0.5, -0.5]; v[2] = [0.5, 0.5, 0.5]; v[3] = [0.5, -0.5, 0.5];
    // -X
    v[4] = [-0.5, -0.5, 0.5]; v[5] = [-0.5, 0.5, 0.5]; v[6] = [-0.5, 0.5, -0.5]; v[7] = [-0.5, -0.5, -0.5];
    // +Y
    v[8] = [-0.5, 0.5, -0.5]; v[9] = [-0.5, 0.5, 0.5]; v[10] = [0.5, 0.5, 0.5]; v[11] = [0.5, 0.5, -0.5];
    // -Y
    v[12] = [-0.5, -0.5, 0.5]; v[13] = [-0.5, -0.5, -0.5]; v[14] = [0.5, -0.5, -0.5]; v[15] = [0.5, -0.5, 0.5];
    // +Z
    v[16] = [-0.5, -0.5, 0.5]; v[17] = [0.5, -0.5, 0.5]; v[18] = [0.5, 0.5, 0.5]; v[19] = [-0.5, 0.5, 0.5];
    // -Z
    v[20] = [0.5, -0.5, -0.5]; v[21] = [-0.5, -0.5, -0.5]; v[22] = [-0.5, 0.5, -0.5]; v[23] = [0.5, 0.5, -0.5];
    v
};

pub const CUBE_NORMS: [[f32; 3]; 24] = {
    let mut v = [[0.0f32; 3]; 24];
    let mut i = 0;
    while i < 24 {
        let f = i / 4; // face index
        v[i] = match f {
            0 => [1.0, 0.0, 0.0],
            1 => [-1.0, 0.0, 0.0],
            2 => [0.0, 1.0, 0.0],
            3 => [0.0, -1.0, 0.0],
            4 => [0.0, 0.0, 1.0],
            _ => [0.0, 0.0, -1.0],
        };
        i += 1;
    }
    v
};

pub const CUBE_IDX: [u32; 36] = {
    let mut idx = [0u32; 36];
    let mut f = 0;
    while f < 6 {
        let b = (f * 4) as u32;
        let o = (f * 6) as usize;
        idx[o] = b; idx[o + 1] = b + 1; idx[o + 2] = b + 2;
        idx[o + 3] = b; idx[o + 4] = b + 2; idx[o + 5] = b + 3;
        f += 1;
    }
    idx
};

/// One rendered voxel (3 × vec4 on the GPU).
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub struct VoxInst {
    pub off_ao: [f32; 4],  // xyz = model-space offset, w = baked AO
    pub color: [f32; 4],   // rgba (a<1 → transparent pass)
    pub misc: [f32; 4],    // x = per-instance scale (1.0 for static models)
}

/// A meshed microvoxel model.
#[derive(Default, Clone)]
pub struct VoxelMesh {
    pub inst: Vec<VoxInst>,
}

impl VoxelMesh {
    pub fn instance_count(&self) -> u32 {
        self.inst.len() as u32
    }
    pub fn total_voxels(meshes: &[VoxelMesh]) -> usize {
        meshes.iter().map(|m| m.inst.len()).sum()
    }
}

/// Dense voxel authoring grid.
pub struct Grid {
    pub nx: usize,
    pub ny: usize,
    pub nz: usize,
    cells: Vec<Option<[f32; 4]>>,
}

impl Grid {
    pub fn new(nx: usize, ny: usize, nz: usize) -> Grid {
        Grid { nx, ny, nz, cells: vec![None; nx * ny * nz] }
    }

    #[inline]
    pub fn set(&mut self, x: i32, y: i32, z: i32, c: [f32; 4]) {
        if x < 0 || y < 0 || z < 0 {
            return;
        }
        let (x, y, z) = (x as usize, y as usize, z as usize);
        if x >= self.nx || y >= self.ny || z >= self.nz {
            return;
        }
        self.cells[x + self.nx * (y + self.ny * z)] = Some(c);
    }

    #[inline]
    pub fn get(&self, x: i32, y: i32, z: i32) -> Option<[f32; 4]> {
        if x < 0 || y < 0 || z < 0 {
            return None;
        }
        let (x, y, z) = (x as usize, y as usize, z as usize);
        if x >= self.nx || y >= self.ny || z >= self.nz {
            return None;
        }
        self.cells[x + self.nx * (y + self.ny * z)]
    }

    pub fn filled_box(&mut self, x0: i32, y0: i32, z0: i32, x1: i32, y1: i32, z1: i32, c: [f32; 4]) {
        for x in x0.min(x1)..=x1.max(x0) {
            for y in y0.min(y1)..=y1.max(y0) {
                for z in z0.min(z1)..=z1.max(z0) {
                    self.set(x, y, z, c);
                }
            }
        }
    }

    pub fn ellipsoid(&mut self, cx: f32, cy: f32, cz: f32, rx: f32, ry: f32, rz: f32, c: [f32; 4]) {
        for x in (cx - rx).floor() as i32..=(cx + rx).ceil() as i32 {
            for y in (cy - ry).floor() as i32..=(cy + ry).ceil() as i32 {
                for z in (cz - rz).floor() as i32..=(cz + rz).ceil() as i32 {
                    let dx = (x as f32 - cx) / rx;
                    let dy = (y as f32 - cy) / ry;
                    let dz = (z as f32 - cz) / rz;
                    if dx * dx + dy * dy + dz * dz <= 1.05 {
                        self.set(x, y, z, c);
                    }
                }
            }
        }
    }

    /// Vertical cylinder.
    pub fn cyl_y(&mut self, cx: f32, cz: f32, r: f32, y0: i32, y1: i32, c: [f32; 4]) {
        for y in y0.min(y1)..=y1.max(y0) {
            for x in (cx - r).floor() as i32..=(cx + r).ceil() as i32 {
                for z in (cz - r).floor() as i32..=(cz + r).ceil() as i32 {
                    let dx = x as f32 - cx;
                    let dz = z as f32 - cz;
                    if dx * dx + dz * dz <= r * r + 0.35 {
                        self.set(x, y, z, c);
                    }
                }
            }
        }
    }

    /// Tapering vertical cone/frustum (r at bottom → r_top at top).
    pub fn cone_y(&mut self, cx: f32, cz: f32, r: f32, r_top: f32, y0: i32, y1: i32, c: [f32; 4]) {
        let steps = (y1 - y0).abs().max(1) as f32;
        for y in y0.min(y1)..=y1.max(y0) {
            let t = (y - y0) as f32 / steps;
            let r = r + (r_top - r) * t;
            self.cyl_y_at(cx, cz, r, y, y, c);
        }
    }

    fn cyl_y_at(&mut self, cx: f32, cz: f32, r: f32, y0: i32, y1: i32, c: [f32; 4]) {
        self.cyl_y(cx, cz, r, y0, y1, c);
    }

    /// Clear a spherical region (for hood openings etc).
    pub fn carve_sphere(&mut self, cx: f32, cy: f32, cz: f32, r: f32) {
        for x in (cx - r).floor() as i32..=(cx + r).ceil() as i32 {
            for y in (cy - r).floor() as i32..=(cy + r).ceil() as i32 {
                for z in (cz - r).floor() as i32..=(cz + r).ceil() as i32 {
                    let dx = x as f32 - cx;
                    let dy = y as f32 - cy;
                    let dz = z as f32 - cz;
                    if dx * dx + dy * dy + dz * dz <= r * r {
                        if let Some(c) = self.get(x, y, z) {
                            let _ = c;
                            self.set(x, y, z, [0.0, 0.0, 0.0, 0.0]);
                            let idx = (x as usize) + self.nx * ((y as usize) + self.ny * (z as usize));
                            self.cells[idx] = None;
                        }
                    }
                }
            }
        }
    }

    /// Mesh to world-scale instances.
    /// `feet_origin`: x/z centered, y starts at 0 (characters) — otherwise fully centered.
    pub fn mesh(&self, voxel: f32, feet_origin: bool) -> VoxelMesh {
        // Hull only. This emitted EVERY filled voxel, so a boulder or a
        // trunk was drawn solid all the way through: instances that cost a
        // vertex-shader run and a slot in the buffer and can never be seen.
        // A voxel is dropped only when all six neighbours are OPAQUE, so
        // anything behind glass or leaves with alpha survives. Wind, brush
        // and blast offsets are smooth in world position, so neighbours move
        // together and no gap opens. AO still comes from the full grid.
        self.mesh_impl(voxel, feet_origin, Some(&|_, _, _| true))
    }

    /// Every filled voxel, interior included. For measuring what `mesh`
    /// saves, and for anything that needs the solid.
    pub fn mesh_solid(&self, voxel: f32, feet_origin: bool) -> VoxelMesh {
        self.mesh_impl(voxel, feet_origin, None)
    }

    /// Mesh only the voxels for which `member` is true, as one PART of a
    /// body that will be posed separately (see `rigged`).
    ///
    /// Two things differ from [`Self::mesh`], both because parts move:
    ///
    /// * **Hull only.** A voxel whose six neighbours are all opaque members
    ///   of the SAME part can never be seen, whatever the pose, so it is
    ///   dropped. A neighbour that is filled but belongs to another part
    ///   counts as open: that is the cut face, and it is exactly what shows
    ///   when a leg swings away from the belly.
    /// * **AO and dither come from the WHOLE grid**, not the part, and
    ///   positions use the whole grid's origin. A part at rest is therefore
    ///   pixel-identical to the same voxels in the uncut mesh -- no bright
    ///   seam at the cut, and identity is the rest transform.
    pub fn mesh_where(&self, voxel: f32, feet_origin: bool, member: &dyn Fn(i32, i32, i32) -> bool) -> VoxelMesh {
        self.mesh_impl(voxel, feet_origin, Some(member))
    }

    fn mesh_impl(&self, voxel: f32, feet_origin: bool, member: Option<&dyn Fn(i32, i32, i32) -> bool>) -> VoxelMesh {
        let cx = self.nx as f32 * 0.5;
        let cz = self.nz as f32 * 0.5;
        let mut inst = Vec::with_capacity(self.cells.len() / 2);
        for z in 0..self.nz {
            for y in 0..self.ny {
                for x in 0..self.nx {
                    if let Some(c) = self.cells[x + self.nx * (y + self.ny * z)] {
                        if let Some(member) = member {
                            let (xi, yi, zi) = (x as i32, y as i32, z as i32);
                            if !member(xi, yi, zi) {
                                continue;
                            }
                            let sealed = [(1, 0, 0), (-1, 0, 0), (0, 1, 0), (0, -1, 0), (0, 0, 1), (0, 0, -1)]
                                .iter()
                                .all(|&(dx, dy, dz)| {
                                    let (nx, ny, nz) = (xi + dx, yi + dy, zi + dz);
                                    matches!(self.get(nx, ny, nz), Some(n) if n[3] >= 1.0) && member(nx, ny, nz)
                                });
                            if sealed && c[3] >= 1.0 {
                                continue;
                            }
                        }
                        let ao = self.bake_ao(x as i32, y as i32, z as i32);
                        // subtle deterministic dither for microvoxel texture
                        let d = dither(x as i32, y as i32, z as i32);
                        let mut col = c;
                        col[0] = (col[0] * d).clamp(0.0, 1.0);
                        col[1] = (col[1] * d).clamp(0.0, 1.0);
                        col[2] = (col[2] * d).clamp(0.0, 1.0);
                        let ox = (x as f32 + 0.5 - cx) * voxel;
                        let oz = (z as f32 + 0.5 - cz) * voxel;
                        let oy = if feet_origin {
                            (y as f32 + 0.5) * voxel
                        } else {
                            (y as f32 + 0.5 - self.ny as f32 * 0.5) * voxel
                        };
                        inst.push(VoxInst { off_ao: [ox, oy, oz, ao], color: col, misc: [1.0, 0.0, 0.0, 0.0] });
                    }
                }
            }
        }
        VoxelMesh { inst }
    }
}

fn dither(x: i32, y: i32, z: i32) -> f32 {
    // cheap integer hash → 0.94..1.06 brightness jitter
    let mut h = (x as u32).wrapping_mul(0x9E3779B1) ^ (y as u32).wrapping_mul(0x85EBCA77) ^ (z as u32).wrapping_mul(0xC2B2AE3D);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B3C6D);
    h ^= h >> 12;
    let f = (h & 0xFFFF) as f32 / 65535.0;
    0.94 + 0.12 * f
}

impl Grid {
    /// Baked ambient occlusion from the 26-neighborhood.
    fn bake_ao(&self, x: i32, y: i32, z: i32) -> f32 {
        let mut occ = 0u32;
        let mut total = 0u32;
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    if dx == 0 && dy == 0 && dz == 0 {
                        continue;
                    }
                    total += 1;
                    if self.get(x + dx, y + dy, z + dz).is_some() {
                        occ += 1;
                    }
                }
            }
        }
        1.0 - 0.62 * (occ as f32 / total as f32)
    }
}

/// Merge several meshes into one (used to batch static world props).
pub fn merge(meshes: &[VoxelMesh], offsets: &[Vec3]) -> VoxelMesh {
    let mut out = VoxelMesh::default();
    for (m, off) in meshes.iter().zip(offsets.iter()) {
        for i in &m.inst {
            out.inst.push(VoxInst {
                off_ao: [i.off_ao[0] + off.x, i.off_ao[1] + off.y, i.off_ao[2] + off.z, i.off_ao[3]],
                color: i.color,
                misc: i.misc,
            });
        }
    }
    out
}

/// Merge with a yaw rotation in quarter-turns (0..3, 90° each around Y).
/// Voxels sit on a uniform grid, so 90° steps map exactly (AO included).
pub fn merge_xform(meshes: &[VoxelMesh], xf: &[(Vec3, u8)]) -> VoxelMesh {
    let mut out = VoxelMesh::default();
    for (m, (off, q)) in meshes.iter().zip(xf.iter()) {
        for i in &m.inst {
            let (mut nx, mut nz) = (i.off_ao[0], i.off_ao[2]);
            for _ in 0..(*q % 4) {
                let t = nx;
                nx = nz;
                nz = -t;
            }
            out.inst.push(VoxInst {
                off_ao: [nx + off.x, i.off_ao[1] + off.y, nz + off.z, i.off_ao[3]],
                color: i.color,
                misc: i.misc,
            });
        }
    }
    out
}

/// A coarse copy of a mesh for distance: voxels merged into cells of
/// `2 * vsize` (world units, grid-aligned), one voxel per occupied cell at
/// the cell's centre, colour and AO averaged, the rest copied from the
/// first member. Drawn with voxel size `2 * vsize`. Roughly a quarter to an
/// eighth of the instances; at the distance it is used, the same pixels.
pub fn downsample(mesh: &VoxelMesh, vsize: f32) -> VoxelMesh {
    use std::collections::HashMap;
    let cell = (2.0 * vsize).max(1e-3);
    let mut acc: HashMap<(i32, i32, i32), (VoxInst, [f32; 5], u32)> = HashMap::new();
    for i in &mesh.inst {
        let k = ((i.off_ao[0] / cell).floor() as i32, (i.off_ao[1] / cell).floor() as i32, (i.off_ao[2] / cell).floor() as i32);
        let e = acc.entry(k).or_insert((*i, [0.0; 5], 0));
        e.1[0] += i.color[0];
        e.1[1] += i.color[1];
        e.1[2] += i.color[2];
        e.1[3] += i.color[3];
        e.1[4] += i.off_ao[3];
        e.2 += 1;
    }
    let mut keys: Vec<_> = acc.keys().copied().collect();
    keys.sort_unstable();
    let inst = keys
        .into_iter()
        .map(|k| {
            let (first, sum, n) = acc[&k];
            let n = n as f32;
            VoxInst {
                off_ao: [(k.0 as f32 + 0.5) * cell, (k.1 as f32 + 0.5) * cell, (k.2 as f32 + 0.5) * cell, sum[4] / n],
                color: [sum[0] / n, sum[1] / n, sum[2] / n, sum[3] / n],
                misc: first.misc,
            }
        })
        .collect();
    VoxelMesh { inst }
}

#[cfg(test)]
mod downsample_tests {
    use super::*;

    #[test]
    fn a_coarse_copy_is_smaller_covers_the_same_space_and_keeps_colour() {
        let mut g = Grid::new(20, 20, 20);
        g.ellipsoid(10.0, 10.0, 10.0, 8.0, 8.0, 8.0, [0.4, 0.5, 0.6, 1.0]);
        let fine = g.mesh(0.6, true);
        let coarse = downsample(&fine, 0.6);
        // meshes are HULLS: a surface merges ~3x (area), not 8x (volume)
        assert!(coarse.inst.len() * 2 < fine.inst.len(), "{} of {}", coarse.inst.len(), fine.inst.len());
        // every fine voxel lies inside some coarse voxel (cells of 1.2)
        for f in &fine.inst {
            assert!(coarse.inst.iter().any(|c| (0..3).all(|a| (c.off_ao[a] - f.off_ao[a]).abs() <= 0.6 + 1e-4)), "uncovered {:?}", f.off_ao);
        }
        // colour is averaged, so the overall colour survives (the mesher
        // varies voxels slightly, so a uniform input is not uniform output)
        let mean = |m: &VoxelMesh, k: usize| m.inst.iter().map(|i| i.color[k]).sum::<f32>() / m.inst.len() as f32;
        for k in 0..3 {
            assert!((mean(&fine, k) - mean(&coarse, k)).abs() < 0.03, "channel {k}: {} vs {}", mean(&fine, k), mean(&coarse, k));
        }
        assert!(downsample(&VoxelMesh::default(), 0.6).inst.is_empty());
    }
}
