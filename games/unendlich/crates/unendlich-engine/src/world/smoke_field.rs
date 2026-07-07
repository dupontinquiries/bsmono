//! `smoke_field` — smoke-puff density + dissipation over time.
//!
//! Worker-03 / WORLD-FX pure core, round 14. A cosmetic density field for a
//! rising smoke puff: the puff spreads (its radius grows with age), drifts
//! downwind (its peak advects by `|wind|·age`), and fades (overall intensity
//! decays with age). Render-side FP32 only — never feeds the deterministic sim,
//! but is still a pure deterministic function (no clock/rng).
//!
//! Self-contained for `rustc --test`.
#![allow(dead_code)]

const FADE_RATE: f32 = 0.3; // intensity decay per unit age
const BASE_RADIUS: f32 = 1.0;
const SPREAD: f32 = 1.5; // radius growth per unit age

/// Smoke density in [0,1] at a point `dist_from_source` away (radial), for a
/// puff of the given `age`, under a horizontal `wind`. Peaks at the advected
/// distance `|wind|·age`, spreads with age, and fades with age.
pub fn density(age: f32, dist_from_source: f32, wind: [f32; 2]) -> f32 {
    if age < 0.0 {
        return 0.0;
    }
    let intensity = (-age * FADE_RATE).exp(); // (0,1], monotone ↓ in age
    let radius = BASE_RADIUS + SPREAD * age; // grows with age
    let wind_speed = (wind[0] * wind[0] + wind[1] * wind[1]).sqrt();
    let peak = wind_speed * age; // downwind advection of the peak
    let d = dist_from_source - peak;
    let shape = (-(d * d) / (2.0 * radius * radius)).exp(); // gaussian, (0,1]
    clamp01(intensity * shape)
}

fn clamp01(v: f32) -> f32 {
    crate::util::mathx::clamp01(v) // r343 dedup
}

#[cfg(test)]
mod tests {
    use super::*;

    const NO_WIND: [f32; 2] = [0.0, 0.0];

    #[test]
    fn fresh_is_denser_than_old() {
        let fresh = density(0.1, 0.0, NO_WIND);
        let old = density(12.0, 0.0, NO_WIND);
        assert!(fresh > old);
    }

    #[test]
    fn monotone_fade_in_age() {
        // At the (windless) center the density equals the intensity → strictly
        // decreasing in age.
        let mut prev = f32::INFINITY;
        for a in 0..50 {
            let v = density(a as f32 * 0.5, 0.0, NO_WIND);
            assert!(v <= prev + 1e-6, "density rose with age");
            prev = v;
        }
    }

    #[test]
    fn spreads_with_age() {
        // The off-center/center density ratio rises as the puff widens.
        let off = 3.0;
        let ratio = |age: f32| density(age, off, NO_WIND) / density(age, 0.0, NO_WIND);
        assert!(ratio(5.0) > ratio(1.0), "puff did not spread with age");
    }

    #[test]
    fn wind_shifts_the_peak_downwind() {
        let wind = [2.0, 0.0];
        let age = 2.0; // peak advects to dist = 4
        let at_peak = density(age, 4.0, wind);
        let at_origin = density(age, 0.0, wind);
        assert!(at_peak > at_origin, "peak not shifted downwind");
    }

    #[test]
    fn in_unit_range() {
        for a in 0..20 {
            for d in 0..20 {
                let v = density(a as f32, d as f32, [1.0, 0.5]);
                assert!(v >= 0.0 && v <= 1.0);
            }
        }
    }

    #[test]
    fn deterministic() {
        assert_eq!(density(3.0, 2.0, [1.0, 1.0]), density(3.0, 2.0, [1.0, 1.0]));
    }
}
