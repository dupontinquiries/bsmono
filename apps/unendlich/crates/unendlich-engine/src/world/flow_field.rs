//! `flow_field` — grid flow-field pathfinder (the GPU nav pre-map, validated
//! on the CPU first).
//!
//! Worker-03 / WORLD-PERF pure core, round 5. Multi-source breadth-first search
//! from goal cells over a walkability grid produces, for every cell, the step
//! cost to the nearest goal and a flow direction pointing one step closer. This
//! is exactly what a compute shader will run; building and property-testing it
//! pure here de-risks the GPU port and gives T01's `nav_query` a field to
//! sample.
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

use std::collections::VecDeque;

/// Sentinel cost for cells with no path to any goal.
pub const UNREACHABLE: u32 = u32::MAX;

/// Flow direction toward the nearest goal (the neighbor one step closer).
/// `None` for goal cells and unreachable cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    None,
    North,
    East,
    South,
    West,
}

/// Computed flow field over a `w × h` grid (row-major).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlowField {
    pub w: u32,
    pub h: u32,
    /// Step cost to the nearest goal; `UNREACHABLE` if none.
    pub cost: Vec<u32>,
    /// Direction one step toward the nearest goal.
    pub dir: Vec<Dir>,
}

impl FlowField {
    pub fn cost_at(&self, x: u32, y: u32) -> u32 {
        let i = (y * self.w + x) as usize;
        if i < self.cost.len() {
            self.cost[i]
        } else {
            UNREACHABLE
        }
    }

    pub fn reachable(&self, x: u32, y: u32) -> bool {
        self.cost_at(x, y) != UNREACHABLE
    }
}

/// Compute a flow field by 4-connected BFS from `goals` over `passable` cells.
///
/// Goal cells that are passable get cost 0 and `Dir::None`. Each other reachable
/// cell gets the shortest step count to a goal and a direction toward a neighbor
/// whose cost is exactly one lower. Impassable or unreachable cells get
/// `UNREACHABLE` / `Dir::None`. Deterministic.
pub fn compute(passable: &[bool], w: u32, h: u32, goals: &[(u32, u32)]) -> FlowField {
    let wu = w as usize;
    let hu = h as usize;
    let n = wu * hu;
    let mut cost = vec![UNREACHABLE; n];
    let mut dir = vec![Dir::None; n];
    if n == 0 || passable.len() < n {
        return FlowField { w, h, cost, dir };
    }

    let mut queue: VecDeque<usize> = VecDeque::new();
    for &(gx, gy) in goals {
        if gx < w && gy < h {
            let i = (gy * w + gx) as usize;
            if passable[i] && cost[i] != 0 {
                cost[i] = 0;
                queue.push_back(i);
            }
        }
    }

    // BFS over passable cells (4-connectivity). FIFO ⇒ first relaxation wins ⇒
    // deterministic costs.
    while let Some(c) = queue.pop_front() {
        let nc = cost[c] + 1;
        for nb in neighbors(c, wu, hu) {
            if passable[nb] && cost[nb] == UNREACHABLE {
                cost[nb] = nc;
                queue.push_back(nb);
            }
        }
    }

    // Assign each reachable non-goal cell a direction toward a strictly-lower
    // neighbor. Fixed probe order (N, E, S, W) ⇒ deterministic tiebreak.
    for c in 0..n {
        if cost[c] == UNREACHABLE || cost[c] == 0 {
            continue;
        }
        let x = c % wu;
        let y = c / wu;
        let want = cost[c] - 1;
        if y > 0 && cost[c - wu] == want {
            dir[c] = Dir::North;
        } else if x + 1 < wu && cost[c + 1] == want {
            dir[c] = Dir::East;
        } else if y + 1 < hu && cost[c + wu] == want {
            dir[c] = Dir::South;
        } else if x > 0 && cost[c - 1] == want {
            dir[c] = Dir::West;
        }
    }

    FlowField { w, h, cost, dir }
}

fn neighbors(c: usize, w: usize, h: usize) -> impl Iterator<Item = usize> {
    let x = c % w;
    let y = c / w;
    [
        if x > 0 { Some(c - 1) } else { None },
        if x + 1 < w { Some(c + 1) } else { None },
        if y > 0 { Some(c - w) } else { None },
        if y + 1 < h { Some(c + w) } else { None },
    ]
    .into_iter()
    .flatten()
}

/// The cell a `dir` points to from `(x, y)`, if any.
pub fn step_target(dir: Dir, x: u32, y: u32) -> Option<(u32, u32)> {
    match dir {
        Dir::North => y.checked_sub(1).map(|ny| (x, ny)),
        Dir::East => Some((x + 1, y)),
        Dir::South => Some((x, y + 1)),
        Dir::West => x.checked_sub(1).map(|nx| (nx, y)),
        Dir::None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn goal_cells_have_zero_cost() {
        let (w, h) = (5u32, 5u32);
        let passable = vec![true; 25];
        let ff = compute(&passable, w, h, &[(2, 2)]);
        assert_eq!(ff.cost_at(2, 2), 0);
        assert_eq!(ff.dir[(2 * w + 2) as usize], Dir::None);
        // Manhattan distance from the single goal.
        assert_eq!(ff.cost_at(0, 2), 2);
        assert_eq!(ff.cost_at(0, 0), 4);
    }

    #[test]
    fn prop_following_dir_strictly_decreases_cost_to_zero() {
        let (w, h) = (7u32, 6u32);
        // A wall column with a gap, to force non-trivial paths.
        let mut passable = vec![true; (w * h) as usize];
        for y in 0..h {
            if y != 3 {
                passable[(y * w + 3) as usize] = false; // wall at x=3 except y=3
            }
        }
        let ff = compute(&passable, w, h, &[(0, 0)]);
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) as usize;
                if !passable[i] || ff.cost[i] == UNREACHABLE || ff.cost[i] == 0 {
                    continue;
                }
                // The flow direction must point to a neighbor with cost-1.
                let t = step_target(ff.dir[i], x, y).expect("reachable non-goal has a dir");
                let ti = (t.1 * w + t.0) as usize;
                assert_eq!(ff.cost[ti], ff.cost[i] - 1, "dir not strictly downhill");
            }
        }
        // Walk from a far cell to the goal following dir; cost hits 0.
        let (mut x, mut y) = (6u32, 5u32);
        if ff.reachable(x, y) {
            let mut guard = 0;
            while ff.cost_at(x, y) != 0 {
                let t = step_target(ff.dir[(y * w + x) as usize], x, y).unwrap();
                x = t.0;
                y = t.1;
                guard += 1;
                assert!(guard < 1000, "walk did not converge");
            }
            assert_eq!(ff.cost_at(x, y), 0);
        }
    }

    #[test]
    fn unreachable_cells_flagged() {
        let (w, h) = (5u32, 1u32);
        // Wall at x=2 splits the row; goal on the left, x=3,4 unreachable.
        let mut passable = vec![true; 5];
        passable[2] = false;
        let ff = compute(&passable, w, h, &[(0, 0)]);
        assert_eq!(ff.cost_at(1, 0), 1);
        assert_eq!(ff.cost_at(2, 0), UNREACHABLE); // wall
        assert_eq!(ff.cost_at(3, 0), UNREACHABLE);
        assert_eq!(ff.cost_at(4, 0), UNREACHABLE);
        assert_eq!(ff.dir[3], Dir::None);
    }

    #[test]
    fn multi_goal_takes_nearest() {
        let (w, h) = (9u32, 1u32);
        let passable = vec![true; 9];
        let ff = compute(&passable, w, h, &[(0, 0), (8, 0)]);
        assert_eq!(ff.cost_at(1, 0), 1); // near left goal
        assert_eq!(ff.cost_at(7, 0), 1); // near right goal
        assert_eq!(ff.cost_at(4, 0), 4); // midpoint, equidistant
    }

    #[test]
    fn deterministic() {
        let (w, h) = (8u32, 8u32);
        let mut passable = vec![true; 64];
        passable[27] = false;
        passable[28] = false;
        let goals = [(0u32, 0u32), (7, 7)];
        let a = compute(&passable, w, h, &goals);
        let b = compute(&passable, w, h, &goals);
        assert_eq!(a, b);
    }

    #[test]
    fn impassable_goal_is_ignored() {
        let (w, h) = (3u32, 1u32);
        let passable = vec![false, true, true];
        let ff = compute(&passable, w, h, &[(0, 0)]); // goal on a wall
        // Nothing seeded ⇒ all unreachable.
        assert!(ff.cost.iter().all(|&c| c == UNREACHABLE));
    }
}
