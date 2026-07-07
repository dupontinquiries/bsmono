//! `sim::plant_growth_env` — how the environment scales a plant's growth rate.
//!
//! `sim::plant_growth` advances a plant through stages but treats the
//! environment abstractly; this module is the **environmental modulation** that
//! sits in front of it. Given the local [`Conditions`] (moisture, light,
//! temperature, season) and a plant's [`Tolerance`], it returns a growth-rate
//! multiplier that peaks near the plant's optimum and falls to ~0 in
//! drought / freeze / dark, plus a [`should_wilt`] predicate for genuinely bad
//! conditions and an [`effective_rate`] wrapper.
//!
//! Composition (does **not** reimplement stages): feed [`effective_rate`] as the
//! per-tick `base_rate` into `plant_growth::advance`. `Tolerance` is derived from
//! `plant_tree_genetics` (hardiness / season bias); `moisture01` comes from
//! `water_physics`, `light01` from `light_physics`.
//!
//! The multiplier is the smooth product of four factors, scaled to a small
//! super-unity bonus at the optimum:
//! - **temperature**: a smooth bump `1 − d²` peaking at `temp_opt`, hitting 0 at
//!   `temp_lo`/`temp_hi`;
//! - **moisture**: rises with `moisture01`, lifted in the dry range by
//!   `drought_resist01`;
//! - **light**: rises with `light01` (0 in the dark);
//! - **season**: a gentle `sin` over the year (floored, never 0).
//!
//! Pure, deterministic, finite (guards non-finite inputs), panic-free. No
//! `crate::render`, no `ash`/`vk`, no IO, no clock, no RNG, no `.unwrap()`/
//! `.expect()` in non-test code; compiles clean under `cargo check --release`
//! (no `cfg(test)`/`cfg(debug_assertions)` gated helpers).

use std::f32::consts::PI;

/// Local environmental conditions at the plant. `moisture01`/`light01`/`season01`
/// in `0..1` (clamped); `temp_c` in degrees Celsius.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Conditions {
    pub moisture01: f32,
    pub light01: f32,
    pub temp_c: f32,
    pub season01: f32,
}

/// A plant's environmental tolerance. Growth is possible in `(temp_lo, temp_hi)`
/// and best at `temp_opt`; `drought_resist01` (`0..1`) softens low moisture.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tolerance {
    pub temp_lo: f32,
    pub temp_hi: f32,
    pub temp_opt: f32,
    pub drought_resist01: f32,
}

/// Peak multiplier at the optimum (a small super-unity bonus).
pub const MAX_MULT: f32 = 1.3;
/// Season factor floor (winter never fully stops growth on its own).
const SEASON_FLOOR: f32 = 0.6;
/// How strongly drought resistance lifts the dry-moisture factor.
const DROUGHT_LIFT: f32 = 0.7;
/// Moisture below `WILT_M·(1−drought_resist)` is a wilting condition.
const WILT_M: f32 = 0.15;

fn clamp01(v: f32) -> f32 {
    crate::util::mathx::saturate(v) // r343 dedup
}

/// Smooth temperature bump: 1 at `opt`, 0 at/outside `[lo, hi]`. Finite and in
/// `[0, 1]` for any inputs (non-finite ⇒ 0).
fn temp_factor(temp: f32, lo: f32, hi: f32, opt: f32) -> f32 {
    if !temp.is_finite() || !lo.is_finite() || !hi.is_finite() || !opt.is_finite() {
        return 0.0;
    }
    if hi <= lo || temp <= lo || temp >= hi {
        return 0.0;
    }
    let opt = opt.max(lo).min(hi); // keep optimum inside the band
    let span = if temp < opt { opt - lo } else { hi - opt };
    let span = if span > 1e-6 { span } else { 1e-6 };
    let d = ((temp - opt).abs() / span).min(1.0);
    (1.0 - d * d).max(0.0)
}

/// Growth-rate multiplier in `[0, MAX_MULT]` from conditions vs tolerance.
/// Maximal at `temp_opt` + full light + full moisture (+ peak season); ~0 in
/// drought, freeze/overheat, or darkness. Smooth and finite.
pub fn growth_multiplier(c: Conditions, t: Tolerance) -> f32 {
    let tf = temp_factor(c.temp_c, t.temp_lo, t.temp_hi, t.temp_opt);

    let m = clamp01(c.moisture01);
    let dr = clamp01(t.drought_resist01);
    // moisture factor: =m normally, lifted toward 1 in the dry range by drought
    // resistance; monotone in both m and dr.
    let moisture_factor = clamp01(m + dr * (1.0 - m) * DROUGHT_LIFT);

    let light_factor = clamp01(c.light01);

    let season = clamp01(c.season01);
    let season_factor = SEASON_FLOOR + (1.0 - SEASON_FLOOR) * (PI * season).sin().max(0.0);

    let mult = MAX_MULT * tf * moisture_factor * light_factor * season_factor;
    // already in range, but clamp for safety/finiteness
    if mult.is_finite() {
        mult.max(0.0).min(MAX_MULT)
    } else {
        0.0
    }
}

/// True when conditions are bad enough that the plant should wilt: out of the
/// temperature band (freeze/overheat) or moisture below the drought-resistance-
/// softened wilt threshold.
pub fn should_wilt(c: Conditions, t: Tolerance) -> bool {
    let temp_bad = !c.temp_c.is_finite() || c.temp_c <= t.temp_lo || c.temp_c >= t.temp_hi;
    let m = clamp01(c.moisture01);
    let dr = clamp01(t.drought_resist01);
    let threshold = WILT_M * (1.0 - dr); // dr=1 ⇒ never wilts from dryness
    let moisture_bad = m < threshold;
    temp_bad || moisture_bad
}

/// Env-scaled growth per tick: `base_rate · growth_multiplier`, never negative.
/// Monotone non-decreasing in `base_rate` (the multiplier is `>= 0`).
pub fn effective_rate(base_rate: f32, c: Conditions, t: Tolerance) -> f32 {
    let r = base_rate * growth_multiplier(c, t);
    if r.is_finite() && r > 0.0 {
        r
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }
    fn randf(seed: &mut u32, lo: f32, hi: f32) -> f32 {
        lo + (lcg(seed) % 1_000_001) as f32 / 1_000_000.0 * (hi - lo)
    }
    fn tol() -> Tolerance {
        Tolerance { temp_lo: 5.0, temp_hi: 40.0, temp_opt: 22.0, drought_resist01: 0.2 }
    }
    fn optimal() -> Conditions {
        Conditions { moisture01: 1.0, light01: 1.0, temp_c: 22.0, season01: 0.5 }
    }

    #[test]
    fn multiplier_peaks_at_optimal() {
        let t = tol();
        let peak = growth_multiplier(optimal(), t);
        assert!((peak - MAX_MULT).abs() < 1e-4, "peak should be ~{}, got {}", MAX_MULT, peak);

        // no perturbation exceeds the optimum
        let mut s = 0x9E14_0000u32;
        for _ in 0..20000 {
            let c = Conditions {
                moisture01: randf(&mut s, 0.0, 1.0),
                light01: randf(&mut s, 0.0, 1.0),
                temp_c: randf(&mut s, -20.0, 60.0),
                season01: randf(&mut s, 0.0, 1.0),
            };
            assert!(growth_multiplier(c, t) <= peak + 1e-4, "exceeded optimum");
        }
    }

    #[test]
    fn multiplier_zero_at_extremes() {
        let t = tol();
        // freeze (below lo)
        assert_eq!(growth_multiplier(Conditions { temp_c: -10.0, ..optimal() }, t), 0.0);
        // overheat (above hi)
        assert_eq!(growth_multiplier(Conditions { temp_c: 60.0, ..optimal() }, t), 0.0);
        // dark
        assert_eq!(growth_multiplier(Conditions { light01: 0.0, ..optimal() }, t), 0.0);
        // bone dry with no drought resistance
        let t0 = Tolerance { drought_resist01: 0.0, ..t };
        assert_eq!(growth_multiplier(Conditions { moisture01: 0.0, ..optimal() }, t0), 0.0);
    }

    #[test]
    fn multiplier_stays_in_band() {
        let mut s = 0x9E14_0001u32;
        for _ in 0..50000 {
            let c = Conditions {
                moisture01: randf(&mut s, -0.5, 1.5),
                light01: randf(&mut s, -0.5, 1.5),
                temp_c: randf(&mut s, -50.0, 80.0),
                season01: randf(&mut s, -0.5, 1.5),
            };
            let t = Tolerance {
                temp_lo: randf(&mut s, -10.0, 10.0),
                temp_hi: randf(&mut s, 30.0, 50.0),
                temp_opt: randf(&mut s, 10.0, 30.0),
                drought_resist01: randf(&mut s, 0.0, 1.0),
            };
            let mult = growth_multiplier(c, t);
            assert!(mult.is_finite() && mult >= 0.0 && mult <= MAX_MULT + 1e-5, "out of band: {}", mult);
        }
    }

    #[test]
    fn drought_resistance_raises_dry_multiplier() {
        let t_lo = Tolerance { drought_resist01: 0.0, ..tol() };
        let t_hi = Tolerance { drought_resist01: 0.9, ..tol() };
        let dry = Conditions { moisture01: 0.05, ..optimal() };
        let m_lo = growth_multiplier(dry, t_lo);
        let m_hi = growth_multiplier(dry, t_hi);
        assert!(m_hi > m_lo, "drought resist should raise dry multiplier: {} !> {}", m_hi, m_lo);
    }

    #[test]
    fn should_wilt_only_when_bad() {
        let t = tol();
        // good conditions: no wilt
        assert!(!should_wilt(optimal(), t));
        assert!(!should_wilt(Conditions { moisture01: 0.5, ..optimal() }, t));
        // freeze / overheat: wilt
        assert!(should_wilt(Conditions { temp_c: -5.0, ..optimal() }, t));
        assert!(should_wilt(Conditions { temp_c: 45.0, ..optimal() }, t));
        // bone dry, no resistance: wilt
        let t0 = Tolerance { drought_resist01: 0.0, ..t };
        assert!(should_wilt(Conditions { moisture01: 0.0, ..optimal() }, t0));
        // very dry but drought-resistant: survives
        let t1 = Tolerance { drought_resist01: 1.0, ..t };
        assert!(!should_wilt(Conditions { moisture01: 0.02, ..optimal() }, t1));
    }

    #[test]
    fn effective_rate_monotone_and_nonnegative() {
        let t = tol();
        let c = optimal();
        let a = effective_rate(1.0, c, t);
        let b = effective_rate(2.0, c, t);
        let d = effective_rate(5.0, c, t);
        assert!(a <= b && b <= d, "monotone in base_rate");
        assert!(a > 0.0, "positive base & good conditions ⇒ positive rate");
        // never negative, even with a negative base_rate
        assert_eq!(effective_rate(-3.0, c, t), 0.0);

        let mut s = 0x9E14_0002u32;
        for _ in 0..20000 {
            let base = randf(&mut s, -5.0, 5.0);
            let cc = Conditions {
                moisture01: randf(&mut s, 0.0, 1.0),
                light01: randf(&mut s, 0.0, 1.0),
                temp_c: randf(&mut s, -20.0, 60.0),
                season01: randf(&mut s, 0.0, 1.0),
            };
            let r = effective_rate(base, cc, t);
            assert!(r.is_finite() && r >= 0.0);
        }
    }

    #[test]
    fn finite_at_extremes_no_panic() {
        let weird_c = Conditions {
            moisture01: f32::NAN,
            light01: f32::INFINITY,
            temp_c: f32::NAN,
            season01: -1e9,
        };
        let weird_t = Tolerance {
            temp_lo: f32::NAN,
            temp_hi: f32::INFINITY,
            temp_opt: f32::NEG_INFINITY,
            drought_resist01: f32::NAN,
        };
        let m = growth_multiplier(weird_c, weird_t);
        assert!(m.is_finite() && m >= 0.0 && m <= MAX_MULT);
        let _ = should_wilt(weird_c, weird_t); // must not panic
        let r = effective_rate(f32::NAN, weird_c, weird_t);
        assert!(r.is_finite() && r >= 0.0);
    }

    #[test]
    fn deterministic() {
        let mut s = 0x9E14_0003u32;
        for _ in 0..5000 {
            let c = Conditions {
                moisture01: randf(&mut s, 0.0, 1.0),
                light01: randf(&mut s, 0.0, 1.0),
                temp_c: randf(&mut s, -20.0, 60.0),
                season01: randf(&mut s, 0.0, 1.0),
            };
            let t = Tolerance {
                temp_lo: 5.0,
                temp_hi: 40.0,
                temp_opt: randf(&mut s, 10.0, 30.0),
                drought_resist01: randf(&mut s, 0.0, 1.0),
            };
            assert_eq!(growth_multiplier(c, t), growth_multiplier(c, t));
            assert_eq!(should_wilt(c, t), should_wilt(c, t));
            assert_eq!(effective_rate(1.5, c, t), effective_rate(1.5, c, t));
        }
    }
}
