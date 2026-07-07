//! `sim::erosion` — one-step hydraulic and thermal erosion over a heightfield tile.
//!
//! Runs in-place on a `w×h` row-major heightfield slice alongside a parallel
//! `water` buffer. Each call to [`erode_step`] performs one combined pass:
//!
//! 1. **Rain** — add `params.rain` water to every cell.
//! 2. **Hydraulic** — water flows to the lowest neighbour; dissolves height into
//!    sediment at rate `solubility`, deposits sediment at rate `deposition` when
//!    flow slows, and carries excess sediment downstream.
//! 3. **Thermal** — slopes steeper than `params.talus` slump: half the excess
//!    height diff transfers from high cell to low neighbour.
//! 4. **Evaporation** — water multiplied by `(1 - evaporation)`.
//!
//! ## Conservation and safety
//! - Mismatched buffer lengths or `w * h != heights.len()` → **no-op, no panic**.
//! - Water is always `>= 0` after each step.
//! - No NaN or Inf is introduced; existing NaN/Inf in inputs propagates but is
//!   not amplified beyond a step's arithmetic.
//! - Total height change per step is bounded by the flow magnitude (no explosion).
//!
//! ## Relation to sibling modules
//! - Kept independent of `water_physics` (R41 sibling): `water_physics` owns
//!   free-surface simulation; this module owns height+sediment only.
//! - The bin seeds `water` from rain/`water_physics` results and applies the
//!   eroded heights back to chunk columns (coordinator wires this).

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Tuning parameters for one erosion step.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ErosionParams {
    /// Water added to every cell per step (≥ 0).
    pub rain: f32,
    /// Fraction of height dissolved into sediment under flowing water (0..1).
    pub solubility: f32,
    /// Fraction of carried sediment deposited when flow slows (0..1).
    pub deposition: f32,
    /// Fraction of water lost to evaporation per step (0..1).
    pub evaporation: f32,
    /// Maximum stable height difference before thermal slump occurs.
    pub talus: f32,
}

impl Default for ErosionParams {
    fn default() -> Self {
        Self {
            rain:        0.01,
            solubility:  0.03,
            deposition:  0.3,
            evaporation: 0.05,
            talus:       0.5,
        }
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// 4-connected neighbours (dy, dx) pairs.
const DIRS: [(isize, isize); 4] = [(-1, 0), (1, 0), (0, -1), (0, 1)];

#[inline]
fn idx(x: isize, y: isize, w: usize) -> usize {
    y as usize * w + x as usize
}

#[inline]
fn in_bounds(x: isize, y: isize, w: usize, h: usize) -> bool {
    x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h
}

// ---------------------------------------------------------------------------
// Core step
// ---------------------------------------------------------------------------

/// One combined hydraulic + thermal erosion step.
///
/// - `heights`: row-major heightfield, length must equal `w * h`.
/// - `water`:   parallel water buffer, same length.
/// - `w`, `h`: tile dimensions. If `w * h != heights.len()` or lengths differ,
///   returns immediately without modifying anything.
pub fn erode_step(heights: &mut [f32], water: &mut [f32], w: usize, h: usize, p: &ErosionParams) {
    let n = w.saturating_mul(h);
    if n == 0 || heights.len() != n || water.len() != n {
        return;
    }

    // Sediment layer, initially zero each step.
    let mut sediment = vec![0.0f32; n];

    // Clamp params to valid ranges.
    let rain        = p.rain.max(0.0);
    let solubility  = p.solubility.clamp(0.0, 1.0);
    let deposition  = p.deposition.clamp(0.0, 1.0);
    let evaporation = p.evaporation.clamp(0.0, 1.0);
    let talus       = p.talus.max(0.0);

    // Step 1: Rain.
    for w_cell in water.iter_mut() {
        *w_cell = (*w_cell + rain).max(0.0);
    }

    // Step 2: Hydraulic erosion — one pass over every cell.
    // We collect transfers into delta arrays to avoid order-dependency.
    let mut dh  = vec![0.0f32; n]; // height delta
    let mut dw  = vec![0.0f32; n]; // water delta
    let mut ds  = vec![0.0f32; n]; // sediment delta

    for row in 0..h {
        for col in 0..w {
            let i = row * w + col;
            let wi = water[i];
            if wi <= 0.0 {
                continue;
            }

            // Find the lowest (height + water) neighbour.
            let surface_i = heights[i] + wi;
            let mut best_j = None;
            let mut best_surface = surface_i;

            for &(dy, dx) in &DIRS {
                let nx = col as isize + dx;
                let ny = row as isize + dy;
                if !in_bounds(nx, ny, w, h) { continue; }
                let j = idx(nx, ny, w);
                let surface_j = heights[j] + water[j];
                if surface_j < best_surface {
                    best_surface = surface_j;
                    best_j = Some(j);
                }
            }

            let Some(j) = best_j else { continue };

            // Amount of water that flows.
            let diff = (surface_i - best_surface).max(0.0);
            let flow = (diff * 0.5).min(wi); // at most half the diff, at most all water

            // Erode: dissolve some height into sediment proportional to flow.
            let eroded = (solubility * flow).min(heights[i]);
            dh[i]  -= eroded;
            ds[i]  += eroded;

            // Carry sediment downstream; deposit some fraction where flow slows.
            let carried   = sediment[i] + eroded;
            let deposited = (deposition * carried).min(carried);
            dh[j]  += deposited;
            ds[i]  -= deposited;

            // Move water.
            dw[i]  -= flow;
            dw[j]  += flow;
        }
    }

    // Apply hydraulic deltas.
    for i in 0..n {
        heights[i] = (heights[i] + dh[i]).max(0.0);
        water[i]   = (water[i]   + dw[i]).max(0.0);
        sediment[i] = (sediment[i] + ds[i]).max(0.0);
    }

    // Deposit any residual sediment in-place (it settles where it ends up).
    for i in 0..n {
        heights[i] += sediment[i];
    }

    // Step 3: Thermal erosion — slump slopes steeper than talus.
    let mut th = vec![0.0f32; n]; // thermal height delta

    for row in 0..h {
        for col in 0..w {
            let i = row * w + col;
            for &(dy, dx) in &DIRS {
                let nx = col as isize + dx;
                let ny = row as isize + dy;
                if !in_bounds(nx, ny, w, h) { continue; }
                let j = idx(nx, ny, w);
                let diff = heights[i] - heights[j];
                if diff > talus {
                    let slump = (diff - talus) * 0.5;
                    th[i] -= slump;
                    th[j] += slump;
                }
            }
        }
    }

    // Apply thermal deltas (halve each transfer since we counted both directions).
    // The direction-split means each pair transfers once, halved.
    for i in 0..n {
        heights[i] = (heights[i] + th[i] * 0.5).max(0.0);
    }

    // Step 4: Evaporation.
    let retain = (1.0 - evaporation).max(0.0);
    for w_cell in water.iter_mut() {
        *w_cell = (*w_cell * retain).max(0.0);
    }
}

// ---------------------------------------------------------------------------
// Convenience wrapper
// ---------------------------------------------------------------------------

/// Run `n` erosion steps. Equivalent to calling [`erode_step`] `n` times.
pub fn erode(heights: &mut [f32], water: &mut [f32], w: usize, h: usize, p: &ErosionParams, n: u32) {
    for _ in 0..n {
        erode_step(heights, water, w, h, p);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(size: usize, height: f32) -> (Vec<f32>, Vec<f32>) {
        (vec![height; size * size], vec![0.0f32; size * size])
    }

    fn peak(size: usize, peak_h: f32, base_h: f32) -> (Vec<f32>, Vec<f32>) {
        let n = size * size;
        let mut h = vec![base_h; n];
        let mid = (size / 2) * size + size / 2;
        h[mid] = peak_h;
        (h, vec![0.0f32; n])
    }

    fn default_p() -> ErosionParams { ErosionParams::default() }

    #[test]
    fn flat_stays_flat() {
        // A flat field with zero rain, zero evaporation — heights should not change.
        let p = ErosionParams { rain: 0.0, ..default_p() };
        let (mut h, mut w) = flat(5, 10.0);
        let before = h.clone();
        erode_step(&mut h, &mut w, 5, 5, &p);
        for (i, (&a, &b)) in before.iter().zip(h.iter()).enumerate() {
            assert!((a - b).abs() < 1e-4, "flat cell {i} changed: {a} -> {b}");
        }
    }

    #[test]
    fn lone_peak_lowers_over_steps() {
        let (mut h, mut w) = peak(7, 20.0, 1.0);
        let mid = 3 * 7 + 3;
        let peak_before = h[mid];
        erode(&mut h, &mut w, 7, 7, &default_p(), 30);
        assert!(h[mid] < peak_before, "peak should lower: {} -> {}", peak_before, h[mid]);
    }

    #[test]
    fn peak_neighbours_rise() {
        let (mut h, mut w) = peak(7, 20.0, 1.0);
        let mid = 3 * 7 + 3;
        let right_before = h[mid + 1];
        erode(&mut h, &mut w, 7, 7, &default_p(), 30);
        assert!(h[mid + 1] > right_before,
            "neighbour should rise from deposited sediment: {} -> {}", right_before, h[mid + 1]);
    }

    #[test]
    fn slopes_end_within_talus() {
        // Build a steep ramp and erode until thermal slump should reduce it.
        let p = ErosionParams { talus: 0.5, rain: 0.0, solubility: 0.0,
                                deposition: 0.0, evaporation: 0.0 };
        let w = 4; let h = 4;
        let mut heights: Vec<f32> = (0..16).map(|i| i as f32 * 2.0).collect();
        let mut water = vec![0.0f32; 16];
        erode(&mut heights, &mut water, w, h, &p, 40);

        // Check no adjacent pair exceeds talus + a small tolerance.
        for row in 0..h {
            for col in 0..w {
                let i = row * w + col;
                for &(dy, dx) in &DIRS {
                    let nx = col as isize + dx;
                    let ny = row as isize + dy;
                    if nx < 0 || ny < 0 || nx >= w as isize || ny >= h as isize { continue; }
                    let j = idx(nx, ny, w);
                    let diff = (heights[i] - heights[j]).abs();
                    assert!(diff <= p.talus + 0.05,
                        "slope ({i},{j}) still exceeds talus: {diff} > {}", p.talus);
                }
            }
        }
    }

    #[test]
    fn water_always_non_negative() {
        let (mut h, mut w) = peak(5, 15.0, 1.0);
        erode(&mut h, &mut w, 5, 5, &default_p(), 50);
        for (i, &wv) in w.iter().enumerate() {
            assert!(wv >= 0.0, "water[{i}] < 0: {wv}");
        }
    }

    #[test]
    fn water_decays_to_near_zero_without_rain() {
        let p = ErosionParams { rain: 0.0, evaporation: 0.5, ..default_p() };
        let mut h = vec![1.0f32; 9];
        let mut w = vec![1.0f32; 9];
        erode(&mut h, &mut w, 3, 3, &p, 20);
        for (i, &wv) in w.iter().enumerate() {
            assert!(wv < 0.01, "water[{i}] should be nearly gone: {wv}");
        }
    }

    #[test]
    fn no_nan_or_inf_after_many_steps() {
        let (mut h, mut w) = peak(8, 100.0, 0.0);
        erode(&mut h, &mut w, 8, 8, &default_p(), 100);
        for (i, &hv) in h.iter().enumerate() {
            assert!(hv.is_finite(), "heights[{i}] is not finite: {hv}");
        }
        for (i, &wv) in w.iter().enumerate() {
            assert!(wv.is_finite(), "water[{i}] is not finite: {wv}");
        }
    }

    #[test]
    fn size_mismatch_is_noop() {
        let mut h = vec![5.0f32; 9];
        let mut w = vec![0.0f32; 9];
        let before_h = h.clone();
        // w=4, h=4 → 16 != 9 → no-op
        erode_step(&mut h, &mut w, 4, 4, &default_p());
        assert_eq!(h, before_h, "mismatched size should be a no-op");
    }

    #[test]
    fn water_length_mismatch_is_noop() {
        let mut h = vec![5.0f32; 9];
        let mut w = vec![0.0f32; 8]; // wrong length
        let before_h = h.clone();
        erode_step(&mut h, &mut w, 3, 3, &default_p());
        assert_eq!(h, before_h, "water length mismatch should be a no-op");
    }

    #[test]
    fn zero_size_is_noop() {
        let mut h: Vec<f32> = vec![];
        let mut w: Vec<f32> = vec![];
        erode_step(&mut h, &mut w, 0, 0, &default_p()); // must not panic
    }

    #[test]
    fn deterministic() {
        let p = default_p();
        let (mut h1, mut w1) = peak(6, 10.0, 1.0);
        let (mut h2, mut w2) = peak(6, 10.0, 1.0);
        erode(&mut h1, &mut w1, 6, 6, &p, 10);
        erode(&mut h2, &mut w2, 6, 6, &p, 10);
        assert_eq!(h1, h2, "heights not deterministic");
        assert_eq!(w1, w2, "water not deterministic");
    }

    #[test]
    fn erode_n_equals_n_steps() {
        let (mut h1, mut w1) = peak(5, 8.0, 1.0);
        let (mut h2, mut w2) = (h1.clone(), w1.clone());
        let p = default_p();
        erode(&mut h1, &mut w1, 5, 5, &p, 5);
        for _ in 0..5 { erode_step(&mut h2, &mut w2, 5, 5, &p); }
        assert_eq!(h1, h2, "erode(n) != n × erode_step");
    }
}
