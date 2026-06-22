//! Flat-ground range of a projectile launched at `angle` (radians) at `speed`:
//! R = v²·sin(2θ)/g, max at 45°. Pure, deterministic.

use std::f32::consts::PI;

pub fn range(speed: f32, angle: f32, gravity: f32) -> f32 {
    let v = speed.max(0.0);
    let g = gravity.max(1e-4);
    let a = angle.clamp(0.0, PI / 2.0);
    (v * v * (2.0 * a).sin() / g).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn zero_angle_zero() { assert_eq!(range(20.0, 0.0, 9.8), 0.0); }
    #[test] fn max_at_45() { assert!(range(20.0, PI / 4.0, 9.8) > range(20.0, PI / 6.0, 9.8) && range(20.0, PI / 4.0, 9.8) > range(20.0, PI / 3.0, 9.8)); }
    #[test] fn faster_farther() { assert!(range(40.0, PI / 4.0, 9.8) > range(20.0, PI / 4.0, 9.8)); }
    #[test] fn symmetric_about_45() { assert!((range(20.0, PI / 6.0, 9.8) - range(20.0, PI / 3.0, 9.8)).abs() < 1e-3); }
    #[test] fn more_gravity_shorter() { assert!(range(20.0, PI / 4.0, 20.0) < range(20.0, PI / 4.0, 9.8)); }
    #[test] fn deterministic() { assert_eq!(range(20.0, PI / 4.0, 9.8).to_bits(), range(20.0, PI / 4.0, 9.8).to_bits()); }
}
