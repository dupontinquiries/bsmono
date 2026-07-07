//! Radial-gauge sweep angle (0..2π) for a value fraction — e.g. an ability
//! cooldown clock. value/max clamped to [0,1] then ×2π. Pure, deterministic.

use std::f32::consts::PI;

pub fn sweep(value: f32, max: f32) -> f32 {
    if max <= 0.0 {
        return 0.0;
    }
    (value / max).clamp(0.0, 1.0) * 2.0 * PI
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn zero_no_sweep() { assert_eq!(sweep(0.0, 10.0), 0.0); }
    #[test] fn full_sweep() { assert!((sweep(10.0, 10.0) - 2.0 * PI).abs() < 1e-5); }
    #[test] fn half_sweep() { assert!((sweep(5.0, 10.0) - PI).abs() < 1e-5); }
    #[test] fn clamped() { assert!((sweep(20.0, 10.0) - 2.0 * PI).abs() < 1e-5); }
    #[test] fn zero_max_guarded() { assert_eq!(sweep(5.0, 0.0), 0.0); }
    #[test] fn deterministic() { assert_eq!(sweep(5.0, 10.0).to_bits(), sweep(5.0, 10.0).to_bits()); }
}
