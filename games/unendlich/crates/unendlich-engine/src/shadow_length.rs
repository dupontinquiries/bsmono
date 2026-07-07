//! Shadow length factor from sun elevation (radians): long at low sun, ~0 at
//! zenith. ∝ 1/tan(elevation), with a low-sun floor to avoid blowup. Pure.

use std::f32::consts::FRAC_PI_2;

pub fn length(elevation: f32) -> f32 {
    let e = elevation.clamp(0.05, FRAC_PI_2);
    (1.0 / e.tan()).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn low_sun_long() { assert!(length(0.1) > length(1.0)); }
    #[test] fn zenith_short() { assert!(length(FRAC_PI_2) < 0.1); }
    #[test] fn lower_longer() {
        let (mut prev, mut e) = (f32::MAX, 0.1f32);
        while e < 1.4 { let l = length(e); assert!(l <= prev + 1e-4); prev = l; e += 0.05; }
    }
    #[test] fn capped_finite() { assert!(length(0.0).is_finite()); }
    #[test] fn positive() { assert!(length(0.5) > 0.0); }
    #[test] fn deterministic() { assert_eq!(length(0.5).to_bits(), length(0.5).to_bits()); }
}
