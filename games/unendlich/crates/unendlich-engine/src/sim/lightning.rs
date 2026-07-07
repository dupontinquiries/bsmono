//! Lightning conduction — how a bolt chooses its path. A real strike doesn't go
//! straight; it seeks the least-resistance route, leaping toward and through
//! water, wet ground, and metal and shunning dry insulators. So the lightning
//! staff's bolt is a **least-resistance path** through the voxel grid: every
//! cell has a conductivity (water and metal high, dry air low, raised by local
//! moisture), the bolt pays the inverse as cost, and Dijkstra finds the channel.
//! Drop a puddle or an iron rod in its way and the arc bends to it.
//!
//! Deterministic (integer costs, coordinate tie-break) so it rolls back with the
//! sim. The caller maps world block + local humidity → conductivity via
//! [`conductivity`]; `arc_path` is world-agnostic (takes a closure), so it works
//! against the live world or a test grid.

use crate::world::blocks::BlockId;
use crate::world::rock::rock_physics;

/// Max conductivity (a perfect conductor). Resistance is `MAX_CONDUCTIVITY −
/// conductivity + 1`, so even a perfect conductor costs 1 per step (the search
/// still prefers the shortest conductive route).
pub const MAX_CONDUCTIVITY: u16 = 250;

/// Conductivity of a block given local `moisture` (0..=255, from the humidity
/// field). Water and metal conduct strongly; dry insulators (wood) barely;
/// stone/soil conduct more when damp. Air is low but moist air conducts a bit.
pub fn conductivity(block: BlockId, moisture: u8) -> u16 {
    use BlockId::*;
    let base: u16 = match block {
        Water | Ice => 250,
        IronBlock | GoldBlock => 220,
        IronOre | GoldOre | CopperOre => 190,
        Snow => 70, // damp
        Air => 8,
        OakLog | OakPlanks | Bookshelf => 3, // dry wood insulates
        Dirt | Grass | Sand | Gravel | Clay => 25,
        _ if rock_physics(block).is_some() => 30, // bare rock, a little
        _ => 12,
    };
    // A wet surface conducts: moisture lifts everything but the already-high
    // metals/water (which are saturated).
    let damp_bonus = if base < 150 { (moisture as u16 * 60) / 255 } else { 0 };
    (base + damp_bonus).min(MAX_CONDUCTIVITY)
}

const NEIGHBORS6: [[i32; 3]; 6] = [
    [1, 0, 0], [-1, 0, 0], [0, 1, 0], [0, -1, 0], [0, 0, 1], [0, 0, -1],
];

/// Cost to enter a cell of the given conductivity: the inverse, always ≥ 1.
#[inline]
fn step_cost(cond: u16) -> u64 {
    (MAX_CONDUCTIVITY - cond.min(MAX_CONDUCTIVITY)) as u64 + 1
}

/// The least-resistance bolt channel from `start` to `target`, or `None` if the
/// target isn't reached within `max_nodes` explored. `cond_at(cell)` gives a
/// cell's conductivity (caller folds in block + moisture). Dijkstra over the
/// 6-neighbourhood; integer costs + coordinate tie-break = deterministic.
pub fn arc_path(
    start: [i32; 3],
    target: [i32; 3],
    cond_at: impl Fn([i32; 3]) -> u16,
    max_nodes: u32,
) -> Option<Vec<[i32; 3]>> {
    use std::cmp::Reverse;
    use std::collections::{BinaryHeap, HashMap};

    if start == target {
        return Some(vec![start]);
    }
    let mut dist: HashMap<[i32; 3], u64> = HashMap::new();
    let mut prev: HashMap<[i32; 3], [i32; 3]> = HashMap::new();
    let mut heap: BinaryHeap<Reverse<(u64, [i32; 3])>> = BinaryHeap::new();
    dist.insert(start, 0);
    heap.push(Reverse((0, start)));
    let mut explored = 0u32;

    while let Some(Reverse((cost, node))) = heap.pop() {
        if node == target {
            // Reconstruct the channel.
            let mut path = vec![target];
            let mut cur = target;
            while let Some(&p) = prev.get(&cur) {
                path.push(p);
                cur = p;
            }
            path.reverse();
            return Some(path);
        }
        if cost > *dist.get(&node).unwrap_or(&u64::MAX) {
            continue; // stale heap entry
        }
        explored += 1;
        if explored > max_nodes {
            break;
        }
        for d in NEIGHBORS6 {
            let nb = [node[0] + d[0], node[1] + d[1], node[2] + d[2]];
            let nd = cost + step_cost(cond_at(nb));
            if nd < *dist.get(&nb).unwrap_or(&u64::MAX) {
                dist.insert(nb, nd);
                prev.insert(nb, node);
                heap.push(Reverse((nd, nb)));
            }
        }
    }
    None
}

/// A branching bolt: from one `source`, the least-resistance channels to several
/// `targets` at once (up to `max_targets`, nearest first). A real multi-target
/// strike doesn't fire independent bolts — it grows one channel that *forks*.
/// We get that for free from a single cost field: one Dijkstra from the source
/// builds the shortest-conductive-path tree, and each target's channel is traced
/// back through it. Branches therefore **share a trunk** near the source and
/// **fan out through conductor clusters** (the field prefers water/metal) before
/// **collapsing onto each target** — exactly the revised conduction model.
///
/// Returns one polyline per reached target (`source … target`); unreached targets
/// are omitted. Deterministic (integer costs + coordinate tie-break), so it rolls
/// back with the sim.
pub fn arc_tree(
    source: [i32; 3],
    targets: &[[i32; 3]],
    cond_at: impl Fn([i32; 3]) -> u16,
    max_nodes: u32,
    max_targets: usize,
) -> Vec<Vec<[i32; 3]>> {
    use std::cmp::Reverse;
    use std::collections::{BinaryHeap, HashMap, HashSet};

    let want: HashSet<[i32; 3]> = targets.iter().copied().collect();
    let mut dist: HashMap<[i32; 3], u64> = HashMap::new();
    let mut prev: HashMap<[i32; 3], [i32; 3]> = HashMap::new();
    let mut heap: BinaryHeap<Reverse<(u64, [i32; 3])>> = BinaryHeap::new();
    dist.insert(source, 0);
    heap.push(Reverse((0, source)));
    let mut explored = 0u32;
    // (cost, cell) for each target as it's finalised — nearest first after sort.
    let mut found: Vec<(u64, [i32; 3])> = Vec::new();

    while let Some(Reverse((cost, node))) = heap.pop() {
        if cost > *dist.get(&node).unwrap_or(&u64::MAX) {
            continue; // stale heap entry
        }
        if want.contains(&node) && !found.iter().any(|(_, c)| *c == node) {
            found.push((cost, node));
            if found.len() == want.len() {
                break; // every target's channel is settled
            }
        }
        explored += 1;
        if explored > max_nodes {
            break;
        }
        for d in NEIGHBORS6 {
            let nb = [node[0] + d[0], node[1] + d[1], node[2] + d[2]];
            let nd = cost + step_cost(cond_at(nb));
            if nd < *dist.get(&nb).unwrap_or(&u64::MAX) {
                dist.insert(nb, nd);
                prev.insert(nb, node);
                heap.push(Reverse((nd, nb)));
            }
        }
    }

    // Nearest first, deterministic tie-break on coordinate; keep up to the cap.
    found.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    found.truncate(max_targets);

    // Trace each target back through the shared shortest-path tree.
    found
        .iter()
        .map(|&(_, t)| {
            let mut branch = vec![t];
            let mut cur = t;
            while let Some(&p) = prev.get(&cur) {
                branch.push(p);
                cur = p;
            }
            branch.reverse();
            branch
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::blocks::BlockId;

    #[test]
    fn conductivity_ranks_water_and_metal_over_dry_air() {
        let dry_air = conductivity(BlockId::Air, 0);
        let moist_air = conductivity(BlockId::Air, 255);
        assert!(moist_air > dry_air, "humid air conducts more");
        assert!(conductivity(BlockId::Water, 0) > conductivity(BlockId::Stone, 0));
        assert!(conductivity(BlockId::IronBlock, 0) > conductivity(BlockId::Stone, 255));
        assert!(conductivity(BlockId::OakLog, 0) < dry_air, "dry wood insulates");
        // Damp stone conducts more than bone-dry stone.
        assert!(conductivity(BlockId::Stone, 255) > conductivity(BlockId::Stone, 0));
    }

    #[test]
    fn bolt_bends_through_a_water_channel() {
        // Strike from (0,0,1) to (6,0,1). The direct row z=1 is dry air; a water
        // channel runs along z=0. The least-resistance path should detour into
        // the water even though it's more cells.
        let cond = |c: [i32; 3]| -> u16 {
            if c[1] == 0 && c[2] == 0 && (0..=6).contains(&c[0]) {
                conductivity(BlockId::Water, 0) // the channel
            } else {
                conductivity(BlockId::Air, 0) // dry surroundings
            }
        };
        let path = arc_path([0, 0, 1], [6, 0, 1], cond, 10_000).expect("reaches target");
        assert_eq!(path.first(), Some(&[0, 0, 1]));
        assert_eq!(path.last(), Some(&[6, 0, 1]));
        assert!(path.iter().any(|c| c[2] == 0), "the arc dips into the water channel: {path:?}");
    }

    #[test]
    fn straight_through_uniform_air_takes_the_direct_line() {
        // No conductor to bend toward → shortest path is the straight line.
        let cond = |_c: [i32; 3]| conductivity(BlockId::Air, 0);
        let path = arc_path([0, 0, 0], [4, 0, 0], cond, 10_000).expect("reaches");
        assert_eq!(path.len(), 5, "5 cells in a straight 4-step run");
        assert!(path.iter().all(|c| c[1] == 0 && c[2] == 0), "no needless detour: {path:?}");
    }

    #[test]
    fn unreachable_within_budget_returns_none() {
        let cond = |_c: [i32; 3]| conductivity(BlockId::Air, 0);
        // Far target, tiny exploration budget → can't get there.
        assert!(arc_path([0, 0, 0], [50, 0, 0], cond, 20).is_none());
    }

    #[test]
    fn forked_bolt_reaches_each_target_and_shares_a_trunk() {
        // Two targets in the same direction → the nearer channel is the trunk
        // and the farther one extends it (shared prefix), not a separate bolt.
        let cond = |_c: [i32; 3]| conductivity(BlockId::Air, 0);
        let branches = arc_tree([0, 0, 0], &[[5, 0, 0], [7, 0, 0]], cond, 10_000, 6);
        assert_eq!(branches.len(), 2);
        assert_eq!(branches[0].first(), Some(&[0, 0, 0]));
        assert_eq!(branches[0].last(), Some(&[5, 0, 0]), "nearest target first");
        assert_eq!(branches[1].last(), Some(&[7, 0, 0]));
        assert!(branches[1].starts_with(&branches[0]), "the farther fork extends the trunk: {branches:?}");
    }

    #[test]
    fn branches_fork_to_targets_in_different_directions() {
        let cond = |_c: [i32; 3]| conductivity(BlockId::Air, 0);
        let branches = arc_tree([0, 0, 0], &[[4, 0, 0], [0, 4, 0]], cond, 10_000, 6);
        assert_eq!(branches.len(), 2);
        assert!(branches.iter().all(|b| b.first() == Some(&[0, 0, 0])), "all rooted at the source");
        let ends: Vec<[i32; 3]> = branches.iter().map(|b| *b.last().unwrap()).collect();
        assert!(ends.contains(&[4, 0, 0]) && ends.contains(&[0, 4, 0]), "both targets struck: {ends:?}");
    }

    #[test]
    fn max_targets_caps_the_forks_to_the_nearest() {
        let cond = |_c: [i32; 3]| conductivity(BlockId::Air, 0);
        let branches = arc_tree([0, 0, 0], &[[2, 0, 0], [4, 0, 0], [6, 0, 0]], cond, 10_000, 2);
        assert_eq!(branches.len(), 2, "capped at two");
        let ends: Vec<[i32; 3]> = branches.iter().map(|b| *b.last().unwrap()).collect();
        assert!(ends.contains(&[2, 0, 0]) && ends.contains(&[4, 0, 0]), "the two nearest");
        assert!(!ends.contains(&[6, 0, 0]), "the far one is dropped");
    }

    #[test]
    fn a_fork_bends_through_a_conductor() {
        let cond = |c: [i32; 3]| -> u16 {
            if c[1] == 0 && c[2] == 0 && (0..=6).contains(&c[0]) {
                conductivity(BlockId::Water, 0)
            } else {
                conductivity(BlockId::Air, 0)
            }
        };
        let branches = arc_tree([0, 0, 1], &[[6, 0, 1]], cond, 10_000, 6);
        assert_eq!(branches.len(), 1);
        assert!(branches[0].iter().any(|c| c[2] == 0), "the fork dips into the water channel: {branches:?}");
    }
}
