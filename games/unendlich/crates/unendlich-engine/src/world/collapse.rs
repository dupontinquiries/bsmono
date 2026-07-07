//! Collapse — structural support, floating detection, and gravity settling.
//!
//! The companion to [`crate::world::erosion`] and
//! [`crate::world::elemental::smash_crater`]: when erosion washes a cell away
//! or a smash clears one, whatever it was holding up loses its load path and
//! falls. Pure and predicate-driven — solidity and anchoring are supplied by
//! the caller, so this never touches the block registry.
//!
//! Three collapse modes:
//!   - **Floating**: a solid cell with no support path to an anchor (you cut
//!     out the base) — it falls as a body.
//!   - **Overhang**: a supported cell whose load path is longer than a span
//!     budget — a cantilever that snaps under its own reach.
//!   - **Settle**: loose/powder cells (sand) drop by gravity onto the resting
//!     surface beneath them.
//!
//! Support is 6-connected: a cell is supported if it connects to an anchor
//! through solid neighbors. A tall grounded column is fine; a far horizontal
//! reach is not (its support distance grows with every step out).

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

/// A world cell.
pub type Cell = (i32, i32, i32);

const NEIGHBORS: [Cell; 6] = [
    (1, 0, 0),
    (-1, 0, 0),
    (0, 1, 0),
    (0, -1, 0),
    (0, 0, 1),
    (0, 0, -1),
];

/// 6-connected support distance from `anchors` through `solid` cells: the
/// shortest number of solid steps from any anchor to each reachable cell.
/// Cells absent from the returned map are unreachable — i.e. floating. An
/// anchor must itself be solid to seed the flood.
pub fn support_distance(solid: &HashSet<Cell>, anchors: &[Cell]) -> HashMap<Cell, u32> {
    let mut dist: HashMap<Cell, u32> = HashMap::new();
    let mut q: VecDeque<Cell> = VecDeque::new();
    for &a in anchors {
        if solid.contains(&a) && !dist.contains_key(&a) {
            dist.insert(a, 0);
            q.push_back(a);
        }
    }
    while let Some(c) = q.pop_front() {
        let d = dist[&c] + 1;
        for (dx, dy, dz) in NEIGHBORS {
            let n = (c.0 + dx, c.1 + dy, c.2 + dz);
            if solid.contains(&n) && !dist.contains_key(&n) {
                dist.insert(n, d);
                q.push_back(n);
            }
        }
    }
    dist
}

/// Solid cells with no support path to an anchor — they're floating and fall
/// as a body. Pass the `support` map from [`support_distance`].
pub fn floating(solid: &HashSet<Cell>, support: &HashMap<Cell, u32>) -> Vec<Cell> {
    solid
        .iter()
        .filter_map(|&c| if support.contains_key(&c) { None } else { Some(c) })
        .collect()
}

/// Supported cells whose load path exceeds `max_span` — overhangs/cantilevers
/// that collapse under their own reach.
pub fn overhang(support: &HashMap<Cell, u32>, max_span: u32) -> Vec<Cell> {
    support
        .iter()
        .filter_map(|(&c, &d)| if d > max_span { Some(c) } else { None })
        .collect()
}

/// Settle loose (powder) cells downward by gravity. Within each `(x, z)`
/// column they restack from the resting surface up (processed bottom-up so
/// they don't overlap). `floor_of(x, z)` gives the y of the top of the solid
/// stack in that column (the cell rests at `floor + 1`, `floor + 2`, …).
/// Returns the `(from, to)` moves for cells that actually fell.
pub fn settle<F: Fn(i32, i32) -> i32>(loose: &[Cell], floor_of: F) -> Vec<(Cell, Cell)> {
    let mut cols: BTreeMap<(i32, i32), Vec<i32>> = BTreeMap::new();
    for &(x, y, z) in loose {
        cols.entry((x, z)).or_default().push(y);
    }
    let mut moves = Vec::new();
    for ((x, z), mut ys) in cols {
        ys.sort_unstable();
        let mut rest = floor_of(x, z);
        for y in ys {
            let ny = rest + 1;
            if ny != y {
                moves.push(((x, y, z), (x, ny, z)));
            }
            rest = ny;
        }
    }
    moves
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A grounded column + a horizontal cantilever arm + a detached island.
    fn scene() -> (HashSet<Cell>, Vec<Cell>) {
        let mut solid: HashSet<Cell> = HashSet::new();
        for y in 0..5 {
            solid.insert((0, y, 0)); // column x0, y0..4
        }
        for x in 1..7 {
            solid.insert((x, 4, 0)); // arm at y4, x1..6
        }
        // detached floating island
        for c in [(20, 8, 0), (21, 8, 0), (20, 9, 0)] {
            solid.insert(c);
        }
        let anchors: Vec<Cell> = (-2..8).map(|x| (x, 0, 0)).collect();
        (solid, anchors)
    }

    #[test]
    fn floating_detects_disconnected_island() {
        let (solid, anchors) = scene();
        let support = support_distance(&solid, &anchors);
        let fl = floating(&solid, &support);
        assert_eq!(fl.len(), 3, "the detached island floats");
        for c in [(20, 8, 0), (21, 8, 0), (20, 9, 0)] {
            assert!(fl.contains(&c));
        }
    }

    #[test]
    fn column_and_arm_are_reached_with_growing_distance() {
        let (solid, anchors) = scene();
        let support = support_distance(&solid, &anchors);
        assert_eq!(support.len(), 11, "11 connected cells reached");
        assert_eq!(support[&(0, 0, 0)], 0, "anchor is distance 0");
        assert_eq!(support[&(6, 4, 0)], 10, "arm tip = 4 up + 6 out");
    }

    #[test]
    fn overhang_beyond_span_collapses() {
        let (solid, anchors) = scene();
        let support = support_distance(&solid, &anchors);
        let mut over = overhang(&support, 8);
        over.sort();
        assert_eq!(over, vec![(5, 4, 0), (6, 4, 0)]);
    }

    #[test]
    fn settle_stacks_loose_bottom_up() {
        // Two loose cells in column (3, _, 1), resting surface at y=2.
        let loose = vec![(3, 5, 1), (3, 7, 1)];
        let mut moves = settle(&loose, |_x, _z| 2);
        moves.sort();
        assert_eq!(
            moves,
            vec![((3, 5, 1), (3, 3, 1)), ((3, 7, 1), (3, 4, 1))]
        );
    }

    #[test]
    fn settle_leaves_resting_cells_alone() {
        // A cell already sitting at floor+1 doesn't move.
        let loose = vec![(0, 3, 0)];
        let moves = settle(&loose, |_x, _z| 2);
        assert!(moves.is_empty());
    }
}
