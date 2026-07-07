//! `sim::plant_root_system` — pure root growth, soil uptake, and anchorage.
//!
//! Roots deepen and spread with age, draw water from the local soil-moisture
//! value, and anchor the plant against wind/uprooting. The underground half of
//! the plant lifecycle: `germination` sprouts it, `canopy_field_shift` is the
//! canopy above, and this is the root below — feeding the moisture draw and the
//! wind-resistance the gameplay layer reads. Pure, deterministic, finite,
//! panic-free (R1/R6/R11); standalone `rustc --test`-able, release-safe.

/// Realised root parameters for a plant (all `>= 0`, finite).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct RootParams {
    /// Maximum rooting depth at full maturity (blocks).
    pub max_depth: f32,
    /// Maximum lateral spread radius at full maturity (blocks).
    pub spread: f32,
    /// Water uptake rate coefficient.
    pub uptake_rate: f32,
}

/// Genetic ceilings the genes scale.
const MAX_DEPTH_CEIL: f32 = 8.0;
const SPREAD_CEIL: f32 = 5.0;
const UPTAKE_BASE: f32 = 1.0;
/// Anchorage scale.
const ANCHOR_K: f32 = 1.0;

fn clamp01(v: f32) -> f32 {
    crate::util::mathx::saturate(v) // r343 dedup
}

impl Default for RootParams {
    /// A mid-of-the-road root system (genes 0.5, vigor 0.5).
    fn default() -> RootParams {
        params_from(0.5, 0.5, 0.5)
    }
}

/// Build root params from depth/spread genes and growth vigor (each `0..1`).
/// Vigor scales all three; out-of-range inputs are clamped.
pub fn params_from(depth_gene01: f32, spread_gene01: f32, vigor01: f32) -> RootParams {
    let d = clamp01(depth_gene01);
    let s = clamp01(spread_gene01);
    let v = clamp01(vigor01);
    let vigor_scale = 0.4 + 0.6 * v; // 0.4..1.0
    RootParams {
        max_depth: MAX_DEPTH_CEIL * d * vigor_scale,
        spread: SPREAD_CEIL * s * vigor_scale,
        uptake_rate: UPTAKE_BASE * (0.5 + 0.5 * v), // 0.5..1.0
    }
}

/// Current `(depth, radius)` reach for a plant of normalised age `age01`.
/// `(0, 0)` at age 0, `(max_depth, spread)` at age 1, monotone in age. Depth
/// leads early (taproot ∝ √age); spread tracks age linearly.
pub fn root_reach(p: &RootParams, age01: f32) -> (f32, f32) {
    let a = clamp01(age01);
    let depth = p.max_depth.max(0.0) * a.sqrt(); // faster early, hits max at a=1
    let radius = p.spread.max(0.0) * a;
    (depth, radius)
}

/// Water drawn this tick: proportional to soil moisture × root reach × rate × dt.
/// Zero in dry soil (`soil_moisture01 == 0`) or at age 0 (no reach). Finite, `>= 0`.
pub fn uptake(p: &RootParams, soil_moisture01: f32, age01: f32, dt: f32) -> f32 {
    let m = clamp01(soil_moisture01);
    let dt = if dt.is_finite() { dt.max(0.0) } else { 0.0 };
    let (depth, radius) = root_reach(p, age01);
    let reach = depth + radius; // total root extent
    (p.uptake_rate.max(0.0) * m * reach * dt).max(0.0)
}

/// Resistance to uprooting: rises with rooting depth and age (a deeper, older
/// plant holds firmer). `0` at age 0; monotone in depth and age. Finite, `>= 0`.
pub fn anchor_strength(p: &RootParams, age01: f32) -> f32 {
    let a = clamp01(age01);
    let (depth, _radius) = root_reach(p, age01);
    (ANCHOR_K * depth * a).max(0.0) // depth(a) ∝ √a, so anchor ∝ a^1.5 — monotone
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finite_nonneg(v: f32) -> bool {
        v.is_finite() && v >= 0.0
    }

    #[test]
    fn reach_grows_with_age_and_endpoints() {
        let p = params_from(1.0, 1.0, 1.0);
        let (d0, r0) = root_reach(&p, 0.0);
        assert_eq!((d0, r0), (0.0, 0.0), "zero reach at age 0");
        let (d1, r1) = root_reach(&p, 1.0);
        assert!((d1 - p.max_depth).abs() < 1e-5 && (r1 - p.spread).abs() < 1e-5, "full reach at age 1");
        // monotone in age
        let mut prev = (-1.0f32, -1.0f32);
        let mut a = 0.0f32;
        while a <= 1.0 {
            let (d, r) = root_reach(&p, a);
            assert!(d >= prev.0 - 1e-6 && r >= prev.1 - 1e-6, "reach not monotone at {}", a);
            prev = (d, r);
            a += 0.05;
        }
    }

    #[test]
    fn uptake_scales_with_moisture_and_reach_zero_when_dry() {
        let p = params_from(1.0, 1.0, 1.0);
        // more moisture ⇒ more uptake
        let lo = uptake(&p, 0.2, 0.5, 1.0);
        let hi = uptake(&p, 0.9, 0.5, 1.0);
        assert!(hi > lo, "uptake should rise with moisture");
        // more reach (older) ⇒ more uptake
        let young = uptake(&p, 0.5, 0.2, 1.0);
        let old = uptake(&p, 0.5, 0.9, 1.0);
        assert!(old > young, "uptake should rise with reach/age");
        // dry soil ⇒ zero; age 0 ⇒ zero
        assert_eq!(uptake(&p, 0.0, 0.5, 1.0), 0.0, "dry soil draws nothing");
        assert_eq!(uptake(&p, 0.8, 0.0, 1.0), 0.0, "no roots at age 0");
    }

    #[test]
    fn anchor_monotone_in_depth_and_age() {
        let shallow = params_from(0.2, 0.5, 0.5);
        let deep = params_from(1.0, 0.5, 0.5);
        // deeper roots anchor harder at the same age
        assert!(anchor_strength(&deep, 0.7) > anchor_strength(&shallow, 0.7), "deeper ⇒ stronger");
        // anchor rises with age, zero at age 0
        assert_eq!(anchor_strength(&deep, 0.0), 0.0);
        let mut prev = -1.0f32;
        let mut a = 0.0f32;
        while a <= 1.0 {
            let s = anchor_strength(&deep, a);
            assert!(s >= prev - 1e-6, "anchor not monotone in age at {}", a);
            prev = s;
            a += 0.05;
        }
    }

    #[test]
    fn params_from_clamps_and_vigor_scales() {
        // out-of-range genes clamped (no blow-up)
        let p = params_from(5.0, -3.0, 2.0);
        assert!(finite_nonneg(p.max_depth) && finite_nonneg(p.spread) && finite_nonneg(p.uptake_rate));
        assert!(p.max_depth <= MAX_DEPTH_CEIL + 1e-4 && p.spread <= SPREAD_CEIL + 1e-4);
        // vigor scales the system up
        let weak = params_from(1.0, 1.0, 0.0);
        let strong = params_from(1.0, 1.0, 1.0);
        assert!(strong.max_depth > weak.max_depth && strong.spread > weak.spread);
        assert!(strong.uptake_rate > weak.uptake_rate);
    }

    #[test]
    fn bounded_and_finite_over_random_inputs() {
        let mut s = 0x54FA_u64; // documented seed (R20)
        let mut rng = || {
            s = s.wrapping_mul(6364136223846793005).wrapping_add(1);
            (s >> 40) as f32 / ((1u64 << 24) as f32)
        };
        for _ in 0..4000 {
            // deliberately out-of-range / NaN-ish inputs
            let p = params_from(rng() * 4.0 - 2.0, rng() * 4.0 - 2.0, rng() * 4.0 - 2.0);
            assert!(finite_nonneg(p.max_depth) && finite_nonneg(p.spread) && finite_nonneg(p.uptake_rate));
            let age = rng() * 4.0 - 2.0;
            let (d, r) = root_reach(&p, age);
            assert!(finite_nonneg(d) && finite_nonneg(r));
            let u = uptake(&p, rng() * 4.0 - 2.0, age, rng() * 10.0 - 2.0);
            assert!(finite_nonneg(u), "uptake oob: {}", u);
            let an = anchor_strength(&p, age);
            assert!(finite_nonneg(an), "anchor oob: {}", an);
        }
        // explicit NaN inputs sanitise
        let p = params_from(f32::NAN, f32::INFINITY, f32::NAN);
        assert!(finite_nonneg(p.max_depth));
        assert!(finite_nonneg(uptake(&p, f32::NAN, f32::NAN, f32::NAN)));
        // Default is sane
        let d = RootParams::default();
        assert!(finite_nonneg(d.max_depth) && finite_nonneg(d.spread) && finite_nonneg(d.uptake_rate));
    }

    #[test]
    fn deterministic() {
        let p = params_from(0.6, 0.4, 0.7);
        assert_eq!(root_reach(&p, 0.5), root_reach(&p, 0.5));
        assert_eq!(uptake(&p, 0.5, 0.5, 0.1), uptake(&p, 0.5, 0.5, 0.1));
        assert_eq!(anchor_strength(&p, 0.5), anchor_strength(&p, 0.5));
    }
}
