//! `orogeny` — mountains via a process, not raw noise.
//!
//! Worker-03 / WORLD-PERF pure core, round 2. A deterministic uplift +
//! erosion model: a tectonic uplift field (seeded value noise) is carved by
//! repeated thermal-erosion relaxation — material moves only downhill, above a
//! talus slope — producing elevation deltas that read like ranges and valleys
//! rather than undirected noise. The terrain generator can sample this where it
//! wants process-built mountains.
//!
//! Pure & deterministic (the only randomness is the seed); no clock/rng/unwrap.
//! Self-contained for standalone `rustc --test`.
//!
//! ## Invariant
//! Erosion moves mass strictly downhill, so any *local maximum* only ever loses
//! height — a peak never rises above the uplift that raised it. Concretely,
//! `max(simulate) <= max(uplift_field)`.
#![allow(dead_code)]

/// Maximum elevation (a heightmap-style ceiling); output stays in [0, this].
pub const UPLIFT_MAX: f32 = 255.0;

/// Erosion tuning.
#[derive(Clone, Copy, Debug)]
pub struct OrogenyParams {
    /// Minimum height difference to a neighbor before material flows.
    pub talus: f32,
    /// Fraction of the over-talus excess moved per neighbor per pass.
    pub strength: f32,
    /// Number of relaxation passes.
    pub iterations: u32,
}

impl Default for OrogenyParams {
    fn default() -> Self {
        OrogenyParams {
            talus: 4.0,
            strength: 0.2,
            iterations: 12,
        }
    }
}

// Integer hash → [0,1). Deterministic; seed-mixed (splitmix-style finalizer).
fn hash01(ix: i64, iy: i64, seed: u64) -> f32 {
    let mut h = seed
        ^ (ix as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (iy as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    h ^= h >> 33;
    h = h.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
    h ^= h >> 33;
    h = h.wrapping_mul(0xC4CE_B9FE_1A85_EC53);
    h ^= h >> 33;
    ((h >> 40) as f32) / ((1u64 << 24) as f32)
}

fn smoothstep(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

// Bilinear value noise on a lattice of cell size `period`.
fn value_noise(x: f32, y: f32, period: f32, seed: u64) -> f32 {
    let fx = x / period;
    let fy = y / period;
    let x0 = fx.floor();
    let y0 = fy.floor();
    let tx = smoothstep(fx - x0);
    let ty = smoothstep(fy - y0);
    let ix = x0 as i64;
    let iy = y0 as i64;
    let v00 = hash01(ix, iy, seed);
    let v10 = hash01(ix + 1, iy, seed);
    let v01 = hash01(ix, iy + 1, seed);
    let v11 = hash01(ix + 1, iy + 1, seed);
    let a = v00 + (v10 - v00) * tx;
    let b = v01 + (v11 - v01) * tx;
    a + (b - a) * ty
}

/// Tectonic uplift at (x, y) in [0, UPLIFT_MAX] — two octaves of value noise.
pub fn uplift_at(x: i32, y: i32, seed: u64) -> f32 {
    let xf = x as f32;
    let yf = y as f32;
    let n = 0.65 * value_noise(xf, yf, 32.0, seed)
        + 0.35 * value_noise(xf, yf, 11.0, seed ^ 0xA5A5_5A5A_5A5A_A5A5);
    clamp(n, 0.0, 1.0) * UPLIFT_MAX
}

/// The raw uplift field over a region (row-major, width × height).
pub fn uplift_field(width: u32, height: u32, seed: u64) -> Vec<f32> {
    let mut v = Vec::with_capacity((width as usize) * (height as usize));
    for y in 0..height as i32 {
        for x in 0..width as i32 {
            v.push(uplift_at(x, y, seed));
        }
    }
    v
}

/// Uplift, then thermal-erosion relaxation. Returns elevation per cell
/// (row-major), bounded to [0, UPLIFT_MAX]. Material moves only to strictly
/// lower neighbors above the talus, so peaks only lose height.
pub fn simulate(width: u32, height: u32, seed: u64, params: OrogenyParams) -> Vec<f32> {
    let w = width as usize;
    let h = height as usize;
    let mut elev = uplift_field(width, height, seed);
    if w == 0 || h == 0 {
        return elev;
    }
    let strength = clamp(params.strength, 0.0, 0.24);
    let talus = if params.talus > 0.0 { params.talus } else { 0.0 };
    let mut delta = vec![0.0f32; w * h];

    for _ in 0..params.iterations {
        for d in delta.iter_mut() {
            *d = 0.0;
        }
        // Accumulate moves against the frozen `elev` (double-buffered via delta).
        for y in 0..h {
            for x in 0..w {
                let c = y * w + x;
                let ec = elev[c];
                let neighbors = [
                    (x > 0, c.wrapping_sub(1)),
                    (x + 1 < w, c + 1),
                    (y > 0, c.wrapping_sub(w)),
                    (y + 1 < h, c + w),
                ];
                for (in_bounds, n) in neighbors {
                    if !in_bounds {
                        continue;
                    }
                    let diff = ec - elev[n];
                    if diff > talus {
                        let move_amt = strength * (diff - talus) * 0.25;
                        delta[c] -= move_amt;
                        delta[n] += move_amt;
                    }
                }
            }
        }
        for i in 0..w * h {
            elev[i] = clamp(elev[i] + delta[i], 0.0, UPLIFT_MAX);
        }
    }
    elev
}

fn clamp(v: f32, lo: f32, hi: f32) -> f32 {
    if v.is_nan() {
        lo
    } else if v < lo {
        lo
    } else if v > hi {
        hi
    } else {
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn maxf(v: &[f32]) -> f32 {
        let mut m = f32::NEG_INFINITY;
        for &x in v {
            if x > m {
                m = x;
            }
        }
        m
    }

    fn minf(v: &[f32]) -> f32 {
        let mut m = f32::INFINITY;
        for &x in v {
            if x < m {
                m = x;
            }
        }
        m
    }

    #[test]
    fn prop_output_bounded() {
        let p = OrogenyParams::default();
        for seed in 0..16u64 {
            let e = simulate(24, 24, seed, p);
            for &v in &e {
                assert!(v >= 0.0 && v <= UPLIFT_MAX, "out of [0, MAX]: {v}");
            }
        }
    }

    #[test]
    fn prop_seed_deterministic() {
        let p = OrogenyParams::default();
        for seed in [1u64, 7, 42, 1000] {
            let a = simulate(20, 16, seed, p);
            let b = simulate(20, 16, seed, p);
            assert_eq!(a, b);
        }
    }

    #[test]
    fn prop_erosion_never_raises_peak() {
        let p = OrogenyParams::default();
        for seed in 0..16u64 {
            let up = uplift_field(28, 28, seed);
            let er = simulate(28, 28, seed, p);
            // Global peak after erosion never exceeds the uplift's peak.
            assert!(maxf(&er) <= maxf(&up) + 1e-3, "erosion raised the peak");
        }
    }

    #[test]
    fn prop_finite() {
        for seed in 0..16u64 {
            for &iters in &[0u32, 1, 8, 30] {
                let p = OrogenyParams { iterations: iters, ..OrogenyParams::default() };
                let e = simulate(16, 16, seed, p);
                for &v in &e {
                    assert!(v.is_finite());
                }
            }
        }
    }

    #[test]
    fn field_actually_varies() {
        // Sanity: the uplift field isn't constant (real relief to erode).
        let up = uplift_field(48, 48, 3);
        assert!(maxf(&up) - minf(&up) > 1.0);
    }

    #[test]
    fn degenerate_sizes_dont_panic() {
        let p = OrogenyParams::default();
        assert_eq!(simulate(0, 0, 1, p).len(), 0);
        assert_eq!(simulate(1, 1, 1, p).len(), 1);
        assert_eq!(simulate(1, 5, 1, p).len(), 5);
    }

    #[test]
    fn erosion_does_not_increase_global_max() {
        // More erosion never lifts the global maximum above the un-eroded one.
        let base = simulate(32, 32, 9, OrogenyParams { iterations: 0, ..OrogenyParams::default() });
        let eroded = simulate(32, 32, 9, OrogenyParams { iterations: 20, ..OrogenyParams::default() });
        assert!(maxf(&eroded) <= maxf(&base) + 1e-3);
    }
}
