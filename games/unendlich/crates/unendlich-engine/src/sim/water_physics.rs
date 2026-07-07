//! `sim::water_physics` — shallow-water flow + soil-moisture step (round R41).
//!
//! A pure step over a small `w*h` heightfield tile (row-major) that the bin runs
//! per chunk: surface water flows toward lower `height+water` neighbours and
//! pools, losing volume only to evaporation and to infiltration into the soil;
//! soil moisture diffuses toward uniform and dries out. No water/moisture core
//! existed (verified gap; `water_sight` is unrelated vision).
//!
//! Flow is mass-conserving by construction: every parcel sent from a cell is
//! subtracted from it and added to a neighbour via a flux buffer, and a cell
//! never sends more than it holds — so water is never created and never goes
//! negative; the total only drops through evaporation + infiltration.
//!
//! Composes with: `erosion` (shares the water buffer — this module owns
//! flow+moisture, erosion owns sediment+height; kept independent),
//! `plant_growth_env` (moisture01) and `germination` (moisture). Note: callers
//! read `moisture` in `[0,1]`.
//!
//! Pure, deterministic, panic-free — no `crate::render`, no GPU, no I/O, no
//! clock, no RNG, no `.unwrap()`/`.expect()`. Size mismatch (`w*h != len`) is a
//! no-op. Release-safe: every helper is unconditional (no `#[cfg]` gating).

/// Tuning for one water/moisture step. All clamped to safe ranges internally.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WaterParams {
    /// Fraction of a head difference moved per step (stable for `<= 0.5`).
    pub flow_rate: f32,
    /// Per-step fraction of water/moisture lost to evaporation.
    pub evaporation: f32,
    /// Per-step fraction of standing water that seeps into soil moisture.
    pub infiltration: f32,
    /// Moisture diffusion strength per step (stable for `<= 0.5`).
    pub spread: f32,
}

impl Default for WaterParams {
    fn default() -> Self {
        WaterParams { flow_rate: 0.5, evaporation: 0.01, infiltration: 0.05, spread: 0.25 }
    }
}

#[inline]
fn idx(x: usize, y: usize, w: usize) -> usize {
    y * w + x
}

fn dims_ok(len_a: usize, len_b: usize, len_c: usize, w: usize, h: usize) -> Option<usize> {
    let n = w.checked_mul(h)?;
    if n == 0 || len_a != n || len_b != n || len_c != n {
        None
    } else {
        Some(n)
    }
}

/// One shallow-water step. Water moves toward lower `height+water` neighbours
/// (mass-conserving), then standing water infiltrates into `moisture` and the
/// rest evaporates. Mutates `water` and `moisture` in place; no-op on size
/// mismatch.
pub fn flow_step(heights: &[f32], water: &mut [f32], moisture: &mut [f32], w: usize, h: usize, p: &WaterParams) {
    let n = match dims_ok(heights.len(), water.len(), moisture.len(), w, h) {
        Some(n) => n,
        None => return,
    };
    let fr = p.flow_rate.clamp(0.0, 0.5);
    let evap = p.evaporation.clamp(0.0, 1.0);
    let infil = p.infiltration.clamp(0.0, 1.0);

    // Flux buffer: net water change per cell (sums to ~0 ⇒ conserves mass).
    let mut delta = vec![0.0f32; n];

    let mut y = 0;
    while y < h {
        let mut x = 0;
        while x < w {
            let i = idx(x, y, w);
            let wi = water[i];
            if wi > 0.0 {
                let hi = heights[i] + wi;
                // gather lower neighbours (4-connected, no wrap)
                let mut nbr = [(0usize, 0.0f32); 4];
                let mut cnt = 0usize;
                let mut sum_d = 0.0f32;
                let consider = |j: usize, nbr: &mut [(usize, f32); 4], cnt: &mut usize, sum_d: &mut f32| {
                    let d = hi - (heights[j] + water[j]);
                    if d > 0.0 {
                        nbr[*cnt] = (j, d);
                        *cnt += 1;
                        *sum_d += d;
                    }
                };
                if x > 0 {
                    consider(idx(x - 1, y, w), &mut nbr, &mut cnt, &mut sum_d);
                }
                if x + 1 < w {
                    consider(idx(x + 1, y, w), &mut nbr, &mut cnt, &mut sum_d);
                }
                if y > 0 {
                    consider(idx(x, y - 1, w), &mut nbr, &mut cnt, &mut sum_d);
                }
                if y + 1 < h {
                    consider(idx(x, y + 1, w), &mut nbr, &mut cnt, &mut sum_d);
                }

                if sum_d > 0.0 {
                    // proposed outflow ∝ head difference, capped to available water
                    let mut sum_out = 0.0f32;
                    let mut k = 0;
                    while k < cnt {
                        sum_out += fr * nbr[k].1;
                        k += 1;
                    }
                    let scale = if sum_out > wi { wi / sum_out } else { 1.0 };
                    let mut k = 0;
                    while k < cnt {
                        let o = fr * nbr[k].1 * scale;
                        delta[i] -= o;
                        delta[nbr[k].0] += o;
                        k += 1;
                    }
                }
            }
            x += 1;
        }
        y += 1;
    }

    // apply flow, then infiltration (water → moisture) and evaporation
    let mut i = 0;
    while i < n {
        let mut wi = water[i] + delta[i];
        if wi < 0.0 {
            wi = 0.0; // fp guard; outflow is capped so this is at most epsilon
        }
        if wi > 0.0 {
            let seep = infil * wi;
            wi -= seep;
            let m = moisture[i] + seep;
            moisture[i] = if m > 1.0 { 1.0 } else { m }; // excess beyond saturation is lost
            wi *= 1.0 - evap;
            if wi < 0.0 {
                wi = 0.0;
            }
        }
        water[i] = wi;
        i += 1;
    }
}

/// One soil-moisture step: moisture diffuses toward its neighbours' average and
/// dries via evaporation. Stays in `[0,1]`. Mutates in place; no-op on mismatch.
pub fn moisture_step(moisture: &mut [f32], w: usize, h: usize, p: &WaterParams) {
    let n = match dims_ok(moisture.len(), moisture.len(), moisture.len(), w, h) {
        Some(n) => n,
        None => return,
    };
    let spread = p.spread.clamp(0.0, 0.5);
    let evap = p.evaporation.clamp(0.0, 1.0);

    let old = moisture.to_vec(); // Jacobi: read old, write new

    let mut y = 0;
    while y < h {
        let mut x = 0;
        while x < w {
            let i = idx(x, y, w);
            let mut sum = 0.0f32;
            let mut cnt = 0.0f32;
            if x > 0 {
                sum += old[idx(x - 1, y, w)];
                cnt += 1.0;
            }
            if x + 1 < w {
                sum += old[idx(x + 1, y, w)];
                cnt += 1.0;
            }
            if y > 0 {
                sum += old[idx(x, y - 1, w)];
                cnt += 1.0;
            }
            if y + 1 < h {
                sum += old[idx(x, y + 1, w)];
                cnt += 1.0;
            }
            let avg = if cnt > 0.0 { sum / cnt } else { old[i] };
            let mut m = old[i] + spread * (avg - old[i]); // diffuse toward uniform
            m *= 1.0 - evap; // dry
            moisture[i] = m.clamp(0.0, 1.0);
            x += 1;
        }
        y += 1;
    }
    let _ = n;
}

/// Run `n` combined `flow_step` + `moisture_step` iterations.
pub fn simulate(heights: &[f32], water: &mut [f32], moisture: &mut [f32], w: usize, h: usize, p: &WaterParams, n: u32) {
    let mut k = 0;
    while k < n {
        flow_step(heights, water, moisture, w, h, p);
        moisture_step(moisture, w, h, p);
        k += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn total(v: &[f32]) -> f32 {
        v.iter().sum()
    }
    fn lcg(s: &mut u32) -> f32 {
        *s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (*s >> 8) as f32 / ((1u32 << 24) as f32)
    }

    #[test]
    fn water_stays_nonnegative() {
        let (w, h) = (8, 8);
        let mut s = 0x41_9A7E_01u32; // documented seed (R20)
        let heights: Vec<f32> = (0..w * h).map(|_| lcg(&mut s) * 10.0).collect();
        let mut water: Vec<f32> = (0..w * h).map(|_| lcg(&mut s) * 2.0).collect();
        let mut moist: Vec<f32> = (0..w * h).map(|_| lcg(&mut s)).collect();
        let p = WaterParams::default();
        for _ in 0..100 {
            flow_step(&heights, &mut water, &mut moist, w, h, &p);
            moisture_step(&mut moist, w, h, &p);
            assert!(water.iter().all(|&x| x >= 0.0 && x.is_finite()), "water must stay >=0 & finite");
        }
    }

    #[test]
    fn pure_flow_conserves_total() {
        // evaporation + infiltration off ⇒ flow neither creates nor destroys water.
        let (w, h) = (6, 6);
        let mut s = 0x41_C0_02u32;
        let heights: Vec<f32> = (0..w * h).map(|_| lcg(&mut s) * 5.0).collect();
        let mut water: Vec<f32> = (0..w * h).map(|_| lcg(&mut s) * 3.0).collect();
        let mut moist = vec![0.0f32; w * h];
        let before = total(&water);
        let p = WaterParams { flow_rate: 0.5, evaporation: 0.0, infiltration: 0.0, spread: 0.0 };
        for _ in 0..50 {
            flow_step(&heights, &mut water, &mut moist, w, h, &p);
        }
        assert!((total(&water) - before).abs() < 1e-2, "pure flow conserves total ({} vs {})", total(&water), before);
        assert!(water.iter().all(|&x| x >= 0.0));
    }

    #[test]
    fn evaporation_and_infiltration_only_decrease_total() {
        let (w, h) = (5, 5);
        let heights = vec![0.0f32; w * h]; // flat
        let mut water = vec![1.0f32; w * h];
        let mut moist = vec![0.0f32; w * h];
        let before = total(&water);
        let p = WaterParams { flow_rate: 0.5, evaporation: 0.02, infiltration: 0.05, spread: 0.0 };
        flow_step(&heights, &mut water, &mut moist, w, h, &p);
        let after = total(&water);
        assert!(after < before, "evap+infil reduce total water");
        assert!(after > 0.0);
        // and never increases over many steps
        let mut prev = after;
        for _ in 0..30 {
            flow_step(&heights, &mut water, &mut moist, w, h, &p);
            let t = total(&water);
            assert!(t <= prev + 1e-4, "water total never increases");
            prev = t;
        }
    }

    #[test]
    fn flows_downhill_lower_cell_gains() {
        // 1x3 descending slope; water on the high (left) cell ⇒ right cells gain.
        let (w, h) = (3usize, 1usize);
        let heights = [2.0f32, 1.0, 0.0];
        let mut water = [1.0f32, 0.0, 0.0];
        let mut moist = [0.0f32; 3];
        let p = WaterParams { flow_rate: 0.5, evaporation: 0.0, infiltration: 0.0, spread: 0.0 };
        flow_step(&heights, &mut water, &mut moist, w, h, &p);
        assert!(water[1] > 0.0, "middle (lower) cell gained water");
        assert!(water[0] < 1.0, "source cell lost water");
        // run to settle: water ends up pooled toward the low end
        for _ in 0..200 {
            flow_step(&heights, &mut water, &mut moist, w, h, &p);
        }
        assert!(water[2] > water[0], "low end holds more than high end at rest");
    }

    #[test]
    fn flat_uniform_sheet_is_stable() {
        // uniform water on flat ground: equal surface everywhere ⇒ no flow, only evap.
        let (w, h) = (5, 5);
        let heights = vec![3.0f32; w * h];
        let mut water = vec![0.5f32; w * h];
        let mut moist = vec![0.0f32; w * h];
        let p = WaterParams { flow_rate: 0.5, evaporation: 0.0, infiltration: 0.0, spread: 0.0 };
        let prev = water.clone();
        flow_step(&heights, &mut water, &mut moist, w, h, &p);
        for i in 0..w * h {
            assert!((water[i] - prev[i]).abs() < 1e-6, "flat uniform sheet must not move");
        }
    }

    #[test]
    fn flat_single_puddle_keeps_center_of_mass() {
        // single puddle on flat ground spreads symmetrically ⇒ COM unchanged (no drift).
        let (w, h) = (5usize, 5usize);
        let heights = vec![0.0f32; w * h];
        let mut water = vec![0.0f32; w * h];
        water[idx(2, 2, w)] = 4.0; // centered
        let mut moist = vec![0.0f32; w * h];
        let p = WaterParams { flow_rate: 0.5, evaporation: 0.0, infiltration: 0.0, spread: 0.0 };
        let com = |water: &[f32]| {
            let (mut sx, mut sy, mut tot) = (0.0f32, 0.0f32, 0.0f32);
            for yy in 0..h {
                for xx in 0..w {
                    let a = water[idx(xx, yy, w)];
                    sx += a * xx as f32;
                    sy += a * yy as f32;
                    tot += a;
                }
            }
            (sx / tot, sy / tot)
        };
        for _ in 0..20 {
            flow_step(&heights, &mut water, &mut moist, w, h, &p);
        }
        let (cx, cy) = com(&water);
        assert!((cx - 2.0).abs() < 1e-3 && (cy - 2.0).abs() < 1e-3, "puddle stays centered ({cx},{cy})");
    }

    #[test]
    fn moisture_in_range_diffuses_and_dries() {
        let (w, h) = (5, 5);
        let mut moist = vec![0.0f32; w * h];
        moist[idx(2, 2, w)] = 1.0; // a spike
        let p = WaterParams { flow_rate: 0.0, evaporation: 0.0, infiltration: 0.0, spread: 0.4 };
        moisture_step(&mut moist, w, h, &p);
        assert!(moist.iter().all(|&m| (0.0..=1.0).contains(&m)), "moisture stays in [0,1]");
        assert!(moist[idx(2, 2, w)] < 1.0, "spike diffuses down");
        assert!(moist[idx(1, 2, w)] > 0.0, "neighbour gains");
        // drying: with evaporation, total moisture decreases
        let mut m2 = vec![0.5f32; w * h];
        let before = total(&m2);
        let pe = WaterParams { evaporation: 0.1, spread: 0.0, ..WaterParams::default() };
        moisture_step(&mut m2, w, h, &pe);
        assert!(total(&m2) < before, "evaporation dries soil");
    }

    #[test]
    fn infiltration_raises_moisture_under_standing_water() {
        let (w, h) = (3, 3);
        let heights = vec![0.0f32; w * h];
        let mut water = vec![1.0f32; w * h];
        let mut moist = vec![0.0f32; w * h];
        let p = WaterParams { flow_rate: 0.0, evaporation: 0.0, infiltration: 0.2, spread: 0.0 };
        flow_step(&heights, &mut water, &mut moist, w, h, &p);
        assert!(moist.iter().all(|&m| m > 0.0 && m <= 1.0), "soil gains moisture under water");
    }

    #[test]
    fn no_nan_after_many_steps_and_size_mismatch_is_noop() {
        let (w, h) = (7, 7);
        let mut s = 0x41_DEAD_03u32;
        let heights: Vec<f32> = (0..w * h).map(|_| lcg(&mut s) * 8.0).collect();
        let mut water: Vec<f32> = (0..w * h).map(|_| lcg(&mut s) * 2.0).collect();
        let mut moist: Vec<f32> = (0..w * h).map(|_| lcg(&mut s)).collect();
        simulate(&heights, &mut water, &mut moist, w, h, &WaterParams::default(), 500);
        assert!(water.iter().chain(moist.iter()).all(|x| x.is_finite()), "no NaN/inf after many steps");

        // size mismatch ⇒ buffers untouched, no panic
        let mut wat2 = vec![1.0f32; 3]; // wrong length for 7x7
        let mut moi2 = vec![0.0f32; 3];
        let snap_w = wat2.clone();
        flow_step(&heights, &mut wat2, &mut moi2, w, h, &WaterParams::default());
        moisture_step(&mut moi2, w, h, &WaterParams::default());
        assert_eq!(wat2, snap_w, "mismatched size is a no-op");
    }

    #[test]
    fn deterministic() {
        let (w, h) = (4, 4);
        let heights: Vec<f32> = (0..16).map(|i| i as f32 * 0.3).collect();
        let mk = || (vec![1.0f32; 16], vec![0.0f32; 16]);
        let (mut a_w, mut a_m) = mk();
        let (mut b_w, mut b_m) = mk();
        let p = WaterParams::default();
        simulate(&heights, &mut a_w, &mut a_m, w, h, &p, 25);
        simulate(&heights, &mut b_w, &mut b_m, w, h, &p, 25);
        assert_eq!(a_w, b_w);
        assert_eq!(a_m, b_m);
    }
}
