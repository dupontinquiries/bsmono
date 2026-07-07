//! Navigation flow field — the CPU reference for GPU pathing.
//!
//! Per-agent A* is sequential (a priority queue) and greedy — fine for one
//! mob, too dumb and too slow for many agents reasoning about a difficult,
//! changing 3D environment. The alternative is a *flow field*: compute a
//! distance-to-goal field over a region once, then every agent simply
//! follows the downhill gradient. It is globally optimal (it sees the whole
//! region and routes around obstacles, not just the next step), trivially
//! cheap per agent (a gradient lookup), shareable across a whole flock, and
//! recomputed wholesale when terrain or the goal moves.
//!
//! The field is computed by **Jacobi relaxation**: every cell takes the min
//! of its passable neighbors' cost plus the step cost, read from the
//! previous buffer and written to the next, so updates are order-independent.
//! That *is* a GPU compute pass — one thread per cell, ping-pong buffers,
//! run K passes — which is the point: this CPU implementation is the
//! reference form a future shader transcribes, the same "functional unit:
//! CPU reference first, then GPU form" pattern as
//! [`crate::world::octree_raycast`].
//!
//! Pairs with [`crate::world::flight`]: [`NavField::flow_dir`] yields a
//! desired direction, the steering/integrator turns it into banking motion.
//! 26-connected with Euclidean step costs, so the field gives smooth 3D
//! directions for a flier.

/// Cost of a cell that can't reach the goal (blocked or walled off).
pub const UNREACHABLE: f32 = f32::INFINITY;

/// A computed distance-to-goal field over a rectangular region.
#[derive(Debug, Clone)]
pub struct NavField {
    dims: (usize, usize, usize),
    /// Distance-to-goal per cell, indexed `(z*ydim + y)*xdim + x`.
    cost: Vec<f32>,
}

/// The 26 neighbor offsets with their Euclidean step costs (1, √2, √3).
fn neighbors() -> Vec<(i32, i32, i32, f32)> {
    let mut out = Vec::with_capacity(26);
    for dz in -1..=1 {
        for dy in -1..=1 {
            for dx in -1..=1 {
                if (dx, dy, dz) == (0, 0, 0) {
                    continue;
                }
                let c = ((dx * dx + dy * dy + dz * dz) as f32).sqrt();
                out.push((dx, dy, dz, c));
            }
        }
    }
    out
}

impl NavField {
    #[inline]
    fn idx(&self, x: usize, y: usize, z: usize) -> usize {
        (z * self.dims.1 + y) * self.dims.0 + x
    }

    #[inline]
    fn in_bounds(dims: (usize, usize, usize), x: i32, y: i32, z: i32) -> bool {
        x >= 0
            && y >= 0
            && z >= 0
            && (x as usize) < dims.0
            && (y as usize) < dims.1
            && (z as usize) < dims.2
    }

    /// Compute the flow field over `dims` toward `goal`, treating only cells
    /// where `passable(x,y,z)` is true as traversable (e.g. `|x,y,z| !solid`
    /// for a flier moving through air). Cells that can't reach the goal end
    /// up [`UNREACHABLE`].
    pub fn compute<P>(dims: (usize, usize, usize), goal: (usize, usize, usize), passable: P) -> Self
    where
        P: Fn(usize, usize, usize) -> bool,
    {
        let (xd, yd, zd) = dims;
        let n = xd * yd * zd;
        let mut field = NavField { dims, cost: vec![UNREACHABLE; n] };
        if n == 0 {
            return field;
        }
        let nbrs = neighbors();

        let goal_ok = goal.0 < xd && goal.1 < yd && goal.2 < zd && passable(goal.0, goal.1, goal.2);
        if goal_ok {
            let gi = field.idx(goal.0, goal.1, goal.2);
            field.cost[gi] = 0.0;
        } else {
            return field; // unreachable goal → all UNREACHABLE
        }

        // Jacobi relaxation (the GPU-transcribable form): double-buffered so
        // a pass is order-independent. Converges within the region's longest
        // shortest-path in hops; cap generously and early-exit when stable.
        let cap = (xd + yd + zd) * 4 + 8;
        for _ in 0..cap {
            let mut next = field.cost.clone();
            let mut changed = false;
            for z in 0..zd {
                for y in 0..yd {
                    for x in 0..xd {
                        if !passable(x, y, z) || (x, y, z) == goal {
                            continue;
                        }
                        let here = field.idx(x, y, z);
                        let mut best = field.cost[here];
                        for &(dx, dy, dz, c) in &nbrs {
                            let (nx, ny, nz) = (x as i32 + dx, y as i32 + dy, z as i32 + dz);
                            if !Self::in_bounds(dims, nx, ny, nz) {
                                continue;
                            }
                            let (nx, ny, nz) = (nx as usize, ny as usize, nz as usize);
                            if !passable(nx, ny, nz) {
                                continue;
                            }
                            let v = field.cost[field.idx(nx, ny, nz)] + c;
                            if v < best {
                                best = v;
                            }
                        }
                        if best < next[here] {
                            next[here] = best;
                            changed = true;
                        }
                    }
                }
            }
            field.cost = next;
            if !changed {
                break;
            }
        }
        field
    }

    pub fn dims(&self) -> (usize, usize, usize) {
        self.dims
    }

    /// Distance-to-goal of a cell, or [`UNREACHABLE`]. Out-of-bounds →
    /// [`UNREACHABLE`].
    pub fn cost_at(&self, x: usize, y: usize, z: usize) -> f32 {
        if x >= self.dims.0 || y >= self.dims.1 || z >= self.dims.2 {
            return UNREACHABLE;
        }
        self.cost[self.idx(x, y, z)]
    }

    /// Whether a cell can reach the goal.
    pub fn reachable(&self, x: usize, y: usize, z: usize) -> bool {
        self.cost_at(x, y, z).is_finite()
    }

    /// Unit direction toward the goal from a cell — the downhill gradient,
    /// i.e. the offset to the lowest-cost passable neighbor, normalized.
    /// `[0,0,0]` at the goal, or if the cell is unreachable or has no lower
    /// neighbor.
    pub fn flow_dir(&self, x: usize, y: usize, z: usize) -> [f32; 3] {
        let here = self.cost_at(x, y, z);
        if !here.is_finite() {
            return [0.0, 0.0, 0.0];
        }
        let mut best = here;
        let mut bo = (0i32, 0i32, 0i32);
        for &(dx, dy, dz, _c) in &neighbors() {
            let (nx, ny, nz) = (x as i32 + dx, y as i32 + dy, z as i32 + dz);
            if !Self::in_bounds(self.dims, nx, ny, nz) {
                continue;
            }
            let c = self.cost_at(nx as usize, ny as usize, nz as usize);
            if c < best {
                best = c;
                bo = (dx, dy, dz);
            }
        }
        if bo == (0, 0, 0) {
            return [0.0, 0.0, 0.0];
        }
        let v = [bo.0 as f32, bo.1 as f32, bo.2 as f32];
        let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        [v[0] / len, v[1] / len, v[2] / len]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn open_region_distances_and_flow() {
        let f = NavField::compute((8, 8, 8), (0, 0, 0), |_, _, _| true);
        assert!(approx(f.cost_at(7, 0, 0), 7.0), "{}", f.cost_at(7, 0, 0));
        assert!(approx(f.cost_at(7, 7, 7), 7.0 * 3f32.sqrt()), "{}", f.cost_at(7, 7, 7));
        // Flow from the far corner points back toward the origin goal.
        let d = f.flow_dir(7, 7, 7);
        let inv = -1.0 / 3f32.sqrt();
        assert!(approx(d[0], inv) && approx(d[1], inv) && approx(d[2], inv), "{:?}", d);
    }

    #[test]
    fn goal_has_zero_cost_and_no_flow() {
        let f = NavField::compute((5, 5, 5), (2, 2, 2), |_, _, _| true);
        assert!(approx(f.cost_at(2, 2, 2), 0.0));
        assert_eq!(f.flow_dir(2, 2, 2), [0.0, 0.0, 0.0]);
    }

    #[test]
    fn routes_around_obstacle() {
        // Wall at x=4 for y>=1, gap at y=0. Goal across the wall.
        let dims = (9, 9, 1);
        let f = NavField::compute(dims, (8, 4, 0), |x, y, _| !(x == 4 && y >= 1));
        let start = f.cost_at(0, 4, 0);
        assert!(start.is_finite(), "start should be reachable via the gap");
        assert!(start > 8.0 + 0.5, "must detour, not go straight: {}", start);
        assert_eq!(f.cost_at(4, 4, 0), UNREACHABLE, "wall cell is blocked");
        // Gradient heads toward the gap: +x (toward wall/goal) and -y (down).
        let d = f.flow_dir(0, 4, 0);
        assert!(d[0] > 0.0 && d[1] < 0.0, "flow toward the gap: {:?}", d);
    }

    #[test]
    fn sealed_goal_is_unreachable() {
        // Wall at x=4 across the whole slab seals the goal off.
        let dims = (9, 9, 1);
        let f = NavField::compute(dims, (8, 4, 0), |x, _, _| x != 4);
        assert!(!f.reachable(0, 4, 0), "fully walled → unreachable");
        assert_eq!(f.flow_dir(0, 4, 0), [0.0, 0.0, 0.0]);
    }

    #[test]
    fn flow_points_to_lower_cost_neighbor() {
        // The defining property: stepping one cell along the flow direction
        // always lands on a strictly-lower-cost cell (closer to the goal).
        let f = NavField::compute((10, 10, 4), (0, 0, 0), |_, _, _| true);
        for &(x, y, z) in &[(9usize, 9usize, 3usize), (5, 2, 1), (3, 7, 0), (1, 0, 2)] {
            let d = f.flow_dir(x, y, z);
            let nx = (x as i32 + d[0].round() as i32) as usize;
            let ny = (y as i32 + d[1].round() as i32) as usize;
            let nz = (z as i32 + d[2].round() as i32) as usize;
            assert!(
                f.cost_at(nx, ny, nz) < f.cost_at(x, y, z),
                "flow must descend at {:?}: {} -> {}",
                (x, y, z),
                f.cost_at(x, y, z),
                f.cost_at(nx, ny, nz)
            );
        }
    }
}
