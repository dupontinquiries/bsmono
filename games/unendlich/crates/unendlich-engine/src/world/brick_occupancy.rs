//! Brick occupancy — the cheap win that keeps the per-brick mesher from doing
//! work it doesn't need to. Most bricks in a voxel world are uniform: empty air,
//! or solid interior buried inside more solid. Neither produces any visible
//! faces, so meshing them (a full 8³ × 6-direction sweep) is wasted. This
//! classifies a brick in one pass into Empty / Full / Mixed, and then — given the
//! same classification for its 6 neighbours — answers in O(1) whether it needs
//! meshing at all and which face directions can carry geometry.
//!
//! The engine maintains this per brick (cheap to update on an edit) and skips the
//! mesher for trivial bricks, which is the large majority. Pure, deterministic.

use crate::world::mesh_brick::BRICK_SIZE;

/// What a brick contains.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Occupancy {
    /// All air — draws nothing.
    Empty,
    /// All solid — only its outer shell can be visible.
    Full,
    /// A surface runs through it — must be meshed.
    Mixed,
}

/// Classify a brick from a solidity closure over its local coords `0..BRICK_SIZE`
/// (air = block id 0).
pub fn classify(block_at: impl Fn([i32; 3]) -> u16) -> Occupancy {
    let mut saw_air = false;
    let mut saw_solid = false;
    for x in 0..BRICK_SIZE {
        for y in 0..BRICK_SIZE {
            for z in 0..BRICK_SIZE {
                if block_at([x, y, z]) == 0 {
                    saw_air = true;
                } else {
                    saw_solid = true;
                }
                if saw_air && saw_solid {
                    return Occupancy::Mixed; // surface present, bail early
                }
            }
        }
    }
    if saw_solid {
        Occupancy::Full
    } else {
        Occupancy::Empty
    }
}

impl Occupancy {
    /// Does this brick need meshing, given its 6 neighbours
    /// (engine face order: +Y, -Y, +X, -X, +Z, -Z)?
    ///
    /// - `Empty`: never (no solids).
    /// - `Mixed`: always (an interior surface can face any way).
    /// - `Full`: only if some neighbour isn't also `Full` — i.e. part of its
    ///   shell is exposed. Buried solid (all neighbours `Full`) → skip.
    pub fn needs_mesh(self, neighbors: [Occupancy; 6]) -> bool {
        match self {
            Occupancy::Empty => false,
            Occupancy::Mixed => true,
            Occupancy::Full => neighbors.iter().any(|&n| n != Occupancy::Full),
        }
    }

    /// Bitmask (bit d = face dir d) of directions that can carry geometry, so the
    /// mesher only sweeps those. `Full` exposes a face only where the neighbour
    /// isn't `Full`; `Mixed` is conservative (all six); `Empty` none.
    pub fn exposed_faces(self, neighbors: [Occupancy; 6]) -> u8 {
        match self {
            Occupancy::Empty => 0,
            Occupancy::Mixed => 0b111111,
            Occupancy::Full => {
                let mut m = 0u8;
                for (d, &n) in neighbors.iter().enumerate() {
                    if n != Occupancy::Full {
                        m |= 1 << d;
                    }
                }
                m
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Occupancy::*;

    const ALL_FULL: [Occupancy; 6] = [Full, Full, Full, Full, Full, Full];

    #[test]
    fn classifies_uniform_and_mixed_bricks() {
        assert_eq!(classify(|_| 0), Empty, "all air");
        assert_eq!(classify(|_| 1), Full, "all solid");
        assert_eq!(classify(|p| if p[1] < 4 { 1 } else { 0 }), Mixed, "half solid → surface");
    }

    #[test]
    fn empty_never_meshes() {
        assert!(!Empty.needs_mesh([Empty; 6]));
        assert!(!Empty.needs_mesh(ALL_FULL));
        assert_eq!(Empty.exposed_faces(ALL_FULL), 0);
    }

    #[test]
    fn buried_solid_is_skipped_but_exposed_solid_is_not() {
        assert!(!Full.needs_mesh(ALL_FULL), "fully buried → no visible faces → skip");
        // One neighbour (the +Y top) is air → the top shell is exposed.
        let mut nb = ALL_FULL;
        nb[0] = Empty;
        assert!(Full.needs_mesh(nb));
        assert_eq!(Full.exposed_faces(nb), 0b000001, "only the +Y face is exposed");
    }

    #[test]
    fn mixed_always_meshes_all_directions() {
        assert!(Mixed.needs_mesh(ALL_FULL), "an internal surface can face into solid neighbours");
        assert_eq!(Mixed.exposed_faces(ALL_FULL), 0b111111);
    }

    #[test]
    fn a_mostly_uniform_chunk_skips_the_vast_majority() {
        // Model a 4×4×4 = 64-brick chunk: a single solid floor layer (the bottom
        // brick-layer Full, the column below would be Full too), the rest air.
        // Count how many of 64 bricks actually need meshing.
        let n = 4; // bricks per axis
        let occ = |_bx: i32, by: i32, _bz: i32| if by == 0 { Full } else { Empty };
        let mut meshed = 0;
        for bz in 0..n {
            for by in 0..n {
                for bx in 0..n {
                    let me = occ(bx, by, bz);
                    let nb = [
                        occ(bx, by + 1, bz),
                        occ(bx, by - 1, bz),
                        occ(bx + 1, by, bz),
                        occ(bx - 1, by, bz),
                        occ(bx, by, bz + 1),
                        occ(bx, by, bz - 1),
                    ];
                    if me.needs_mesh(nb) {
                        meshed += 1;
                    }
                }
            }
        }
        // Only the 16 floor bricks (top exposed to air) mesh; the other 48 skip.
        assert_eq!(meshed, 16, "48 of 64 bricks skipped in O(1)");
    }
}
