//! `sim::sleep_cycle` — how active a creature is across the day.
//!
//! [`activity`] maps a time-of-day (`time01`, where `0` and `1` are midnight and
//! `0.5` is midday) to an activity level in `0..1`. A diurnal creature peaks at
//! midday and sleeps through the night; a nocturnal one is the mirror image,
//! peaking at midnight. The curve is a cosine, so it is smooth, periodic
//! (`activity(0) == activity(1)`), and seamless across the day boundary.
//!
//! Presentation/AI-tier and pure: one cosine — no clock, no randomness, no
//! cross-imports. `time01` wraps, so values outside `[0,1]` fold into the cycle.

use std::f32::consts::PI;

/// Activity level (`0..1`) at `time01` for a diurnal or nocturnal creature.
///
/// Diurnal: `0.5 - 0.5·cos(2π·t)` (peaks at midday). Nocturnal: the inverse
/// (peaks at midnight). `time01` is taken modulo 1, so the cycle is periodic.
pub fn activity(time01: f32, nocturnal: bool) -> f32 {
    let t = time01.rem_euclid(1.0);
    let diurnal = 0.5 - 0.5 * (2.0 * PI * t).cos();
    if nocturnal {
        1.0 - diurnal
    } else {
        diurnal
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }

    #[test]
    fn diurnal_peaks_at_midday() {
        let midday = activity(0.5, false);
        assert!((midday - 1.0).abs() < 1e-5, "midday {}", midday);
        // higher at midday than morning or night
        assert!(midday > activity(0.25, false));
        assert!(midday > activity(0.0, false));
    }

    #[test]
    fn nocturnal_is_inverted() {
        // nocturnal peaks at midnight, troughs at midday
        assert!((activity(0.0, true) - 1.0).abs() < 1e-5);
        assert!(activity(0.5, true) < 1e-5);
        // at any time the two sum to 1
        let mut seed = 0x5111_u32;
        for _ in 0..1000 {
            let t = (lcg(&mut seed) % 100_001) as f32 / 100_000.0;
            assert!((activity(t, false) + activity(t, true) - 1.0).abs() < 1e-5);
        }
    }

    #[test]
    fn bounded_unit_range() {
        let mut seed = 0xB0AD_u32;
        for _ in 0..4000 {
            let t = (lcg(&mut seed) % 400_001) as f32 / 100_000.0 - 2.0; // incl out-of-range
            let noct = lcg(&mut seed) & 1 == 0;
            let v = activity(t, noct);
            assert!(v >= 0.0 && v <= 1.0, "activity {}", v);
        }
    }

    #[test]
    fn periodic_at_boundaries() {
        assert!((activity(0.0, false) - activity(1.0, false)).abs() < 1e-6);
        assert!((activity(0.0, true) - activity(1.0, true)).abs() < 1e-6);
        // and wraps: t and t+1 agree
        assert!((activity(0.3, false) - activity(1.3, false)).abs() < 1e-5);
    }

    #[test]
    fn continuous_across_the_day_boundary() {
        let before = activity(0.999, false);
        let after = activity(0.001, false);
        assert!((after - before).abs() < 0.01, "discontinuity {} -> {}", before, after);
    }

    #[test]
    fn deterministic() {
        let mut seed = 0xD37E_u32;
        for _ in 0..3000 {
            let t = (lcg(&mut seed) % 100_001) as f32 / 100_000.0;
            let noct = lcg(&mut seed) & 1 == 0;
            assert_eq!(activity(t, noct), activity(t, noct));
        }
    }
}
