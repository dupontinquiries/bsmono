//! `sim::river_flow` — hydrology core: D8 flow direction, flow accumulation, and
//! channel velocity.
//!
//! Per `docs/specs/river_systems.docx` (7 river archetypes over the shared
//! H/R/W/P/L/S/B property system). This is the CORE flow math; archetype tuning
//! comes later. It operates on already-sampled height arrays (row-major,
//! `len == w*h`) — no terrain gen, no GPU, no IO. Pure, deterministic,
//! panic-free (R1/R6/R11); standalone `rustc --test`-able, release-safe.

use std::f32::consts::SQRT_2;

fn in_grid(heights: &[f32], w: usize, h: usize) -> bool {
    w != 0 && h != 0 && heights.len() == w * h
}

/// Index of the steepest-descent (D8) neighbour of cell `i`: the in-bounds
/// neighbour maximising `(height[i] − height[n]) / distance` (distance `1` for
/// orthogonal, `√2` for diagonal) among strictly-lower neighbours. `None` for a
/// pit or flat (no strictly-lower neighbour). Ties break to the lowest neighbour
/// index (deterministic). Safe on empty/ragged input or out-of-range `i`.
pub fn flow_dir_d8(heights: &[f32], w: usize, h: usize, i: usize) -> Option<usize> {
    if !in_grid(heights, w, h) || i >= w * h {
        return None;
    }
    let hi = heights[i];
    if !hi.is_finite() {
        return None;
    }
    let (x, y) = (i % w, i / w);
    let mut best: Option<usize> = None;
    let mut best_slope = 0.0f32;
    // iterate neighbours in ascending index order so equal slopes keep the
    // lowest neighbour index
    for dy in -1i64..=1 {
        for dx in -1i64..=1 {
            if dx == 0 && dy == 0 {
                continue;
            }
            let nx = x as i64 + dx;
            let ny = y as i64 + dy;
            if nx < 0 || ny < 0 || nx >= w as i64 || ny >= h as i64 {
                continue;
            }
            let ni = ny as usize * w + nx as usize;
            let hn = heights[ni];
            if !hn.is_finite() {
                continue;
            }
            let drop = hi - hn;
            if !(drop > 0.0) {
                continue; // not strictly lower (also rejects NaN)
            }
            let dist = if dx != 0 && dy != 0 { SQRT_2 } else { 1.0 };
            let slope = drop / dist;
            if best.is_none() || slope > best_slope {
                best = Some(ni);
                best_slope = slope;
            }
        }
    }
    best
}

/// Flow accumulation: for each cell, the number of cells (including itself, so
/// every cell is `>= 1`) whose drainage passes through it. Rivers emerge where
/// this is high. Downstream cells are `>=` the sum of their upstream
/// contributors. Empty/ragged input ⇒ empty vec.
pub fn accumulate_flow(heights: &[f32], w: usize, h: usize) -> Vec<f32> {
    if !in_grid(heights, w, h) {
        return Vec::new();
    }
    let n = w * h;
    let mut accum = vec![1.0f32; n];

    // Process cells from highest to lowest. Flow only goes to strictly-lower
    // cells, so when a cell is processed all its upstream contributors (which are
    // strictly higher) are already finalised. total_cmp gives a deterministic
    // total order (incl. NaN) with no unwrap; index breaks height ties.
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| heights[b].total_cmp(&heights[a]).then(a.cmp(&b)));

    for &c in &order {
        if let Some(d) = flow_dir_d8(heights, w, h, c) {
            accum[d] += accum[c];
        }
    }
    accum
}

/// Minimum roughness, so velocity stays finite.
const MIN_ROUGHNESS: f32 = 0.01;

/// Manning-style channel velocity: faster on steeper / deeper / smoother
/// channels. `v = depth^(2/3) · √slope / roughness`. Zero at zero slope; finite
/// and `>= 0` for any input (negatives/NaN sanitised, roughness floored).
pub fn channel_velocity(slope: f32, depth: f32, roughness: f32) -> f32 {
    let s = slope.max(0.0); // NaN/neg → 0
    let d = depth.max(0.0);
    let r = roughness.max(MIN_ROUGHNESS); // NaN → MIN_ROUGHNESS
    let v = d.powf(2.0 / 3.0) * s.sqrt() / r;
    if v.is_finite() {
        v.max(0.0)
    } else {
        0.0
    }
}

/// A cell is a river channel when its accumulation meets `threshold`.
pub fn is_channel(accum: f32, threshold: f32) -> bool {
    accum.is_finite() && accum >= threshold
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flow_dir_points_downhill_pit_is_none_ties_deterministic() {
        // 3x3, center high, west neighbour clearly lowest
        // idx: 0 1 2 / 3 4 5 / 6 7 8
        let mut grid = vec![5.0f32; 9];
        grid[4] = 9.0; // center
        grid[3] = 1.0; // west — lowest, orthogonal (steepest)
        assert_eq!(flow_dir_d8(&grid, 3, 3, 4), Some(3));

        // pit: center lower than all neighbours → None
        let mut pit = vec![5.0f32; 9];
        pit[4] = 0.0;
        assert_eq!(flow_dir_d8(&pit, 3, 3, 4), None);

        // flat: all equal → None
        let flat = vec![2.0f32; 9];
        assert_eq!(flow_dir_d8(&flat, 3, 3, 4), None);

        // ties: two orthogonal neighbours equally lower → lowest index (3 < 5)
        let mut tie = vec![5.0f32; 9];
        tie[4] = 9.0;
        tie[3] = 2.0;
        tie[5] = 2.0;
        assert_eq!(flow_dir_d8(&tie, 3, 3, 4), Some(3), "tie should pick lowest index");
    }

    #[test]
    fn accumulate_basics_and_monotone_downhill() {
        // single descending row: 4 3 2 1 → flows right, accum 1 2 3 4
        let row = [4.0f32, 3.0, 2.0, 1.0];
        let a = accumulate_flow(&row, 4, 1);
        assert_eq!(a, vec![1.0, 2.0, 3.0, 4.0]);
        // every cell >= 1
        assert!(a.iter().all(|&v| v >= 1.0));
        // monotonically increasing downhill
        assert!(a.windows(2).all(|w| w[1] > w[0]));
    }

    #[test]
    fn accumulate_downstream_ge_upstream_sum() {
        // a small basin draining to one outlet; check the outlet >= its inflows
        // 3x3 bowl sloping toward center-bottom (idx 7)
        let grid = [
            9.0, 9.0, 9.0, //
            6.0, 6.0, 6.0, //
            3.0, 1.0, 3.0, //
        ];
        let a = accumulate_flow(&grid, 3, 3);
        assert!(a.iter().all(|&v| v >= 1.0), "all cells >= 1");
        // outlet (lowest, idx 7) should accumulate the most
        let outlet = a[7];
        let max = a.iter().cloned().fold(0.0f32, f32::max);
        assert_eq!(outlet, max, "lowest cell should hold the max accumulation");
        // conservation: total accumulation deposited equals sum over cells of
        // their own +1 contributions counted along their path ≥ n
        assert!(a.iter().sum::<f32>() >= 9.0);
    }

    #[test]
    fn channel_velocity_monotone_zero_at_zero_slope() {
        assert_eq!(channel_velocity(0.0, 2.0, 0.03), 0.0, "zero slope ⇒ zero velocity");
        // increases with slope
        assert!(channel_velocity(0.5, 1.0, 0.03) > channel_velocity(0.1, 1.0, 0.03));
        // increases with depth
        assert!(channel_velocity(0.2, 4.0, 0.03) > channel_velocity(0.2, 1.0, 0.03));
        // decreases with roughness
        assert!(channel_velocity(0.2, 1.0, 0.02) > channel_velocity(0.2, 1.0, 0.08));
    }

    #[test]
    fn velocity_and_channel_finite_and_safe() {
        let mut s = 0x58FA_u64; // documented seed (R20)
        let mut rng = || {
            s = s.wrapping_mul(6364136223846793005).wrapping_add(1);
            (s >> 40) as f32 / ((1u64 << 24) as f32)
        };
        for _ in 0..4000 {
            let v = channel_velocity(rng() * 4.0 - 2.0, rng() * 10.0 - 2.0, rng() * 2.0 - 1.0);
            assert!(v.is_finite() && v >= 0.0, "velocity oob: {}", v);
        }
        // explicit NaN/inf inputs sanitise
        assert_eq!(channel_velocity(f32::NAN, f32::NAN, f32::NAN).is_finite(), true);
        assert!(channel_velocity(f32::INFINITY, 1.0, 0.0).is_finite());
        // is_channel threshold
        assert!(is_channel(10.0, 5.0));
        assert!(!is_channel(3.0, 5.0));
        assert!(!is_channel(f32::NAN, 5.0));
    }

    #[test]
    fn empty_and_ragged_inputs_safe() {
        assert!(accumulate_flow(&[], 0, 0).is_empty());
        assert!(accumulate_flow(&[1.0, 2.0, 3.0], 2, 2).is_empty(), "ragged ⇒ empty");
        assert_eq!(flow_dir_d8(&[], 0, 0, 0), None);
        assert_eq!(flow_dir_d8(&[1.0, 2.0, 3.0], 2, 2, 0), None, "ragged ⇒ None");
        assert_eq!(flow_dir_d8(&[1.0; 9], 3, 3, 99), None, "out-of-range i ⇒ None");
    }

    #[test]
    fn deterministic() {
        let grid = [5.0f32, 4.0, 3.0, 4.0, 2.0, 1.0, 3.0, 2.0, 0.0];
        assert_eq!(accumulate_flow(&grid, 3, 3), accumulate_flow(&grid, 3, 3));
        assert_eq!(flow_dir_d8(&grid, 3, 3, 4), flow_dir_d8(&grid, 3, 3, 4));
    }
}
