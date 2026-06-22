//! `competition` — forest-competition growth factor (PLANT_GENETICS §3c).
//!
//! Worker-03 / WORLD-PERF pure core. Taller, less-shaded plants out-compete
//! their neighbors and grow faster; short, shaded ones grow slower. This is
//! the pure hook the growth tick multiplies into a plant's rate once the
//! neighbor broadphase (`neighbor_grid`) supplies each plant's local height
//! rank and shade. The §3c note marked it "not built" — this builds it.
//!
//! Pure & deterministic; clamps its inputs so out-of-range or NaN values are
//! safe; output is always finite and within `[MIN_FACTOR, MAX_FACTOR]`.
#![allow(dead_code)]

/// Slowest a fully-shaded, shortest-in-neighborhood plant grows.
pub const MIN_FACTOR: f32 = 0.5;
/// Fastest a full-sun, tallest-in-neighborhood plant grows.
pub const MAX_FACTOR: f32 = 1.5;

/// Growth multiplier from local competition.
///
/// * `height_rank` ∈ [0,1] — this plant's height relative to its local
///   neighborhood (0 = shortest, 1 = tallest).
/// * `shade` ∈ [0,1] — how shaded it is (0 = full sun, 1 = fully shaded).
///
/// Monotonically increasing in `height_rank`, decreasing in `shade`. Returns
/// a value in `[MIN_FACTOR, MAX_FACTOR]`. Inputs outside [0,1] (and NaN) are
/// clamped, so the result is always finite.
pub fn competition_factor(height_rank: f32, shade: f32) -> f32 {
    let hr = clamp01(height_rank);
    let sun = 1.0 - clamp01(shade);
    // Equal-weight blend of "is tall" and "is in sun", mapped onto the band.
    let advantage = 0.5 * hr + 0.5 * sun; // ∈ [0,1]
    MIN_FACTOR + (MAX_FACTOR - MIN_FACTOR) * advantage
}

fn clamp01(v: f32) -> f32 {
    crate::util::mathx::clamp01(v) // r343 dedup
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoints() {
        // tallest + full sun → fastest
        assert!((competition_factor(1.0, 0.0) - MAX_FACTOR).abs() < 1e-6);
        // shortest + full shade → slowest
        assert!((competition_factor(0.0, 1.0) - MIN_FACTOR).abs() < 1e-6);
        // neutral
        let mid = competition_factor(0.5, 0.5);
        assert!((mid - 0.5 * (MIN_FACTOR + MAX_FACTOR)).abs() < 1e-6);
    }

    #[test]
    fn prop_monotonic_increasing_in_height() {
        for shade_i in 0..=10 {
            let shade = shade_i as f32 / 10.0;
            let mut prev = competition_factor(0.0, shade);
            for hr_i in 1..=20 {
                let hr = hr_i as f32 / 20.0;
                let cur = competition_factor(hr, shade);
                assert!(cur >= prev - 1e-6, "not monotonic in height_rank");
                prev = cur;
            }
        }
    }

    #[test]
    fn prop_decreasing_in_shade() {
        for hr_i in 0..=10 {
            let hr = hr_i as f32 / 10.0;
            let mut prev = competition_factor(hr, 0.0);
            for sh_i in 1..=20 {
                let sh = sh_i as f32 / 20.0;
                let cur = competition_factor(hr, sh);
                assert!(cur <= prev + 1e-6, "not decreasing in shade");
                prev = cur;
            }
        }
    }

    #[test]
    fn prop_bounded_and_finite_incl_out_of_range() {
        let probes = [
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
            -5.0,
            -0.001,
            0.0,
            0.5,
            1.0,
            1.001,
            10.0,
        ];
        for &hr in &probes {
            for &sh in &probes {
                let f = competition_factor(hr, sh);
                assert!(f.is_finite(), "non-finite output for ({hr}, {sh})");
                assert!(
                    f >= MIN_FACTOR - 1e-6 && f <= MAX_FACTOR + 1e-6,
                    "out of band: {f} for ({hr}, {sh})"
                );
            }
        }
    }

    #[test]
    fn prop_out_of_range_clamps_like_endpoints() {
        // Below-zero / above-one behave like the clamped endpoints.
        assert!((competition_factor(5.0, -2.0) - MAX_FACTOR).abs() < 1e-6);
        assert!((competition_factor(-1.0, 9.0) - MIN_FACTOR).abs() < 1e-6);
    }
}
