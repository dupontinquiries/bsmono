//! Sky/sun warmth by sun elevation (radians, 0 = horizon, π/2 = zenith): 1.0 warm
//! (low sun, sunset orange) → 0.0 neutral (high sun). Pure, deterministic.

use std::f32::consts::FRAC_PI_2;

pub fn warmth(elevation: f32) -> f32 {
    let e = elevation.clamp(0.0, FRAC_PI_2);
    (1.0 - e / FRAC_PI_2).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn horizon_warm() { assert!((warmth(0.0) - 1.0).abs() < 1e-6); }
    #[test] fn zenith_neutral() { assert!(warmth(FRAC_PI_2) < 1e-6); }
    #[test] fn mid_between() { let w = warmth(FRAC_PI_2 * 0.5); assert!(w > 0.0 && w < 1.0); }
    #[test] fn bounded() {
        let mut e = -0.5f32;
        while e < 2.0 { assert!((0.0..=1.0).contains(&warmth(e))); e += 0.1; }
    }
    #[test] fn monotone_decreasing() {
        let (mut prev, mut e) = (f32::MAX, 0.0f32);
        while e <= FRAC_PI_2 { let w = warmth(e); assert!(w <= prev + 1e-6); prev = w; e += 0.1; }
    }
    #[test] fn deterministic() { assert_eq!(warmth(0.5).to_bits(), warmth(0.5).to_bits()); }
}
