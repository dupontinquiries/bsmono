//! `sim::migrate` — which way a creature wants to migrate this season.
//!
//! [`migrate_dir`] returns a 2D direction from the time of year (`season01`,
//! `0`/`1` = deep winter, `0.5` = midsummer) and the creature's `warm_pref`
//! (`0` = indifferent .. `1` = strongly heat-seeking). A warmth-loving creature
//! heads toward warmth (`-y`, "toward the equator") in winter and reverses to
//! cooler latitudes (`+y`) in summer; at the equinoxes the urge passes through
//! zero. The magnitude scales with `warm_pref`, so an indifferent creature never
//! moves, and the direction is smooth and periodic across the year.
//!
//! Pure and deterministic: one cosine — no clock, no randomness, no
//! cross-imports. Magnitude is bounded by `warm_pref` (≤ 1).

use std::f32::consts::PI;

fn clamp01(x: f32) -> f32 {
    let x = if x > 0.0 { x } else { 0.0 };
    if x < 1.0 {
        x
    } else {
        1.0
    }
}

/// Seasonal migration direction `[x, y]`.
///
/// `y = -warm_pref · cos(2π·season01)` (toward warmth `-y` in winter, away in
/// summer); `x` is `0` (migration runs along the warmth axis). Magnitude `≤
/// warm_pref`.
pub fn migrate_dir(season01: f32, warm_pref: f32) -> [f32; 2] {
    let pref = clamp01(warm_pref);
    let t = season01.rem_euclid(1.0);
    let y = -pref * (2.0 * PI * t).cos();
    [0.0, y]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }
    fn mag(v: [f32; 2]) -> f32 {
        (v[0] * v[0] + v[1] * v[1]).sqrt()
    }

    #[test]
    fn warm_pref_heads_to_warmth_in_winter() {
        // deep winter (season 0): head toward warmth = -y
        let d = migrate_dir(0.0, 1.0);
        assert!(d[1] < 0.0, "should head -y toward warmth, got {:?}", d);
    }

    #[test]
    fn reverses_in_summer() {
        // midsummer (season 0.5): head to cooler latitudes = +y
        let d = migrate_dir(0.5, 1.0);
        assert!(d[1] > 0.0, "should reverse to +y, got {:?}", d);
    }

    #[test]
    fn magnitude_bounded() {
        let mut seed = 0xB0AD_u32;
        for _ in 0..5000 {
            let s = (lcg(&mut seed) % 400_001) as f32 / 100_000.0 - 2.0; // incl out-of-range
            let pref = (lcg(&mut seed) % 300_001) as f32 / 100_000.0 - 1.0; // incl <0,>1
            let d = migrate_dir(s, pref);
            assert!(mag(d) <= 1.0 + 1e-5, "magnitude {}", mag(d));
        }
    }

    #[test]
    fn continuous_and_periodic() {
        // periodic across the year boundary
        assert!((migrate_dir(0.0, 1.0)[1] - migrate_dir(1.0, 1.0)[1]).abs() < 1e-5);
        // continuous: nearby seasons give nearby directions
        let a = migrate_dir(0.30, 0.8);
        let b = migrate_dir(0.31, 0.8);
        assert!((a[1] - b[1]).abs() < 0.05);
    }

    #[test]
    fn pref_zero_is_neutral() {
        let mut seed = 0x0001_u32;
        for _ in 0..1000 {
            let s = (lcg(&mut seed) % 100_001) as f32 / 100_000.0;
            assert_eq!(migrate_dir(s, 0.0), [0.0, 0.0]);
        }
    }

    #[test]
    fn deterministic() {
        let mut seed = 0xD37E_u32;
        for _ in 0..3000 {
            let s = (lcg(&mut seed) % 100_001) as f32 / 100_000.0;
            let pref = (lcg(&mut seed) % 100_001) as f32 / 100_000.0;
            assert_eq!(migrate_dir(s, pref), migrate_dir(s, pref));
        }
    }
}
