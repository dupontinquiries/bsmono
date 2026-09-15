//! `sim::neighbor_grid` — a uniform spatial hash for fauna **broadphase**.
//!
//! The VISION doc flags an O(n²) mating scan at the 96-mob cap: every creature
//! compared against every other to find a nearby mate / rival / flockmate. This
//! bins creatures into a coarse cell grid so a "who's near me" query only visits
//! the 3×3×3 cells around a point — turning the scan into O(n) for the usual case
//! where creatures are spread out.
//!
//! Use `cell` ≥ the largest interaction radius, so every candidate within radius
//! lands in the queried cell or one of its 26 neighbors. Rebuild once per tick
//! (insert all live creatures), then query per creature. Pure + deterministic:
//! candidates are visited in a fixed `(dx, dy, dz)` order and, within a cell, in
//! insertion order — never via `HashMap` iteration.

use std::collections::HashMap;

/// A uniform grid of entity ids, keyed by integer cell.
#[derive(Default)]
pub struct NeighborGrid {
    cell: i32,
    bins: HashMap<(i32, i32, i32), Vec<u32>>,
}

impl NeighborGrid {
    /// `cell` is the bin edge length in blocks (≥ the interaction radius). Must be
    /// positive.
    pub fn new(cell: i32) -> NeighborGrid {
        assert!(cell > 0, "cell size must be positive");
        NeighborGrid { cell, bins: HashMap::new() }
    }

    /// Drop all bins. Call once per tick before re-inserting.
    pub fn clear(&mut self) {
        self.bins.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.bins.values().all(|v| v.is_empty())
    }

    #[inline]
    fn cell_of(&self, x: i32, y: i32, z: i32) -> (i32, i32, i32) {
        // div_euclid floors toward −∞ so negative coordinates bin correctly.
        (x.div_euclid(self.cell), y.div_euclid(self.cell), z.div_euclid(self.cell))
    }

    /// Insert entity `id` at world position `(x, y, z)`.
    pub fn insert(&mut self, id: u32, x: i32, y: i32, z: i32) {
        self.bins.entry(self.cell_of(x, y, z)).or_default().push(id);
    }

    /// Visit every entity id binned in the 3×3×3 block of cells around
    /// `(x, y, z)` — the broadphase candidate set. Deterministic order. Note the
    /// querying entity sees *itself* if it was inserted; the caller skips `id == self`.
    pub fn for_candidates<F: FnMut(u32)>(&self, x: i32, y: i32, z: i32, mut f: F) {
        let (cx, cy, cz) = self.cell_of(x, y, z);
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    if let Some(v) = self.bins.get(&(cx + dx, cy + dy, cz + dz)) {
                        for &id in v {
                            f(id);
                        }
                    }
                }
            }
        }
    }

    /// Convenience: collect the candidate ids near a point.
    pub fn candidates(&self, x: i32, y: i32, z: i32) -> Vec<u32> {
        let mut out = Vec::new();
        self.for_candidates(x, y, z, |id| out.push(id));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_entities_in_the_same_cell() {
        let mut g = NeighborGrid::new(8);
        g.insert(1, 0, 0, 0);
        g.insert(2, 3, 1, 2);
        let c = g.candidates(1, 0, 1);
        assert!(c.contains(&1) && c.contains(&2));
    }

    #[test]
    fn finds_across_a_cell_boundary() {
        let mut g = NeighborGrid::new(8);
        g.insert(1, 7, 0, 0); // cell (0,0,0)
        g.insert(2, 9, 0, 0); // cell (1,0,0) — adjacent
        // querying near the boundary sees both (they're within the 3x3x3 block)
        assert!(g.candidates(8, 0, 0).contains(&1));
        assert!(g.candidates(8, 0, 0).contains(&2));
    }

    #[test]
    fn ignores_entities_two_cells_away() {
        let mut g = NeighborGrid::new(8);
        g.insert(1, 0, 0, 0); // cell 0
        g.insert(2, 24, 0, 0); // cell 3 — two cells past the 3x3x3 block around cell 0
        let c = g.candidates(0, 0, 0);
        assert!(c.contains(&1));
        assert!(!c.contains(&2), "far entity is not a candidate");
    }

    #[test]
    fn negative_coordinates_bin_correctly() {
        let mut g = NeighborGrid::new(8);
        g.insert(1, -1, -1, -1); // cell (-1,-1,-1)
        g.insert(2, -9, 0, 0); // cell (-2,0,0)
        assert!(g.candidates(-1, -1, -1).contains(&1));
        // -9 is in cell -2; querying at -1 (cell -1) includes neighbor -2
        assert!(g.candidates(-1, 0, 0).contains(&2));
    }

    #[test]
    fn spread_out_population_is_linear_not_quadratic() {
        // Put N creatures each in their own far-apart cell. Each query then visits
        // ~1 candidate, so total work is O(N), not O(N^2).
        let mut g = NeighborGrid::new(8);
        let n = 200u32;
        for i in 0..n {
            g.insert(i, (i as i32) * 100, 0, 0); // 100 blocks apart -> distinct cells, no overlap
        }
        let mut total_visits = 0u64;
        for i in 0..n {
            g.for_candidates((i as i32) * 100, 0, 0, |_| total_visits += 1);
        }
        // Each creature sees only itself: ~N visits, vastly less than N*N.
        assert_eq!(total_visits, n as u64);
        assert!(total_visits < (n as u64) * (n as u64) / 10);
    }

    #[test]
    fn candidate_order_is_deterministic() {
        let mut g = NeighborGrid::new(8);
        for i in 0..10 {
            g.insert(i, (i % 3) as i32, 0, (i / 3) as i32);
        }
        assert_eq!(g.candidates(1, 0, 1), g.candidates(1, 0, 1));
    }

    #[test]
    fn clear_empties_the_grid() {
        let mut g = NeighborGrid::new(4);
        g.insert(1, 0, 0, 0);
        assert!(!g.is_empty());
        g.clear();
        assert!(g.is_empty());
        assert!(g.candidates(0, 0, 0).is_empty());
    }
}
