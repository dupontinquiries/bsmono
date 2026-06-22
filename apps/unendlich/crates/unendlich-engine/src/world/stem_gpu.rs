//! GPU-ready parallel stem growth — the benchmarkable form of
//! [`crate::world::stem`]'s nodal growth.
//!
//! Serial growth reserves cells as it walks its tips, which doesn't parallelize.
//! This is the parallel reformulation, run as two passes:
//!
//! - **Pass 1 (parallel, one thread per active tip).** Each tip reads a
//!   read-only snapshot — the node-cell set, the occupancy/moisture/temperature
//!   fields — searches its forward hemisphere, scores candidates exactly as
//!   [`crate::world::stem`] does, and emits a single *claim* (its best open cell,
//!   plus an optional branch second). This is the heavy, embarrassingly-parallel
//!   work the GPU accelerates.
//! - **Pass 2 (arbitration).** Cells claimed by several tips go to the
//!   lowest-index tip; losers simply wait and retry next step. Because the
//!   winner is the min index (not the execution order), the result is identical
//!   no matter what order threads ran — validated.
//!
//! This is its *own* deterministic algorithm with its own reference (it differs
//! from the serial `stem` step, which lets a blocked tip fall back same-step).
//! Both are valid; this one parallelizes. The GPU shader reimplements Pass 1 and
//! must match [`grow_step_parallel`] bit-for-bit — that's the correctness gate.
//!
//! ## GPU DISPATCH CONTRACT
//! ```text
//! Pass 1 — claim kernel (one invocation per active tip t):
//!   in:  nodes SSBO [{cell:i32×3, dir:i8×3, energy:u16, active:u8}]
//!        occupancy (3D tex / SSBO), moisture + temperature field samplers
//!        uniforms: seed:u64, step:u32
//!   out: claims SSBO [{tip:u32, cell:i32×3, dir:i8×3, energy:u16,
//!                      second_cell:i32×3, has_second:u8}]  (or a "dormant" flag)
//!   each thread: gate on moisture/temperature; scan the 26-neighbourhood in the
//!     forward hemisphere (dot(dir,d) >= 0); skip occupied / existing-node cells;
//!     score = W_CONT·dot + W_UP·dy + W_MOIST·moisture(c) + W_JIT·hash01(c);
//!     pick argmax (tie: lower cell); branch if wet + hash gate.
//! Pass 2 — arbitration (atomicMin of tip index per claimed cell into an owner
//!   buffer keyed by a spatial hash), then a compaction kernel appends the nodes
//!   whose claimant is the owner. Lowest-index-wins == determinism.
//! ```

use crate::sim::gpu_bench::{bench, BenchResult};
use crate::world::stem::{Cell, StemGraph, StemNode};
use std::collections::{HashMap, HashSet};

// Scoring mirrors `world::stem` — keep in sync (the GPU shader reimplements it).
const MOISTURE_MIN: f32 = 0.30;
const TEMP_MIN: f32 = 0.20;
const TEMP_MAX: f32 = 0.85;
const BRANCH_MOISTURE: f32 = 0.70;
const W_CONT: f32 = 1.0;
const W_UP: f32 = 0.6;
const W_MOISTURE: f32 = 0.8;
const W_JITTER: f32 = 0.5;
const BRANCH_PROB: f32 = 0.35;
const MAX_NODES: usize = 4096;

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

struct Claim {
    tip: usize,
    cell: Cell,
    dir: (i8, i8, i8),
    energy: u16,
    second: Option<(Cell, (i8, i8, i8))>,
}

fn new_node(cell: Cell, parent: usize, dir: (i8, i8, i8), energy: u16) -> StemNode {
    StemNode { cell, parent: parent as i32, dir, age: 0, energy, active: true }
}

/// One parallel growth step over all active tips in `g`. See module docs.
pub fn grow_step_parallel<M, T, O>(g: &mut StemGraph, moisture: &M, temperature: &T, occupied: &O)
where
    M: Fn(Cell) -> f32,
    T: Fn(Cell) -> f32,
    O: Fn(Cell) -> bool,
{
    let node_cells: HashSet<Cell> = g.nodes.iter().map(|n| n.cell).collect();

    // Pass 1: independent per-tip claims (the parallel kernel).
    let mut claims: Vec<Claim> = Vec::new();
    let mut deactivate: Vec<usize> = Vec::new();
    for ti in 0..g.nodes.len() {
        let n = g.nodes[ti];
        if !n.active {
            continue;
        }
        let m = moisture(n.cell);
        let t = temperature(n.cell);
        if t < TEMP_MIN || t > TEMP_MAX || m < MOISTURE_MIN {
            continue; // dormant — stays active
        }
        if n.energy == 0 {
            deactivate.push(ti);
            continue;
        }
        let mut cands: Vec<(f32, Cell, (i8, i8, i8))> = Vec::new();
        for dx in -1..=1i8 {
            for dy in -1..=1i8 {
                for dz in -1..=1i8 {
                    if (dx, dy, dz) == (0, 0, 0) {
                        continue;
                    }
                    let dot = n.dir.0 as i32 * dx as i32 + n.dir.1 as i32 * dy as i32 + n.dir.2 as i32 * dz as i32;
                    if dot < 0 {
                        continue;
                    }
                    let c = (n.cell.0 + dx as i32, n.cell.1 + dy as i32, n.cell.2 + dz as i32);
                    if node_cells.contains(&c) || occupied(c) {
                        continue;
                    }
                    let score = W_CONT * dot as f32
                        + W_UP * dy as f32
                        + W_MOISTURE * moisture(c)
                        + W_JITTER * hash01(c, g.seed, g.step, 0);
                    cands.push((score, c, (dx, dy, dz)));
                }
            }
        }
        if cands.is_empty() {
            deactivate.push(ti);
            continue;
        }
        cands.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal).then(a.1.cmp(&b.1)));
        let branch = m >= BRANCH_MOISTURE && n.energy >= 4 && cands.len() >= 2 && hash01(n.cell, g.seed, g.step, 1) < BRANCH_PROB;
        let second = if branch { Some((cands[1].1, cands[1].2)) } else { None };
        claims.push(Claim { tip: ti, cell: cands[0].1, dir: cands[0].2, energy: n.energy, second });
    }

    // Pass 2: min-index owner per claimed cell (order-independent).
    let mut owner: HashMap<Cell, usize> = HashMap::new();
    for c in &claims {
        for cell in std::iter::once(c.cell).chain(c.second.map(|s| s.0)) {
            owner.entry(cell).and_modify(|e| {
                if c.tip < *e {
                    *e = c.tip;
                }
            }).or_insert(c.tip);
        }
    }

    let mut pending: Vec<StemNode> = Vec::new();
    for c in &claims {
        if owner.get(&c.cell) != Some(&c.tip) {
            continue; // lost the cell — wait and retry next step
        }
        if g.nodes.len() + pending.len() >= MAX_NODES {
            continue;
        }
        let win_second = c.second.filter(|s| owner.get(&s.0) == Some(&c.tip) && s.0 != c.cell);
        if let Some((scell, sdir)) = win_second {
            let ce = (c.energy - 1) / 2;
            pending.push(new_node(c.cell, c.tip, c.dir, ce));
            if g.nodes.len() + pending.len() < MAX_NODES {
                pending.push(new_node(scell, c.tip, sdir, ce));
            }
        } else {
            pending.push(new_node(c.cell, c.tip, c.dir, c.energy - 1));
        }
        deactivate.push(c.tip);
    }

    for ti in deactivate {
        g.nodes[ti].active = false;
    }
    g.nodes.extend(pending);
    for n in &mut g.nodes {
        n.age = n.age.saturating_add(1);
    }
    g.step = g.step.wrapping_add(1);
}

/// Benchmark `steps` of parallel growth from `base` over `iters` runs.
pub fn bench_growth<M, T, O>(
    base: &StemGraph,
    steps: u32,
    moisture: &M,
    temperature: &T,
    occupied: &O,
    iters: usize,
) -> BenchResult
where
    M: Fn(Cell) -> f32,
    T: Fn(Cell) -> f32,
    O: Fn(Cell) -> bool,
{
    bench(iters, || {
        let mut g = base.clone();
        for _ in 0..steps {
            grow_step_parallel(&mut g, moisture, temperature, occupied);
        }
        std::hint::black_box(&g);
    })
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
    fn roots(rs: &[(Cell, (i8, i8, i8))], seed: u64) -> StemGraph {
        let nodes = rs
            .iter()
            .map(|&(cell, dir)| StemNode { cell, parent: -1, dir, age: 0, energy: 20, active: true })
            .collect();
        StemGraph { nodes, seed, step: 0 }
    }

    #[test]
    fn gating() {
        let mut g = roots(&[((0, 0, 0), (0, 1, 0))], 1);
        for _ in 0..8 {
            grow_step_parallel(&mut g, &k(0.1), &k(0.5), &open);
        }
        assert_eq!(g.nodes.len(), 1, "dry: no growth");
        for _ in 0..8 {
            grow_step_parallel(&mut g, &k(0.8), &k(0.5), &open);
        }
        assert!(g.nodes.len() > 1, "wet: grows");
        let mut c = roots(&[((0, 0, 0), (0, 1, 0))], 1);
        for _ in 0..8 {
            grow_step_parallel(&mut c, &k(0.8), &k(0.1), &open);
        }
        assert_eq!(c.nodes.len(), 1, "cold: no growth");
    }

    #[test]
    fn forest_is_collision_free_and_connected() {
        let mut g = roots(&[((0, 0, 0), (0, 1, 0)), ((1, 0, 0), (0, 1, 0)), ((2, 0, 0), (0, 1, 0))], 1);
        for _ in 0..20 {
            grow_step_parallel(&mut g, &k(0.6), &k(0.5), &open);
        }
        let cells: Vec<Cell> = g.nodes.iter().map(|n| n.cell).collect();
        let uniq: HashSet<Cell> = cells.iter().copied().collect();
        assert_eq!(cells.len(), uniq.len(), "arbitration prevents shared cells");
        for n in &g.nodes {
            if n.parent >= 0 {
                let pc = g.nodes[n.parent as usize].cell;
                let cheb = (pc.0 - n.cell.0).abs().max((pc.1 - n.cell.1).abs()).max((pc.2 - n.cell.2).abs());
                assert_eq!(cheb, 1, "child adjacent to parent");
            }
        }
        assert!(g.nodes.len() > 3, "the forest grew");
    }

    #[test]
    fn deterministic() {
        let mut a = roots(&[((0, 0, 0), (0, 1, 0)), ((1, 0, 0), (0, 1, 0))], 9);
        let mut b = roots(&[((0, 0, 0), (0, 1, 0)), ((1, 0, 0), (0, 1, 0))], 9);
        for _ in 0..20 {
            grow_step_parallel(&mut a, &k(0.6), &k(0.5), &open);
            grow_step_parallel(&mut b, &k(0.6), &k(0.5), &open);
        }
        let ca: Vec<Cell> = a.nodes.iter().map(|n| n.cell).collect();
        let cb: Vec<Cell> = b.nodes.iter().map(|n| n.cell).collect();
        assert_eq!(ca, cb, "same seed → same graph");
    }

    #[test]
    fn contested_cell_lowest_index_wins() {
        // Both tips lean toward (1,1,0); the lower index wins, the other waits.
        let mut g = roots(&[((0, 0, 0), (1, 1, 0)), ((2, 0, 0), (-1, 1, 0))], 1);
        grow_step_parallel(&mut g, &k(0.6), &k(0.5), &open);
        let cells: HashSet<Cell> = g.nodes.iter().map(|n| n.cell).collect();
        assert_eq!(cells.len(), g.nodes.len(), "no shared cell after a contested step");
        assert_eq!(g.nodes.len(), 3, "exactly one new node from the contested pair");
        assert!(g.nodes.iter().any(|n| n.active && n.parent < 0), "the loser root stays active to retry");
    }
}
