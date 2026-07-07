//! `beach_band` — sand band just above the water line.
//!
//! Worker-03 / WORLD pure core, round 18. Returns how "beachy" a cell is in
//! [0,1]: full sand right at the water line, fading to zero by `band` height
//! above it. Below the water line (submerged) and above the band it's not a
//! beach. Pairs with `sea_level` (R3) and biome/material selection.
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

/// Beach factor in [0,1] for terrain at `height` against `water` level, over a
/// sand `band` thickness above the water. 1 at the water line, linearly to 0 at
/// `water + band`; 0 below water and above the band.
pub fn is_beach(height: f32, water: f32, band: f32) -> f32 {
    let b = band.max(0.0);
    if b <= 0.0 {
        return if height == water { 1.0 } else { 0.0 };
    }
    let above = height - water;
    if above < 0.0 || above > b {
        return 0.0;
    }
    clamp01(1.0 - above / b)
}

fn clamp01(v: f32) -> f32 {
    crate::util::mathx::clamp01(v) // r343 dedup
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn at_water_is_full_beach() {
        assert!((is_beach(64.0, 64.0, 4.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn above_band_is_zero() {
        assert_eq!(is_beach(70.0, 64.0, 4.0), 0.0); // 6 above, band 4
    }

    #[test]
    fn below_water_is_zero() {
        assert_eq!(is_beach(62.0, 64.0, 4.0), 0.0); // submerged
    }

    #[test]
    fn band_width_scales() {
        // height 5 above water: in a band of 10 (0.5), out of a band of 4 (0).
        assert!(is_beach(69.0, 64.0, 10.0) > 0.0);
        assert_eq!(is_beach(69.0, 64.0, 4.0), 0.0);
    }

    #[test]
    fn bounded() {
        for h in 600..720 {
            let v = is_beach(h as f32 / 10.0, 64.0, 5.0);
            assert!(v >= 0.0 && v <= 1.0);
        }
    }

    #[test]
    fn deterministic() {
        assert_eq!(is_beach(66.0, 64.0, 4.0), is_beach(66.0, 64.0, 4.0));
    }
}
