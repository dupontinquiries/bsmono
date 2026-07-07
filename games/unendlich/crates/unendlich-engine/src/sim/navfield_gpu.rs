//! Kernel: **NavField distance field** via Jacobi relaxation.
//!
//! Computes, for every passable cell of a 3D navigation grid, the unit-step
//! shortest-path distance to the nearest seed (goal) cell, propagated only
//! through passable cells with 6-neighbour (von Neumann) connectivity. The
//! result is a flow/distance field an agent can descend to reach the nearest
//! goal — the GPU-friendly alternative to per-agent A*.
//!
//! Written in GPU-ready form for the [`crate::sim::gpu_bench`] harness: one
//! logical thread per cell, ping-pong (double-buffered) reads/writes, no
//! host-side branching inside the sweep. The CPU reference here is the
//! correctness gate the compute shader must match bit-for-bit, and the thing the
//! benchmark times.
//!
//! ## GPU DISPATCH CONTRACT
//!   bindings:
//!     0: grid   SSBO  — [u32 blocked; nx*ny*nz]  (1 = impassable, 0 = passable)
//!     1: src    SSBO  — [u32 dist;    nx*ny*nz]  (read this sweep)
//!     2: dst    SSBO  — [u32 dist;    nx*ny*nz]  (written this sweep)
//!     3: flags        — [u32 changed] (atomicOr 1 when any cell changes)
//!   uniforms: nx, ny, nz (u32)
//!   dispatch: one invocation per cell c=(x,y,z); thread writes dst[idx(c)] only.
//!     dst[c] = blocked ? WALL
//!                      : min(src[c], 1 + min over in-bounds 6-neighbours n of
//!                                         src[n] where src[n] != WALL)
//!     host loop: clear `changed`; dispatch; if `changed`==0 stop; else swap
//!     src/dst and repeat (≤ max_iters). Seeds are passable cells preloaded to 0
//!     in the initial src buffer and stay 0 (0 is the min). Integer-only so GPU
//!     output equals [`navfield_cpu`] exactly — that equality gates the timings.

use crate::sim::gpu_bench::{bench, BenchResult};

/// Sentinel for an impassable or not-yet-reached cell (also the max distance).
/// Reachable distances must stay below this, so the grid's reachable diameter is
/// effectively capped at `u16::MAX - 1` steps (ample for any nav grid).
pub const WALL: u16 = u16::MAX;

/// A flat 3D navigation grid. Storage is a single row-major Vec (x fastest, then
/// y, then z) — the SoA layout the GPU kernel binds directly.
#[derive(Clone, Debug)]
pub struct NavGrid {
    pub nx: usize,
    pub ny: usize,
    pub nz: usize,
    /// `true` = impassable. Length must be `nx*ny*nz`.
    pub blocked: Vec<bool>,
}

impl NavGrid {
    /// Open (no walls) grid of the given dimensions.
    pub fn open(nx: usize, ny: usize, nz: usize) -> Self {
        NavGrid { nx, ny, nz, blocked: vec![false; nx * ny * nz] }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.nx * self.ny * self.nz
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Row-major flat index for `(x,y,z)`. Caller ensures in-bounds.
    #[inline]
    pub fn idx(&self, x: usize, y: usize, z: usize) -> usize {
        (z * self.ny + y) * self.nx + x
    }
}

/// One Jacobi relaxation sweep: `dst[c] = blocked ? WALL : min(src[c], 1 + min
/// passable-neighbour)`. Reads only `src`, writes only `dst` (double-buffered, so
/// it's order-independent and matches the GPU). Returns whether any cell changed
/// (host convergence flag). `dst` is fully overwritten; its prior contents are
/// irrelevant.
pub fn relax_sweep(grid: &NavGrid, src: &[u16], dst: &mut [u16]) -> bool {
    debug_assert_eq!(src.len(), grid.len());
    debug_assert_eq!(dst.len(), grid.len());
    let (nx, ny, nz) = (grid.nx, grid.ny, grid.nz);
    let mut changed = false;
    for z in 0..nz {
        for y in 0..ny {
            for x in 0..nx {
                let c = grid.idx(x, y, z);
                if grid.blocked[c] {
                    dst[c] = WALL;
                    continue;
                }
                let mut best = src[c];
                // 6-neighbour von Neumann; each guarded for in-bounds.
                if x > 0 {
                    best = relax_from(best, src[grid.idx(x - 1, y, z)]);
                }
                if x + 1 < nx {
                    best = relax_from(best, src[grid.idx(x + 1, y, z)]);
                }
                if y > 0 {
                    best = relax_from(best, src[grid.idx(x, y - 1, z)]);
                }
                if y + 1 < ny {
                    best = relax_from(best, src[grid.idx(x, y + 1, z)]);
                }
                if z > 0 {
                    best = relax_from(best, src[grid.idx(x, y, z - 1)]);
                }
                if z + 1 < nz {
                    best = relax_from(best, src[grid.idx(x, y, z + 1)]);
                }
                if best != src[c] {
                    changed = true;
                }
                dst[c] = best;
            }
        }
    }
    changed
}

/// Relax `best` against a neighbour distance: if the neighbour is reached,
/// `1 + neighbour` is a candidate. `WALL`-guarded so the `+1` never overflows
/// (a reached neighbour is `<= WALL - 1`).
#[inline]
fn relax_from(best: u16, neighbour: u16) -> u16 {
    if neighbour != WALL {
        let cand = neighbour + 1;
        if cand < best {
            return cand;
        }
    }
    best
}

/// Full distance field: seeds carry distance 0; relax until converged (a sweep
/// changes nothing) or `max_iters` sweeps elapse. Out-of-range or blocked seeds
/// are ignored. The returned field has length `grid.len()`; unreachable and
/// blocked cells are [`WALL`].
pub fn navfield_cpu(grid: &NavGrid, seeds: &[usize], max_iters: usize) -> Vec<u16> {
    let n = grid.len();
    let mut cur = vec![WALL; n];
    for &s in seeds {
        if s < n && !grid.blocked[s] {
            cur[s] = 0;
        }
    }
    let mut nxt = vec![WALL; n];
    for _ in 0..max_iters {
        let changed = relax_sweep(grid, &cur, &mut nxt);
        std::mem::swap(&mut cur, &mut nxt);
        if !changed {
            break;
        }
    }
    cur
}

/// Benchmark the NavField kernel over `iters` runs (each a full relaxation to
/// convergence or `max_iters`). Feed the result to
/// [`BenchResult::metric_line`] with `name = "navfield"`.
pub fn bench_navfield(grid: &NavGrid, seeds: &[usize], max_iters: usize, iters: usize) -> BenchResult {
    bench(iters, || {
        let field = navfield_cpu(grid, seeds, max_iters);
        std::hint::black_box(&field);
    })
}

// ── Companion kernel: flow field ─────────────────────────────────────────────
//
// Given a converged distance field, the flow field stores per cell the direction
// code of the neighbour with the smallest distance — the next step an agent
// takes toward the nearest seed. Pure per-cell map (no iteration): one GPU thread
// reads its 6 neighbours from the distance SSBO and writes one byte. Following
// the codes from any reachable cell strictly decreases distance by 1 and ends at
// a seed.
//
// GPU DISPATCH CONTRACT
//   bindings: 0: grid blocked SSBO, 1: field SSBO (from navfield), 2: out [u8; N]
//   uniforms: nx, ny, nz
//   dispatch: one invocation per cell; out[c] = lowest-code argmin neighbour, or
//             FLOW_NONE for seed/wall/unreached. Integer-only ⇒ equals
//             [`flow_field_cpu`] exactly.

/// Flow direction codes. `FLOW_NONE` = a seed (arrived), a wall, or unreachable.
pub const FLOW_NONE: u8 = 0;
pub const FLOW_NEG_X: u8 = 1;
pub const FLOW_POS_X: u8 = 2;
pub const FLOW_NEG_Y: u8 = 3;
pub const FLOW_POS_Y: u8 = 4;
pub const FLOW_NEG_Z: u8 = 5;
pub const FLOW_POS_Z: u8 = 6;

/// Decode a flow code into a unit step `(dx, dy, dz)`; `FLOW_NONE` → `(0,0,0)`.
pub fn flow_step(code: u8) -> (i32, i32, i32) {
    match code {
        FLOW_NEG_X => (-1, 0, 0),
        FLOW_POS_X => (1, 0, 0),
        FLOW_NEG_Y => (0, -1, 0),
        FLOW_POS_Y => (0, 1, 0),
        FLOW_NEG_Z => (0, 0, -1),
        FLOW_POS_Z => (0, 0, 1),
        _ => (0, 0, 0),
    }
}

/// Relax a candidate neighbour into the running best (strictly-smaller wins;
/// probing in code order so ties resolve to the lowest direction code).
#[inline]
fn consider(neighbour: u16, dir: u8, best: &mut u16, code: &mut u8) {
    if neighbour != WALL && neighbour < *best {
        *best = neighbour;
        *code = dir;
    }
}

/// Build the flow field from a distance `field` (from [`navfield_cpu`] on the
/// same `grid`). Each passable, reached, non-seed cell points to the in-bounds
/// neighbour with the strictly-smallest distance; ties resolve to the lowest
/// direction code. Length is `grid.len()`.
pub fn flow_field_cpu(grid: &NavGrid, field: &[u16]) -> Vec<u8> {
    debug_assert_eq!(field.len(), grid.len());
    let (nx, ny, nz) = (grid.nx, grid.ny, grid.nz);
    let mut out = vec![FLOW_NONE; grid.len()];
    for z in 0..nz {
        for y in 0..ny {
            for x in 0..nx {
                let c = grid.idx(x, y, z);
                let here = field[c];
                if grid.blocked[c] || here == WALL || here == 0 {
                    continue; // seed / wall / unreached → FLOW_NONE
                }
                let mut best = here;
                let mut code = FLOW_NONE;
                if x > 0 {
                    consider(field[grid.idx(x - 1, y, z)], FLOW_NEG_X, &mut best, &mut code);
                }
                if x + 1 < nx {
                    consider(field[grid.idx(x + 1, y, z)], FLOW_POS_X, &mut best, &mut code);
                }
                if y > 0 {
                    consider(field[grid.idx(x, y - 1, z)], FLOW_NEG_Y, &mut best, &mut code);
                }
                if y + 1 < ny {
                    consider(field[grid.idx(x, y + 1, z)], FLOW_POS_Y, &mut best, &mut code);
                }
                if z > 0 {
                    consider(field[grid.idx(x, y, z - 1)], FLOW_NEG_Z, &mut best, &mut code);
                }
                if z + 1 < nz {
                    consider(field[grid.idx(x, y, z + 1)], FLOW_POS_Z, &mut best, &mut code);
                }
                out[c] = code;
            }
        }
    }
    out
}

/// Benchmark the flow-field kernel over `iters` runs. The distance field is
/// computed once up front; the per-cell flow map is the thing timed.
pub fn bench_flow(grid: &NavGrid, seeds: &[usize], max_iters: usize, iters: usize) -> BenchResult {
    let field = navfield_cpu(grid, seeds, max_iters);
    bench(iters, || {
        let f = flow_field_cpu(grid, &field);
        std::hint::black_box(&f);
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    /// Independent BFS ground truth (different implementation than the kernel).
    fn bfs(grid: &NavGrid, seeds: &[usize]) -> Vec<u16> {
        let n = grid.len();
        let mut dist = vec![WALL; n];
        let mut q = VecDeque::new();
        for &s in seeds {
            if s < n && !grid.blocked[s] && dist[s] == WALL {
                dist[s] = 0;
                q.push_back(s);
            }
        }
        let (nx, ny, nz) = (grid.nx, grid.ny, grid.nz);
        while let Some(c) = q.pop_front() {
            let z = c / (nx * ny);
            let y = (c / nx) % ny;
            let x = c % nx;
            let d = dist[c];
            let mut nbrs: Vec<usize> = Vec::with_capacity(6);
            if x > 0 {
                nbrs.push(grid.idx(x - 1, y, z));
            }
            if x + 1 < nx {
                nbrs.push(grid.idx(x + 1, y, z));
            }
            if y > 0 {
                nbrs.push(grid.idx(x, y - 1, z));
            }
            if y + 1 < ny {
                nbrs.push(grid.idx(x, y + 1, z));
            }
            if z > 0 {
                nbrs.push(grid.idx(x, y, z - 1));
            }
            if z + 1 < nz {
                nbrs.push(grid.idx(x, y, z + 1));
            }
            for cc in nbrs {
                if !grid.blocked[cc] && dist[cc] == WALL {
                    dist[cc] = d + 1;
                    q.push_back(cc);
                }
            }
        }
        dist
    }

    #[test]
    fn straight_line_distances() {
        let grid = NavGrid::open(5, 1, 1);
        let f = navfield_cpu(&grid, &[0], grid.len() + 2);
        assert_eq!(f, vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn multi_seed_takes_nearest() {
        let grid = NavGrid::open(5, 1, 1);
        let f = navfield_cpu(&grid, &[0, 4], grid.len() + 2);
        // distance to nearer of the two ends
        assert_eq!(f, vec![0, 1, 2, 1, 0]);
    }

    #[test]
    fn wall_forces_detour() {
        // 3x3 plane; a wall column at x=1, y=0 makes the top row route around.
        let mut grid = NavGrid::open(3, 3, 1);
        let block_idx = grid.idx(1, 0, 0);
        grid.blocked[block_idx] = true; // block (1,0)
        let seed = grid.idx(0, 0, 0);
        let f = navfield_cpu(&grid, &[seed], grid.len() + 2);
        // (2,0) can't go straight (1,0 blocked) → must detour via (0,1)->(1,1)->(2,1)->(2,0): 4 steps
        assert_eq!(f[grid.idx(2, 0, 0)], 4);
        assert_eq!(f[grid.idx(1, 0, 0)], WALL); // the wall
    }

    #[test]
    fn unreachable_pocket_stays_wall() {
        // Fully wall off cell (2,0) in a 3x1 line: seed at 0, wall at 1 → 2 unreachable.
        let mut grid = NavGrid::open(3, 1, 1);
        grid.blocked[1] = true;
        let f = navfield_cpu(&grid, &[0], grid.len() + 2);
        assert_eq!(f[0], 0);
        assert_eq!(f[1], WALL);
        assert_eq!(f[2], WALL);
    }

    #[test]
    fn matches_bfs_on_structured_grids() {
        // Deterministic pseudo-random grids (splitmix-ish) cross-checked vs BFS.
        let mut state: u64 = 0x9E3779B97F4A7C15;
        let mut next = || {
            state = state.wrapping_add(0x9E3779B97F4A7C15);
            let mut z = state;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
            z ^ (z >> 31)
        };
        for _ in 0..120 {
            let nx = 2 + (next() % 7) as usize;
            let ny = 2 + (next() % 7) as usize;
            let nz = 1 + (next() % 3) as usize;
            let n = nx * ny * nz;
            let blocked: Vec<bool> = (0..n).map(|_| (next() % 100) < 25).collect();
            let grid = NavGrid { nx, ny, nz, blocked };
            let mut seeds = Vec::new();
            for _ in 0..(1 + next() % 3) {
                let s = (next() as usize) % n;
                if !grid.blocked[s] {
                    seeds.push(s);
                }
            }
            if seeds.is_empty() {
                continue;
            }
            let got = navfield_cpu(&grid, &seeds, n + 2);
            let exp = bfs(&grid, &seeds);
            assert_eq!(got, exp, "navfield != bfs for {nx}x{ny}x{nz}");
        }
    }

    #[test]
    fn bench_runs_and_formats() {
        let grid = NavGrid::open(8, 8, 1);
        let br = bench_navfield(&grid, &[0], grid.len() + 2, 3);
        assert_eq!(br.samples.len(), 3);
        let line = br.metric_line("navfield", Some(1.0));
        assert!(line.contains("navfield n=3"), "{line}");
    }

    #[test]
    fn flow_points_downhill_straight_line() {
        let grid = NavGrid::open(5, 1, 1);
        let field = navfield_cpu(&grid, &[0], grid.len() + 2);
        let flow = flow_field_cpu(&grid, &field);
        assert_eq!(flow[0], FLOW_NONE); // seed
        assert_eq!(flow[1], FLOW_NEG_X);
        assert_eq!(flow[4], FLOW_NEG_X);
    }

    #[test]
    fn flow_none_on_wall_and_unreached() {
        let mut grid = NavGrid::open(3, 1, 1);
        grid.blocked[1] = true;
        let field = navfield_cpu(&grid, &[0], grid.len() + 2);
        let flow = flow_field_cpu(&grid, &field);
        assert_eq!(flow[0], FLOW_NONE); // seed
        assert_eq!(flow[1], FLOW_NONE); // wall
        assert_eq!(flow[2], FLOW_NONE); // unreached pocket
    }

    #[test]
    fn following_flow_reaches_seed() {
        // 4x4 plane, seed at (0,0): descend the flow from the far corner.
        let grid = NavGrid::open(4, 4, 1);
        let seed = grid.idx(0, 0, 0);
        let field = navfield_cpu(&grid, &[seed], grid.len() + 2);
        let flow = flow_field_cpu(&grid, &field);
        let (mut x, mut y, mut z) = (3i32, 3i32, 0i32);
        let mut steps = 0usize;
        while field[grid.idx(x as usize, y as usize, z as usize)] != 0 {
            let code = flow[grid.idx(x as usize, y as usize, z as usize)];
            assert_ne!(code, FLOW_NONE, "stuck before reaching seed");
            let d0 = field[grid.idx(x as usize, y as usize, z as usize)];
            let (dx, dy, dz) = flow_step(code);
            x += dx;
            y += dy;
            z += dz;
            assert_eq!(field[grid.idx(x as usize, y as usize, z as usize)], d0 - 1);
            steps += 1;
            assert!(steps <= grid.len(), "flow cycle detected");
        }
        assert_eq!((x, y, z), (0, 0, 0));
    }
}
