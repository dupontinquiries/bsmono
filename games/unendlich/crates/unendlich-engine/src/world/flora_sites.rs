//! Cave-flora placement — choosing *where* stems root and *which way* they grow.
//!
//! A site is an air cell touching a solid surface, underground, with enough open
//! runway ahead to grow into. The growth normal points off the attached surface,
//! so the direction is read straight from geometry: a pearl-stem on the floor
//! grows up, one on a wall grows outward, one on the ceiling hangs down. Sites
//! are picked sparsely by a determinism hash keyed on cell + world seed, so every
//! host populates a region identically and a per-site seed feeds each
//! [`crate::world::stem::StemGraph`]. Stays `BlockId`-agnostic: it reads only an
//! `occupied` predicate and returns roots for the assembler to grow.

use crate::world::stem::Cell;

/// Fraction of eligible surface cells that sprout flora.
const DENSITY: f32 = 0.05;
/// Cells of open space probed ahead of a candidate normal.
const RUNWAY: i32 = 3;
/// Minimum clear runway for a site to be viable.
const MIN_RUNWAY: i32 = 2;
/// The six face directions.
const FACES: [(i8, i8, i8); 6] =
    [(1, 0, 0), (-1, 0, 0), (0, 1, 0), (0, -1, 0), (0, 0, 1), (0, 0, -1)];

/// A chosen place to root a stem.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FloraSite {
    /// Air cell the stem roots in (against the surface).
    pub root: Cell,
    /// Growth normal, pointing off the attached surface into open air.
    pub dir: (i8, i8, i8),
    /// Per-site seed for the stem's reproducible growth.
    pub seed: u64,
}

/// splitmix64 of a cell + seed (mirrors the engine's determinism hashing).
fn site_hash(cell: Cell, seed: u64) -> u64 {
    let (x, y, z) = cell;
    let mut h: u64 = 0x9E37_79B9_7F4A_7C15;
    h ^= (x as i64 as u64).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= (y as i64 as u64).wrapping_mul(0x94D0_49BB_1331_11EB);
    h ^= (z as i64 as u64).wrapping_mul(0x2545_F491_4F6C_DD1D);
    h ^= seed.wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    h ^= h >> 33;
    h = h.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
    h ^= h >> 33;
    h = h.wrapping_mul(0xC4CE_B9FE_1A85_EC53);
    h ^= h >> 33;
    h
}

fn hash01(cell: Cell, seed: u64) -> f32 {
    ((site_hash(cell, seed) >> 40) as f32) / (1u32 << 24) as f32
}

/// Prefer floor (grow up), then walls (grow out), then ceiling (hang down).
fn priority(n: (i8, i8, i8)) -> u8 {
    if n == (0, 1, 0) {
        0
    } else if n.1 == 0 {
        1
    } else {
        2
    }
}

/// Find all flora sites in the box `[min, min+dims)`. `occupied` reports solid
/// world cells; `max_y` keeps sites underground (only roots at or below it).
pub fn find_sites<O>(min: Cell, dims: (i32, i32, i32), world_seed: u64, max_y: i32, occupied: &O) -> Vec<FloraSite>
where
    O: Fn(Cell) -> bool,
{
    let mut sites = Vec::new();
    for x in min.0..min.0 + dims.0 {
        for y in min.1..min.1 + dims.1 {
            for z in min.2..min.2 + dims.2 {
                let cell = (x, y, z);
                if y > max_y || occupied(cell) {
                    continue; // surface-side air only, underground
                }
                // Gather viable normals: opposite a solid face, with open runway.
                let mut norms: Vec<(i32, (i8, i8, i8))> = Vec::new();
                for f in FACES {
                    if occupied((x + f.0 as i32, y + f.1 as i32, z + f.2 as i32)) {
                        let nrm = (-f.0, -f.1, -f.2);
                        let mut run = 0;
                        for k in 1..=RUNWAY {
                            let c = (x + nrm.0 as i32 * k, y + nrm.1 as i32 * k, z + nrm.2 as i32 * k);
                            if occupied(c) {
                                break;
                            }
                            run += 1;
                        }
                        if run >= MIN_RUNWAY {
                            norms.push((run, nrm));
                        }
                    }
                }
                if norms.is_empty() {
                    continue;
                }
                if hash01(cell, world_seed) >= DENSITY {
                    continue; // sparse scatter
                }
                norms.sort_by(|a, b| {
                    b.0.cmp(&a.0)
                        .then(priority(a.1).cmp(&priority(b.1)))
                        .then(a.1.cmp(&b.1))
                });
                sites.push(FloraSite {
                    root: cell,
                    dir: norms[0].1,
                    seed: site_hash(cell, world_seed ^ 0x5DEE_CE66_D),
                });
            }
        }
    }
    sites
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: Cell = (-30, -5, -30);
    const DIMS: (i32, i32, i32) = (60, 12, 60);

    #[test]
    fn floor_sites_grow_up() {
        let s = find_sites(MIN, DIMS, 7, 8, &|c: Cell| c.1 < 0);
        assert!(!s.is_empty());
        assert!(s.iter().all(|f| f.dir == (0, 1, 0) && f.root.1 == 0), "floor flora roots at y=0 growing up");
    }

    #[test]
    fn ceiling_sites_hang_down() {
        let s = find_sites(MIN, DIMS, 7, 8, &|c: Cell| c.1 >= 1);
        assert!(!s.is_empty());
        assert!(s.iter().all(|f| f.dir == (0, -1, 0)), "ceiling flora hangs down");
    }

    #[test]
    fn wall_sites_grow_outward() {
        let s = find_sites(MIN, DIMS, 7, 8, &|c: Cell| c.0 >= 1);
        assert!(!s.is_empty());
        assert!(s.iter().all(|f| f.dir == (-1, 0, 0)), "wall flora grows away from the +x wall");
    }

    #[test]
    fn deterministic_and_gated() {
        let occ = |c: Cell| c.1 < 0;
        assert_eq!(find_sites(MIN, DIMS, 7, 8, &occ), find_sites(MIN, DIMS, 7, 8, &occ), "same seed → same sites");
        assert!(find_sites(MIN, DIMS, 7, 8, &|_: Cell| false).is_empty(), "no surface → no sites");
        assert!(find_sites(MIN, DIMS, 7, -3, &occ).is_empty(), "above max_y → no sites");
    }

    #[test]
    fn density_is_sparse() {
        let n = find_sites(MIN, DIMS, 7, 8, &|c: Cell| c.1 < 0).len();
        let frac = n as f32 / (60.0 * 60.0);
        assert!((0.02..0.08).contains(&frac), "selected fraction near DENSITY: {frac}");
    }
}
