//! `sim::spatial_grid` — a coarse uniform grid that turns an O(n²) all-pairs scan
//! into an ~O(n) neighbour-pairs scan. The omniscient "compare every creature to
//! every other" pattern (breeding mate-finding, predator prey-selection) is the
//! wrong shape: interactions are LOCAL, so candidates should be found by proximity.
//!
//! Bin each entity into a cell of side `cell_size`; two entities within `cell_size`
//! are then guaranteed to share a cell or sit in adjacent cells. [`neighbour_pairs`]
//! emits every such candidate pair exactly once. The caller still does the precise
//! distance/eligibility test — the grid only bounds the candidate set, so picking
//! `cell_size >= interaction_radius` guarantees no real pair is missed.
//!
//! Cost: O(n) to bin + O(pairs) to emit, where `pairs` is bounded by local density
//! (the count of entities packed into a neighbourhood), not n. Worst case (every
//! entity in one cell) degrades to O(n²) — but that only happens when everything is
//! mutually adjacent, i.e. when there genuinely are n² interactions to consider.
//!
//! Pure & deterministic: positions in, index pairs out. No clock / RNG / IO, no
//! panic. The emitted pair order is deterministic for a given input (cells visited
//! in insertion order, members in push order) so replays match.

use std::collections::HashMap;

/// The four "forward" neighbour offsets. Each cell checks only these, so every
/// adjacency between two distinct cells is visited from exactly one side (the
/// reverse offsets — (-1,0), (0,-1), (1,-1), (-1,-1) — are covered when the OTHER
/// cell looks forward). Together with within-cell pairs this yields every
/// neighbouring pair exactly once.
const FORWARD: [(i32, i32); 4] = [(1, 0), (1, 1), (0, 1), (-1, 1)];

#[inline]
fn cell_of(p: [f32; 3], inv: f32) -> (i32, i32) {
    // x/z plane — creatures live on a surface, so vertical separation is ignored
    // for neighbourhood bucketing (the caller's distance test can still use y).
    ((p[0] * inv).floor() as i32, (p[2] * inv).floor() as i32)
}

/// Bin `positions` into grid cells of side `cell_size`. Returns a map from cell to
/// the indices of the positions in it (in push order). `cell_size <= 0` is treated
/// as a tiny epsilon so every entity lands in its own cell (degenerate but safe).
pub fn build(positions: &[[f32; 3]], cell_size: f32) -> HashMap<(i32, i32), Vec<usize>> {
    let inv = 1.0 / cell_size.max(1e-3);
    let mut grid: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
    for (idx, &p) in positions.iter().enumerate() {
        grid.entry(cell_of(p, inv)).or_default().push(idx);
    }
    grid
}

/// Every candidate pair `(i, j)` with `i < j` whose entities share a cell or sit in
/// adjacent cells, each pair emitted exactly once. `cell_size` should be `>=` the
/// caller's interaction radius so no real pair is missed. The caller applies the
/// precise distance/eligibility test to each emitted pair.
pub fn neighbour_pairs(positions: &[[f32; 3]], cell_size: f32) -> Vec<(usize, usize)> {
    let grid = build(positions, cell_size);
    let mut pairs = Vec::new();
    for (&(cx, cz), members) in &grid {
        // within-cell pairs
        for a in 0..members.len() {
            for b in (a + 1)..members.len() {
                let (i, j) = (members[a], members[b]);
                pairs.push((i.min(j), i.max(j)));
            }
        }
        // forward-neighbour pairs (each cross-cell adjacency visited once)
        for (dx, dz) in FORWARD {
            if let Some(other) = grid.get(&(cx + dx, cz + dz)) {
                for &i in members {
                    for &j in other {
                        pairs.push((i.min(j), i.max(j)));
                    }
                }
            }
        }
    }
    pairs
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn at(x: f32, z: f32) -> [f32; 3] {
        [x, 0.0, z]
    }

    /// Brute-force reference: all pairs within `radius` (the property the grid must
    /// preserve — every such pair appears in the candidate set).
    fn brute_within(positions: &[[f32; 3]], radius: f32) -> HashSet<(usize, usize)> {
        let r2 = radius * radius;
        let mut out = HashSet::new();
        for i in 0..positions.len() {
            for j in (i + 1)..positions.len() {
                let d2 = (positions[i][0] - positions[j][0]).powi(2)
                    + (positions[i][2] - positions[j][2]).powi(2);
                if d2 <= r2 {
                    out.insert((i, j));
                }
            }
        }
        out
    }

    #[test]
    fn empty_and_singleton() {
        assert!(neighbour_pairs(&[], 3.0).is_empty());
        assert!(neighbour_pairs(&[at(0.0, 0.0)], 3.0).is_empty());
    }

    #[test]
    fn each_pair_emitted_at_most_once() {
        // a clustered blob — many within-cell + cross-cell pairs
        let pts: Vec<[f32; 3]> = (0..20).map(|k| at((k % 5) as f32, (k / 5) as f32)).collect();
        let pairs = neighbour_pairs(&pts, 3.0);
        let set: HashSet<_> = pairs.iter().copied().collect();
        assert_eq!(pairs.len(), set.len(), "no pair emitted twice");
        // all normalized i<j
        assert!(pairs.iter().all(|&(i, j)| i < j));
    }

    #[test]
    fn superset_of_all_close_pairs() {
        // the key correctness property: with cell_size >= radius, the candidate set
        // CONTAINS every pair that's actually within radius.
        let pts = [
            at(0.0, 0.0),
            at(2.0, 1.0),   // within 3 of #0
            at(10.0, 10.0), // far
            at(11.0, 10.5), // within 3 of #2
            at(2.5, 2.5),   // within 3 of #0 and #1
        ];
        let radius = 3.0;
        let candidates: HashSet<_> = neighbour_pairs(&pts, radius).into_iter().collect();
        for pair in brute_within(&pts, radius) {
            assert!(candidates.contains(&pair), "missed close pair {pair:?}");
        }
    }

    #[test]
    fn distant_clusters_do_not_cross_pair() {
        // two tight clusters far apart → no candidate pair bridges them (the whole
        // point: O(n^2) cross-cluster comparisons are avoided).
        let mut pts = vec![at(0.0, 0.0), at(1.0, 0.0), at(0.0, 1.0)];
        pts.extend([at(100.0, 100.0), at(101.0, 100.0), at(100.0, 101.0)]);
        let pairs = neighbour_pairs(&pts, 3.0);
        for &(i, j) in &pairs {
            let cross = (i < 3) != (j < 3);
            assert!(!cross, "cluster bridge {i},{j} should not be a candidate");
        }
    }

    #[test]
    fn matches_brute_force_on_random_layout() {
        // deterministic pseudo-random scatter; the grid candidate set must be a
        // superset of brute-force-within for a few radii.
        let mut x = 0x1234_5678u64;
        let mut rng = || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 40) as f32 / (1u64 << 24) as f32 * 50.0
        };
        let pts: Vec<[f32; 3]> = (0..60).map(|_| at(rng(), rng())).collect();
        for &radius in &[2.0f32, 5.0, 9.0] {
            let candidates: HashSet<_> = neighbour_pairs(&pts, radius).into_iter().collect();
            for pair in brute_within(&pts, radius) {
                assert!(candidates.contains(&pair), "radius {radius}: missed {pair:?}");
            }
        }
    }

    #[test]
    fn determinism() {
        let pts = [at(0.0, 0.0), at(1.0, 1.0), at(2.0, 0.5), at(1.5, 2.0)];
        assert_eq!(neighbour_pairs(&pts, 3.0), neighbour_pairs(&pts, 3.0));
    }

    #[test]
    fn far_fewer_pairs_than_n_squared_when_spread() {
        // entities spread one-per-cell → almost no pairs, vs n(n-1)/2 brute pairs.
        let pts: Vec<[f32; 3]> = (0..50).map(|k| at(k as f32 * 10.0, 0.0)).collect();
        let pairs = neighbour_pairs(&pts, 3.0);
        let brute = 50 * 49 / 2;
        assert!(pairs.len() < brute / 10, "expected far fewer than {brute}, got {}", pairs.len());
    }
}
