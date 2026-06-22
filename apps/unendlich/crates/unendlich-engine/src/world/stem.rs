//! Stem nodes — the growing backbone of cave flora.
//!
//! A stem grows as a *connected node graph*. Each step every active tip searches
//! its forward neighbourhood for the best open cell — rewarding continuation,
//! upward tropism, and proximity to moisture — and expands into it, gated by the
//! local moisture and temperature fields (a dry or too-cold tip stays dormant
//! rather than dying, and resumes when conditions improve). The graph this
//! builds is the structure everything else hangs off:
//!
//! - **Lumen links.** [`StemGraph::edges`] are exactly the `(parent, child)`
//!   pairs `world::lumen::LumenField` walks, so a pearl grown on each node
//!   inherits the chain path for free.
//! - **The future rope graph.** Every node already carries a cell + parent link,
//!   so the same topology lifts straight into rope/vine nodes later.
//! - **Block placement.** [`StemGraph::cells`] is the set of stem blocks to
//!   write (bind to a `Stem` `BlockId` + `BlockInstanceState` on the engine side).
//!
//! Tips read a read-only snapshot and decide independently, so this CPU
//! reference maps directly onto a GPU growth kernel — one thread per tip,
//! lowest-index-wins on cell conflicts (matching this serial order). The
//! determinism hash mirrors `plant_lifecycle::sticky_walk_random`, so growth is
//! reproducible across hosts.

use std::collections::HashSet;

pub type Cell = (i32, i32, i32);

/// Below this local moisture a tip is dormant (waits, doesn't grow or die).
const MOISTURE_MIN: f32 = 0.30;
/// Growth happens only inside this normalized temperature window.
const TEMP_MIN: f32 = 0.20;
const TEMP_MAX: f32 = 0.85;
/// At or above this moisture a tip may fork into two.
const BRANCH_MOISTURE: f32 = 0.70;
/// Growth budget a root lineage starts with (bounds total size).
const START_ENERGY: u16 = 20;
/// Scoring weights: continue current direction, climb, seek moisture, vary.
const W_CONT: f32 = 1.0;
const W_UP: f32 = 0.6;
const W_MOISTURE: f32 = 0.8;
const W_JITTER: f32 = 0.5;
/// Probability a qualifying wet tip branches on a given step.
const BRANCH_PROB: f32 = 0.35;
/// Hard cap so a runaway field can't allocate without bound.
const MAX_NODES: usize = 4096;

/// One stem node — a cell in the grid and a link back toward the root.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StemNode {
    /// Voxel cell this node occupies.
    pub cell: Cell,
    /// Index of the parent node, or `-1` for the root.
    pub parent: i32,
    /// The step direction that grew this node (drives continuation tropism).
    pub dir: (i8, i8, i8),
    /// Saturating age in growth steps.
    pub age: u16,
    /// Remaining growth budget along this lineage.
    pub energy: u16,
    /// Whether this node is still a growing tip.
    pub active: bool,
}

impl StemNode {
    fn root(cell: Cell, dir: (i8, i8, i8)) -> Self {
        Self { cell, parent: -1, dir, age: 0, energy: START_ENERGY, active: true }
    }
}

/// A growing stem: its nodes plus the seed that makes growth reproducible.
#[derive(Clone, Debug)]
pub struct StemGraph {
    pub nodes: Vec<StemNode>,
    pub seed: u64,
    pub step: u32,
}

/// splitmix-style hash → `[0, 1)`, mirroring `plant_lifecycle::sticky_walk_random`
/// so stem growth is deterministic and identical across hosts.
fn hash01(cell: Cell, seed: u64, step: u32, salt: u64) -> f32 {
    let (x, y, z) = cell;
    let mut h: u64 = 0x9E37_79B9_7F4A_7C15;
    h ^= (x as i64 as u64).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= (y as i64 as u64).wrapping_mul(0x94D0_49BB_1331_11EB);
    h ^= (z as i64 as u64).wrapping_mul(0x2545_F491_4F6C_DD1D);
    h ^= (seed ^ (step as u64) ^ salt).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    h ^= h >> 33;
    h = h.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
    h ^= h >> 33;
    h = h.wrapping_mul(0xC4CE_B9FE_1A85_EC53);
    h ^= h >> 33;
    ((h >> 40) as f32) / (1u32 << 24) as f32
}

impl StemGraph {
    /// A new stem rooted at `cell`, initially growing along `dir` (e.g. `(0,1,0)`
    /// up, or a cave-wall normal). `seed` makes the organic variation reproducible.
    pub fn new(cell: Cell, dir: (i8, i8, i8), seed: u64) -> Self {
        Self { nodes: vec![StemNode::root(cell, dir)], seed, step: 0 }
    }

    /// `(parent, child)` index pairs — the stem's edges, ready to hand to
    /// `LumenField` as light-chain links (and the future rope graph).
    pub fn edges(&self) -> Vec<(u32, u32)> {
        self.nodes
            .iter()
            .enumerate()
            .filter_map(|(i, n)| if n.parent >= 0 { Some((n.parent as u32, i as u32)) } else { None })
            .collect()
    }

    /// Every node's cell — the set of stem blocks to place.
    pub fn cells(&self) -> Vec<Cell> {
        self.nodes.iter().map(|n| n.cell).collect()
    }

    /// Number of still-growing tips.
    pub fn tip_count(&self) -> usize {
        self.nodes.iter().filter(|n| n.active).count()
    }

    /// Advance growth by one step. `moisture`/`temperature` sample the local
    /// fields (normalized `[0, 1]`); `occupied` reports cells already filled by
    /// the world so stems grow into open space.
    pub fn grow_step<M, T, O>(&mut self, moisture: &M, temperature: &T, occupied: &O)
    where
        M: Fn(Cell) -> f32,
        T: Fn(Cell) -> f32,
        O: Fn(Cell) -> bool,
    {
        let mut reserved: HashSet<Cell> = self.nodes.iter().map(|n| n.cell).collect();
        let mut pending: Vec<StemNode> = Vec::new();
        let mut deactivate: Vec<usize> = Vec::new();

        for ti in 0..self.nodes.len() {
            let node = self.nodes[ti];
            if !node.active {
                continue;
            }
            if self.nodes.len() + pending.len() >= MAX_NODES {
                break;
            }
            let m = moisture(node.cell);
            let t = temperature(node.cell);
            if t < TEMP_MIN || t > TEMP_MAX || m < MOISTURE_MIN {
                continue; // dormant — tip waits for better conditions
            }
            if node.energy == 0 {
                deactivate.push(ti);
                continue;
            }

            // Search the forward hemisphere for open candidate cells.
            let mut cands: Vec<(f32, Cell, (i8, i8, i8))> = Vec::new();
            for dx in -1..=1i8 {
                for dy in -1..=1i8 {
                    for dz in -1..=1i8 {
                        if (dx, dy, dz) == (0, 0, 0) {
                            continue;
                        }
                        let dot = node.dir.0 as i32 * dx as i32
                            + node.dir.1 as i32 * dy as i32
                            + node.dir.2 as i32 * dz as i32;
                        if dot < 0 {
                            continue; // never reverse on the parent direction
                        }
                        let c = (
                            node.cell.0 + dx as i32,
                            node.cell.1 + dy as i32,
                            node.cell.2 + dz as i32,
                        );
                        if reserved.contains(&c) || occupied(c) {
                            continue;
                        }
                        let score = W_CONT * dot as f32
                            + W_UP * dy as f32
                            + W_MOISTURE * moisture(c)
                            + W_JITTER * hash01(c, self.seed, self.step, 0);
                        cands.push((score, c, (dx, dy, dz)));
                    }
                }
            }
            if cands.is_empty() {
                deactivate.push(ti); // blocked — matures in place
                continue;
            }
            cands.sort_by(|a, b| {
                b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal).then(a.1.cmp(&b.1))
            });

            let branch = m >= BRANCH_MOISTURE
                && node.energy >= 4
                && cands.len() >= 2
                && hash01(node.cell, self.seed, self.step, 1) < BRANCH_PROB;

            if branch {
                let child_energy = (node.energy - 1) / 2;
                for &(_, c, d) in cands.iter().take(2) {
                    reserved.insert(c);
                    pending.push(StemNode { cell: c, parent: ti as i32, dir: d, age: 0, energy: child_energy, active: true });
                }
            } else {
                let (_, c, d) = cands[0];
                reserved.insert(c);
                pending.push(StemNode { cell: c, parent: ti as i32, dir: d, age: 0, energy: node.energy - 1, active: true });
            }
            deactivate.push(ti);
        }

        for ti in deactivate {
            self.nodes[ti].active = false;
        }
        self.nodes.extend(pending);
        for n in &mut self.nodes {
            n.age = n.age.saturating_add(1);
        }
        self.step = self.step.wrapping_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(v: f32) -> impl Fn(Cell) -> f32 {
        move |_| v
    }
    fn open(_: Cell) -> bool {
        false
    }

    #[test]
    fn moisture_gates_growth() {
        let mut g = StemGraph::new((0, 0, 0), (0, 1, 0), 1);
        for _ in 0..8 {
            g.grow_step(&k(0.1), &k(0.5), &open);
        }
        assert_eq!(g.nodes.len(), 1, "a dry stem stays dormant");
        for _ in 0..8 {
            g.grow_step(&k(0.8), &k(0.5), &open);
        }
        assert!(g.nodes.len() > 1, "a wet stem grows");
    }

    #[test]
    fn temperature_gates_growth() {
        let mut g = StemGraph::new((0, 0, 0), (0, 1, 0), 1);
        for _ in 0..8 {
            g.grow_step(&k(0.8), &k(0.1), &open);
        }
        assert_eq!(g.nodes.len(), 1, "too cold to grow");
        for _ in 0..8 {
            g.grow_step(&k(0.8), &k(0.5), &open);
        }
        assert!(g.nodes.len() > 1, "in the temperature window it grows");
    }

    #[test]
    fn graph_is_connected_and_collision_free() {
        let mut g = StemGraph::new((0, 0, 0), (0, 1, 0), 1);
        for _ in 0..15 {
            g.grow_step(&k(0.55), &k(0.5), &open);
        }
        let cells = g.cells();
        let uniq: HashSet<Cell> = cells.iter().copied().collect();
        assert_eq!(cells.len(), uniq.len(), "no two nodes share a cell");

        for n in &g.nodes {
            if n.parent >= 0 {
                let pc = g.nodes[n.parent as usize].cell;
                let cheb = (pc.0 - n.cell.0).abs().max((pc.1 - n.cell.1).abs()).max((pc.2 - n.cell.2).abs());
                assert_eq!(cheb, 1, "every child is adjacent to its parent");
            }
        }

        let edges = g.edges();
        assert_eq!(edges.len(), g.nodes.len() - 1, "edges form a spanning tree");
        let mut seen: HashSet<u32> = HashSet::new();
        seen.insert(0);
        for _ in 0..g.nodes.len() {
            for &(p, c) in &edges {
                if seen.contains(&p) {
                    seen.insert(c);
                }
            }
        }
        assert_eq!(seen.len(), g.nodes.len(), "edges connect every node to the root");
        assert!(g.nodes.iter().map(|n| n.cell.1).max().unwrap() >= 3, "grows upward via tropism");
    }

    #[test]
    fn deterministic_for_a_seed() {
        let mut a = StemGraph::new((2, 0, -3), (0, 1, 0), 42);
        let mut b = StemGraph::new((2, 0, -3), (0, 1, 0), 42);
        for _ in 0..20 {
            a.grow_step(&k(0.6), &k(0.5), &open);
            b.grow_step(&k(0.6), &k(0.5), &open);
        }
        assert_eq!(a.cells(), b.cells(), "same seed → identical graph");
    }
}
