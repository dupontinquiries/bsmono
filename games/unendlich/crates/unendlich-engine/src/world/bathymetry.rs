//! `bathymetry` — ocean floor profile + trenches.
//!
//! Worker-03 / WORLD-PERF pure core, round 3. The below-sea-level companion to
//! round 2's `orogeny`: a deterministic seabed-depth model with a continental
//! shelf, a continental slope, an abyssal plain, and seeded trenches. `orogeny`
//! builds the relief above sea level; this builds it below. The terrain
//! generator samples seabed depth as a function of distance from the coast.
//!
//! Pure & deterministic (only the seed varies); no clock/rng/unwrap.
//! Self-contained for standalone `rustc --test`.
//!
//! Depths are positive numbers measured downward from sea level.
#![allow(dead_code)]

/// Seabed profile tuning.
#[derive(Clone, Copy, Debug)]
pub struct BathyParams {
    /// Width (in distance units) of the gently-sloping continental shelf.
    pub shelf_width: f32,
    /// Depth reached at the shelf break.
    pub shelf_depth: f32,
    /// Width of the steep continental slope past the shelf.
    pub slope_width: f32,
    /// Depth of the abyssal plain (the flat deep beyond the slope).
    pub abyssal_depth: f32,
    /// Maximum extra depth a trench can add below the abyssal plain.
    pub trench_extra_max: f32,
    /// Noise above this [0,1] threshold becomes a trench.
    pub trench_threshold: f32,
    /// Spatial period of the trench noise lattice.
    pub trench_period: f32,
}

impl Default for BathyParams {
    fn default() -> Self {
        BathyParams {
            shelf_width: 40.0,
            shelf_depth: 30.0,
            slope_width: 30.0,
            abyssal_depth: 200.0,
            trench_extra_max: 120.0,
            trench_threshold: 0.82,
            trench_period: 53.0,
        }
    }
}

/// The base (trench-free) seabed depth at `dist_from_coast`. Monotonically
/// non-decreasing in distance and bounded to `[0, abyssal_depth]`: shelf →
/// slope → abyssal plain.
pub fn base_depth(dist_from_coast: f32, p: &BathyParams) -> f32 {
    let d = if dist_from_coast.is_nan() || dist_from_coast < 0.0 {
        0.0
    } else {
        dist_from_coast
    };
    let shelf_w = p.shelf_width.max(1e-3);
    let slope_w = p.slope_width.max(1e-3);
    if d < shelf_w {
        p.shelf_depth * (d / shelf_w)
    } else if d < shelf_w + slope_w {
        let t = (d - shelf_w) / slope_w;
        p.shelf_depth + (p.abyssal_depth - p.shelf_depth) * t
    } else {
        p.abyssal_depth
    }
}

/// Extra trench depth at `dist_from_coast`, in `[0, trench_extra_max]`. Zero on
/// the shelf and slope — trenches only cut the abyssal zone.
pub fn trench_extra(dist_from_coast: f32, seed: u64, p: &BathyParams) -> f32 {
    let d = if dist_from_coast.is_nan() || dist_from_coast < 0.0 {
        0.0
    } else {
        dist_from_coast
    };
    if d < p.shelf_width.max(1e-3) + p.slope_width.max(1e-3) {
        return 0.0;
    }
    let v = value_noise_1d(d, p.trench_period.max(1e-3), seed);
    let thr = clamp01(p.trench_threshold);
    if v > thr {
        let denom = (1.0 - thr).max(1e-3);
        let t = ((v - thr) / denom).min(1.0);
        t * p.trench_extra_max.max(0.0)
    } else {
        0.0
    }
}

/// Full seabed depth = base profile + any trench, bounded and finite.
pub fn seabed_depth(dist_from_coast: f32, seed: u64, p: &BathyParams) -> f32 {
    base_depth(dist_from_coast, p) + trench_extra(dist_from_coast, seed, p)
}

/// Seabed depth sampled along distances `0..n` from the coast.
pub fn profile(n: u32, seed: u64, p: &BathyParams) -> Vec<f32> {
    let mut v = Vec::with_capacity(n as usize);
    for d in 0..n {
        v.push(seabed_depth(d as f32, seed, p));
    }
    v
}

// --- deterministic 1D value noise ---

fn hash01(i: i64, seed: u64) -> f32 {
    let mut h = seed ^ (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
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

fn value_noise_1d(x: f32, period: f32, seed: u64) -> f32 {
    let fx = x / period;
    let x0 = fx.floor();
    let t = smoothstep(fx - x0);
    let i = x0 as i64;
    let a = hash01(i, seed);
    let b = hash01(i + 1, seed);
    a + (b - a) * t
}

fn clamp01(v: f32) -> f32 {
    crate::util::mathx::clamp01(v) // r343 dedup
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prop_base_depth_increases_away_from_coast() {
        let p = BathyParams::default();
        let mut prev = base_depth(0.0, &p);
        let mut d = 0.0f32;
        while d <= 200.0 {
            let cur = base_depth(d, &p);
            assert!(cur >= prev - 1e-4, "base depth not monotone at d={d}");
            prev = cur;
            d += 0.5;
        }
    }

    #[test]
    fn prop_base_depth_bounded() {
        let p = BathyParams::default();
        let mut d = 0.0f32;
        while d <= 500.0 {
            let b = base_depth(d, &p);
            assert!(b >= 0.0 && b <= p.abyssal_depth + 1e-4);
            d += 1.0;
        }
    }

    #[test]
    fn prop_trench_extra_bounded_and_only_in_abyss() {
        let p = BathyParams::default();
        for seed in 0..16u64 {
            let mut d = 0.0f32;
            while d <= 600.0 {
                let t = trench_extra(d, seed, &p);
                assert!(t >= 0.0 && t <= p.trench_extra_max + 1e-4);
                if d < p.shelf_width + p.slope_width {
                    assert_eq!(t, 0.0, "trench on shelf/slope at d={d}");
                }
                d += 1.0;
            }
        }
    }

    #[test]
    fn prop_seabed_depth_bounded_and_finite() {
        let p = BathyParams::default();
        let max_total = p.abyssal_depth + p.trench_extra_max;
        for seed in 0..16u64 {
            for d in 0..600u32 {
                let s = seabed_depth(d as f32, seed, &p);
                assert!(s.is_finite());
                assert!(s >= 0.0 && s <= max_total + 1e-4);
                // trench only deepens, never shallows below base
                assert!(s >= base_depth(d as f32, &p) - 1e-4);
            }
        }
    }

    #[test]
    fn prop_seed_deterministic() {
        let p = BathyParams::default();
        for seed in [1u64, 13, 777] {
            assert_eq!(profile(256, seed, &p), profile(256, seed, &p));
        }
    }

    #[test]
    fn trenches_actually_appear() {
        // Over a long abyssal stretch, at least one trench should form for a
        // typical seed (sanity that the gate isn't dead).
        let p = BathyParams::default();
        let prof = profile(2000, 42, &p);
        let any_trench = prof.iter().any(|&s| s > p.abyssal_depth + 1.0);
        assert!(any_trench);
    }

    #[test]
    fn degenerate_inputs_safe() {
        let p = BathyParams::default();
        assert!(seabed_depth(f32::NAN, 1, &p).is_finite());
        assert!(seabed_depth(-100.0, 1, &p) >= 0.0);
        assert_eq!(profile(0, 1, &p).len(), 0);
    }
}
