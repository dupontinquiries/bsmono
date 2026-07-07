//! Coarse occupancy "bones" — a downsampled solidity grid that sits beside the
//! full-resolution octree and answers spatial queries cheaply:
//!
//! - **Advanced reflections**: reflect a ray off a surface and march the coarse
//!   grid ([`OccupancyMip::first_solid_ray`]) to find roughly what it hits — a
//!   one-bounce probe that costs a handful of steps instead of a full-res voxel
//!   walk, perfect for glossy reflections where exactness doesn't matter.
//! - **Long-range sound occlusion**: count solid coarse cells along a line
//!   ([`OccupancyMip::coarse_occlusion`]) for cheap "is there a hill between the
//!   source and the listener?" attenuation when full-res tracing is overkill.
//!
//! The mip downsamples by an integer `factor`: each coarse cell covers a
//! `factor³` block of voxels and is "solid" if at least `threshold` of them are.
//! Building is O(voxels) once; queries are O(ray length in coarse cells).

/// A coarse solid/empty grid downsampled from full voxel occupancy.
#[derive(Debug, Clone, PartialEq)]
pub struct OccupancyMip {
    pub nx: usize,
    pub ny: usize,
    pub nz: usize,
    /// Voxels per coarse cell per axis (coarse cell = `factor³` voxels).
    pub factor: usize,
    solid: Vec<bool>,
}

impl OccupancyMip {
    /// Build from full-resolution voxel solidity. The coarse grid is
    /// `ceil(v / factor)` per axis; a coarse cell is solid when the fraction of
    /// solid voxels inside it is `>= threshold` (0.0 = any solid voxel makes it
    /// solid; 1.0 = must be completely full).
    pub fn from_voxels(
        vnx: usize,
        vny: usize,
        vnz: usize,
        factor: usize,
        threshold: f32,
        is_solid: impl Fn(usize, usize, usize) -> bool,
    ) -> Self {
        let factor = factor.max(1);
        let nx = vnx.div_ceil(factor);
        let ny = vny.div_ceil(factor);
        let nz = vnz.div_ceil(factor);
        let mut solid = vec![false; nx * ny * nz];
        let per_cell = (factor * factor * factor) as f32;
        let need = (threshold.clamp(0.0, 1.0) * per_cell).ceil().max(1.0) as u32;

        for cz in 0..nz {
            for cy in 0..ny {
                for cx in 0..nx {
                    let mut count = 0u32;
                    for dz in 0..factor {
                        let z = cz * factor + dz;
                        if z >= vnz {
                            break;
                        }
                        for dy in 0..factor {
                            let y = cy * factor + dy;
                            if y >= vny {
                                break;
                            }
                            for dx in 0..factor {
                                let x = cx * factor + dx;
                                if x >= vnx {
                                    break;
                                }
                                if is_solid(x, y, z) {
                                    count += 1;
                                }
                            }
                        }
                    }
                    if count >= need {
                        solid[cx + nx * (cy + ny * cz)] = true;
                    }
                }
            }
        }
        Self { nx, ny, nz, factor, solid }
    }

    #[inline]
    pub fn get(&self, cx: usize, cy: usize, cz: usize) -> bool {
        self.solid[cx + self.nx * (cy + self.ny * cz)]
    }

    #[inline]
    fn in_bounds(&self, x: i32, y: i32, z: i32) -> bool {
        x >= 0 && y >= 0 && z >= 0
            && (x as usize) < self.nx
            && (y as usize) < self.ny
            && (z as usize) < self.nz
    }

    /// March a ray (in *coarse-cell* coordinates — divide world coords by
    /// `factor`) and return the first solid coarse cell it enters, or `None` if
    /// it leaves the grid or exceeds `max_cells` steps first. Amanatides–Woo
    /// voxel traversal: exact cell stepping, no skipped cells.
    pub fn first_solid_ray(
        &self,
        origin: [f32; 3],
        dir: [f32; 3],
        max_cells: usize,
    ) -> Option<[usize; 3]> {
        let len = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2]).sqrt();
        if len <= 1e-9 {
            return None;
        }
        let d = [dir[0] / len, dir[1] / len, dir[2] / len];
        let mut cell = [
            origin[0].floor() as i32,
            origin[1].floor() as i32,
            origin[2].floor() as i32,
        ];
        let mut step = [0i32; 3];
        let mut t_max = [f32::INFINITY; 3];
        let mut t_delta = [f32::INFINITY; 3];
        for i in 0..3 {
            if d[i] > 0.0 {
                step[i] = 1;
                t_delta[i] = 1.0 / d[i];
                t_max[i] = (cell[i] as f32 + 1.0 - origin[i]) / d[i];
            } else if d[i] < 0.0 {
                step[i] = -1;
                t_delta[i] = -1.0 / d[i];
                t_max[i] = (cell[i] as f32 - origin[i]) / d[i];
            }
        }

        for _ in 0..max_cells {
            if !self.in_bounds(cell[0], cell[1], cell[2]) {
                return None;
            }
            if self.get(cell[0] as usize, cell[1] as usize, cell[2] as usize) {
                return Some([cell[0] as usize, cell[1] as usize, cell[2] as usize]);
            }
            // Advance along the axis with the nearest cell boundary.
            let axis = if t_max[0] < t_max[1] && t_max[0] < t_max[2] {
                0
            } else if t_max[1] < t_max[2] {
                1
            } else {
                2
            };
            cell[axis] += step[axis];
            t_max[axis] += t_delta[axis];
        }
        None
    }

    /// Count solid coarse cells strictly between two coarse cells along a 3D
    /// Bresenham line (endpoints excluded). Integer + deterministic — a cheap
    /// occlusion estimate for long-range sound. O(longest axis).
    pub fn coarse_occlusion(&self, a: [usize; 3], b: [usize; 3]) -> i32 {
        let a = [a[0] as i32, a[1] as i32, a[2] as i32];
        let b = [b[0] as i32, b[1] as i32, b[2] as i32];
        let d = [(b[0] - a[0]).abs(), (b[1] - a[1]).abs(), (b[2] - a[2]).abs()];
        let s = [
            (b[0] - a[0]).signum(),
            (b[1] - a[1]).signum(),
            (b[2] - a[2]).signum(),
        ];
        let axis = if d[0] >= d[1] && d[0] >= d[2] {
            0
        } else if d[1] >= d[2] {
            1
        } else {
            2
        };
        let dm = d[axis];
        if dm == 0 {
            return 0;
        }
        let (u, v) = match axis {
            0 => (1, 2),
            1 => (0, 2),
            _ => (0, 1),
        };
        let mut p = a;
        let mut e1 = 2 * d[u] - dm;
        let mut e2 = 2 * d[v] - dm;
        let mut solids = 0;
        for _ in 0..dm {
            if e1 > 0 {
                p[u] += s[u];
                e1 -= 2 * dm;
            }
            if e2 > 0 {
                p[v] += s[v];
                e2 -= 2 * dm;
            }
            e1 += 2 * d[u];
            e2 += 2 * d[v];
            p[axis] += s[axis];
            if p != b && self.in_bounds(p[0], p[1], p[2])
                && self.get(p[0] as usize, p[1] as usize, p[2] as usize)
            {
                solids += 1;
            }
        }
        solids
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downsamples_solid_and_empty_regions() {
        // 8³ voxels, factor 4 → 2³ coarse. Make the lower-x half solid.
        let mip = OccupancyMip::from_voxels(8, 8, 8, 4, 0.5, |x, _, _| x < 4);
        assert_eq!((mip.nx, mip.ny, mip.nz), (2, 2, 2));
        // cx=0 covers x 0..4 (all solid) → solid; cx=1 covers x 4..8 (all air).
        assert!(mip.get(0, 0, 0), "solid half → solid coarse cell");
        assert!(!mip.get(1, 0, 0), "empty half → empty coarse cell");
    }

    #[test]
    fn threshold_controls_sensitivity() {
        // One solid voxel in a 4³ cell = 1/64 fraction.
        let one = |x: usize, y: usize, z: usize| x == 0 && y == 0 && z == 0;
        // threshold 0 → any solid voxel makes the cell solid.
        let sensitive = OccupancyMip::from_voxels(4, 4, 4, 4, 0.0, one);
        assert!(sensitive.get(0, 0, 0));
        // threshold 0.5 → a single voxel isn't enough.
        let strict = OccupancyMip::from_voxels(4, 4, 4, 4, 0.5, one);
        assert!(!strict.get(0, 0, 0));
    }

    #[test]
    fn non_multiple_dims_round_up() {
        // 5 voxels, factor 2 → ceil(5/2) = 3 coarse cells per axis.
        let mip = OccupancyMip::from_voxels(5, 5, 5, 2, 0.0, |_, _, _| false);
        assert_eq!((mip.nx, mip.ny, mip.nz), (3, 3, 3));
    }

    #[test]
    fn reflection_ray_hits_a_wall() {
        // A solid slab at cx=5 in a 10³ coarse grid; ray from x=0 along +x hits it.
        let mip = OccupancyMip::from_voxels(10, 1, 1, 1, 0.0, |x, _, _| x == 5);
        let hit = mip.first_solid_ray([0.5, 0.5, 0.5], [1.0, 0.0, 0.0], 32);
        assert_eq!(hit, Some([5, 0, 0]), "ray should land on the slab cell");
    }

    #[test]
    fn reflection_ray_misses_through_open_space() {
        let mip = OccupancyMip::from_voxels(10, 1, 1, 1, 0.0, |_, _, _| false);
        let hit = mip.first_solid_ray([0.5, 0.5, 0.5], [1.0, 0.0, 0.0], 32);
        assert_eq!(hit, None, "empty grid → no reflection hit");
        // A ray pointing out of bounds also returns None.
        let back = mip.first_solid_ray([0.5, 0.5, 0.5], [-1.0, 0.0, 0.0], 32);
        assert_eq!(back, None);
    }

    #[test]
    fn diagonal_reflection_ray_traverses_cells() {
        // Solid corner cell; a diagonal ray should reach it.
        let mip = OccupancyMip::from_voxels(6, 6, 1, 1, 0.0, |x, y, _| x == 4 && y == 4);
        let hit = mip.first_solid_ray([0.5, 0.5, 0.5], [1.0, 1.0, 0.0], 32);
        assert_eq!(hit, Some([4, 4, 0]));
    }

    #[test]
    fn coarse_occlusion_counts_obstacles_between() {
        // A wall cell at cx=3 between (0,0,0) and (6,0,0).
        let mip = OccupancyMip::from_voxels(7, 1, 1, 1, 0.0, |x, _, _| x == 3);
        assert_eq!(mip.coarse_occlusion([0, 0, 0], [6, 0, 0]), 1, "one wall between");
        // Endpoint solidity is not counted.
        let mip_end = OccupancyMip::from_voxels(7, 1, 1, 1, 0.0, |x, _, _| x == 6);
        assert_eq!(mip_end.coarse_occlusion([0, 0, 0], [6, 0, 0]), 0);
        // Clear line → zero.
        let clear = OccupancyMip::from_voxels(7, 1, 1, 1, 0.0, |_, _, _| false);
        assert_eq!(clear.coarse_occlusion([0, 0, 0], [6, 0, 0]), 0);
    }

    #[test]
    fn occupancy_keeps_cells_that_low_corner_sampling_drops() {
        // The mid-LOD mesher bug: the GPU shader picks each stride³ group's
        // LOW-CORNER block as the cell's representative, and skips the whole
        // group if that one block is air. Here a 2³ group is solid only at its
        // far corner (1,1,1), low corner (0,0,0) is air — exactly a surface
        // cell straddling solid/air, which is most of a fragmented region.
        let is_solid = |x: usize, y: usize, z: usize| (x, y, z) == (1, 1, 1);

        // Low-corner sampling (what the shader does today) → the cell is
        // classified air and the group emits nothing. This is the hole.
        assert!(!is_solid(0, 0, 0), "low-corner sample misses the solid block");

        // Group-occupancy sampling (the fix): any solid block in the group
        // (threshold 0.0) makes the coarse cell solid → the group emits.
        let mip = OccupancyMip::from_voxels(2, 2, 2, 2, 0.0, is_solid);
        assert!(mip.get(0, 0, 0), "occupancy keeps the cell the low corner drops");

        // A fully-air group is still correctly empty under occupancy — the fix
        // doesn't over-fill, so solid regions don't bloat.
        let empty = OccupancyMip::from_voxels(2, 2, 2, 2, 0.0, |_, _, _| false);
        assert!(!empty.get(0, 0, 0), "all-air group stays empty");
    }
}
