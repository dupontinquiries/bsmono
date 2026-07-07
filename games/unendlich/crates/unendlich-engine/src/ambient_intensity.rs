//! Ambient light level over the day from sun elevation (radians): bright by day,
//! a small night floor below the horizon. In [0,1]. Pure, deterministic.

use std::f32::consts::FRAC_PI_2;

pub fn intensity(elevation: f32) -> f32 {
    let night_floor = 0.05;
    if elevation <= 0.0 {
        night_floor
    } else {
        let day = (elevation / FRAC_PI_2).clamp(0.0, 1.0);
        (night_floor + (1.0 - night_floor) * day).clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn night_floor() { assert!((intensity(-0.5) - 0.05).abs() < 1e-6); }
    #[test] fn zenith_full() { assert!((intensity(FRAC_PI_2) - 1.0).abs() < 1e-5); }
    #[test] fn dawn_floor() { assert!((intensity(0.0) - 0.05).abs() < 1e-6); }
    #[test] fn higher_brighter() { assert!(intensity(1.0) > intensity(0.3)); }
    #[test] fn bounded() {
        let mut e = -1.0f32;
        while e < 2.0 { assert!((0.0..=1.0).contains(&intensity(e))); e += 0.1; }
    }
    #[test] fn deterministic() { assert_eq!(intensity(0.5).to_bits(), intensity(0.5).to_bits()); }
}
