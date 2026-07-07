//! `water_cell` — voxel water cellular-automaton step + freeze/thaw.
//!
//! Worker-03 / WORLD-PERF pure core, round 5. One settling update over a level
//! grid: water moves toward lower *surface* (terrain + water) neighbors,
//! conserving total water everywhere except tagged source/sink cells, and
//! settling toward flat pools. A temperature input freezes water to ice
//! (held, doesn't flow) below a threshold and thaws it above — pair the temps
//! with `climate::temp_for`.
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
//!
//! Mass safety: outflow from a cell is scaled so it never exceeds the water
//! present, so water never goes negative and (off sources) total is conserved.
#![allow(dead_code)]

/// Tuning for the water step.
#[derive(Clone, Copy, Debug)]
pub struct WaterParams {
    /// Global flow rate multiplier (1.0 = default). Per-neighbor move is
    /// `flow_factor * 0.25 * surface_diff`, which stays well under the
    /// half-difference overshoot bound, so surfaces settle monotonically.
    pub flow_factor: f32,
    /// Water freezes where `temp < freeze_thresh`.
    pub freeze_thresh: f32,
}

impl Default for WaterParams {
    fn default() -> Self {
        WaterParams {
            flow_factor: 1.0,
            freeze_thresh: 0.0,
        }
    }
}

/// Frozen iff temperature is below the threshold.
pub fn is_frozen(temp: f32, thresh: f32) -> bool {
    temp < thresh
}

/// Per-cell ice mask from a temperature field.
pub fn freeze_mask(temps: &[f32], thresh: f32) -> Vec<bool> {
    temps.iter().map(|&t| is_frozen(t, thresh)).collect()
}

/// Total water across the grid.
pub fn total_water(water: &[f32]) -> f32 {
    water.iter().sum()
}

/// One settling step over a `w × h` grid (row-major).
///
/// * `terrain` — fixed ground height per cell.
/// * `water` — water depth per cell (mutated in place; clamped ≥ 0).
/// * `frozen` — cells whose water is ice this step (no flow in or out).
/// * `source` — per-cell net add (positive) / remove (negative) applied after
///   flow; this is the only thing that changes total water.
///
/// Flow is computed from a frozen snapshot of the surface (double-buffered via
/// a delta accumulator), so the result is order-independent and deterministic.
pub fn step(
    terrain: &[f32],
    water: &mut [f32],
    frozen: &[bool],
    source: &[f32],
    params: &WaterParams,
    w: u32,
    h: u32,
) {
    let wu = w as usize;
    let hu = h as usize;
    let n = wu * hu;
    if n == 0 || water.len() < n || terrain.len() < n {
        return;
    }
    let f = if params.flow_factor.is_finite() && params.flow_factor > 0.0 {
        params.flow_factor
    } else {
        0.0
    };

    let surf: Vec<f32> = (0..n).map(|c| terrain[c] + water[c]).collect();
    let is_froz = |c: usize| c < frozen.len() && frozen[c];

    // Pass 1: desired total outflow per cell (to strictly-lower, non-frozen neighbors).
    let mut out_sum = vec![0.0f32; n];
    for c in 0..n {
        if is_froz(c) || water[c] <= 0.0 {
            continue;
        }
        for nb in neighbors(c, wu, hu) {
            if is_froz(nb) {
                continue;
            }
            let diff = surf[c] - surf[nb];
            if diff > 0.0 {
                out_sum[c] += f * 0.25 * diff;
            }
        }
    }

    // Pass 2: scale each cell's outflow by available water, accumulate deltas.
    let mut delta = vec![0.0f32; n];
    for c in 0..n {
        if out_sum[c] <= 0.0 {
            continue;
        }
        let scale = if out_sum[c] > water[c] {
            water[c] / out_sum[c]
        } else {
            1.0
        };
        for nb in neighbors(c, wu, hu) {
            if is_froz(nb) {
                continue;
            }
            let diff = surf[c] - surf[nb];
            if diff > 0.0 {
                let moved = f * 0.25 * diff * scale;
                delta[c] -= moved;
                delta[nb] += moved;
            }
        }
    }

    // Apply flow, then sources/sinks; clamp non-negative.
    for c in 0..n {
        let s = source.get(c).copied().unwrap_or(0.0);
        let v = water[c] + delta[c] + s;
        water[c] = if v.is_nan() || v < 0.0 { 0.0 } else { v };
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn surface_range(terrain: &[f32], water: &[f32]) -> f32 {
        let mut lo = f32::INFINITY;
        let mut hi = f32::NEG_INFINITY;
        for i in 0..water.len() {
            let s = terrain[i] + water[i];
            if s < lo {
                lo = s;
            }
            if s > hi {
                hi = s;
            }
        }
        hi - lo
    }

    #[test]
    fn freeze_iff_below_threshold() {
        assert!(is_frozen(-1.0, 0.0));
        assert!(!is_frozen(0.0, 0.0)); // not strictly below
        assert!(!is_frozen(5.0, 0.0));
        let m = freeze_mask(&[-2.0, 0.0, 3.0, -0.1], 0.0);
        assert_eq!(m, vec![true, false, false, true]);
    }

    #[test]
    fn conserves_total_off_sources() {
        let (w, h) = (4u32, 4u32);
        let terrain = vec![0.0f32; 16];
        let mut water = vec![0.0f32; 16];
        water[5] = 10.0; // a blob
        water[10] = 3.0;
        let frozen = vec![false; 16];
        let source = vec![0.0f32; 16];
        let before = total_water(&water);
        let p = WaterParams::default();
        for _ in 0..50 {
            step(&terrain, &mut water, &frozen, &source, &p, w, h);
            assert!((total_water(&water) - before).abs() < 1e-3, "water not conserved");
        }
    }

    #[test]
    fn never_negative() {
        let (w, h) = (5u32, 5u32);
        let terrain: Vec<f32> = (0..25).map(|i| (i % 5) as f32).collect();
        let mut water = vec![1.0f32; 25];
        let frozen = vec![false; 25];
        // sinks pulling water out aggressively
        let mut source = vec![0.0f32; 25];
        source[12] = -100.0;
        let p = WaterParams::default();
        for _ in 0..30 {
            step(&terrain, &mut water, &frozen, &source, &p, w, h);
            for &v in &water {
                assert!(v >= 0.0, "negative water");
            }
        }
    }

    #[test]
    fn settles_monotonically_toward_flat() {
        let (w, h) = (8u32, 1u32);
        let terrain = vec![0.0f32; 8];
        let mut water = vec![0.0f32; 8];
        water[0] = 8.0; // all piled on one end
        let frozen = vec![false; 8];
        let source = vec![0.0f32; 8];
        let p = WaterParams::default();
        let mut prev_range = surface_range(&terrain, &water);
        for _ in 0..200 {
            step(&terrain, &mut water, &frozen, &source, &p, w, h);
            let r = surface_range(&terrain, &water);
            assert!(r <= prev_range + 1e-4, "surface range increased (not settling)");
            prev_range = r;
        }
        // Eventually nearly flat.
        assert!(prev_range < 0.5);
    }

    #[test]
    fn frozen_water_does_not_move() {
        let (w, h) = (3u32, 1u32);
        let terrain = vec![0.0f32; 3];
        let mut water = vec![5.0, 0.0, 0.0];
        let frozen = vec![true, false, false]; // cell 0 is ice
        let source = vec![0.0f32; 3];
        let p = WaterParams::default();
        step(&terrain, &mut water, &frozen, &source, &p, w, h);
        assert_eq!(water[0], 5.0, "ice moved");
    }

    #[test]
    fn deterministic() {
        let (w, h) = (6u32, 6u32);
        let terrain: Vec<f32> = (0..36).map(|i| ((i * 7) % 5) as f32 * 0.5).collect();
        let frozen = vec![false; 36];
        let source = vec![0.0f32; 36];
        let p = WaterParams::default();
        let mut a = vec![2.0f32; 36];
        let mut b = vec![2.0f32; 36];
        for _ in 0..20 {
            step(&terrain, &mut a, &frozen, &source, &p, w, h);
            step(&terrain, &mut b, &frozen, &source, &p, w, h);
        }
        assert_eq!(a, b);
    }
}
