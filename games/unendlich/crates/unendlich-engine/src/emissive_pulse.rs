//! Pulsing emissive intensity for glow (dragon glow, lava breathing). Oscillates
//! between `base` and base·(1+depth). Always >= 0.

use std::f32::consts::PI;

pub fn pulse(base: f32, t: f32, freq: f32, depth: f32) -> f32 {
    let b = base.max(0.0);
    let d = depth.max(0.0);
    let s = 0.5 * (1.0 + (2.0 * PI * freq * t).sin());
    (b * (1.0 + d * s)).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn depth_zero_is_constant() {
        let mut t = 0.0f32;
        while t < 3.0 { assert!((pulse(2.0, t, 1.0, 0.0) - 2.0).abs() < 1e-6); t += 0.2; }
    }
    #[test] fn within_range() {
        let mut t = 0.0f32;
        while t < 3.0 { let p = pulse(2.0, t, 1.0, 0.5); assert!((2.0 - 1e-4..=3.0 + 1e-4).contains(&p)); t += 0.1; }
    }
    #[test] fn periodic() { assert!((pulse(2.0, 0.0, 1.0, 0.5) - pulse(2.0, 1.0, 1.0, 0.5)).abs() < 1e-4); }
    #[test] fn base_scales() {
        let mut t = 0.0f32;
        while t < 2.0 { assert!(pulse(4.0, t, 1.0, 0.5) >= pulse(2.0, t, 1.0, 0.5)); t += 0.25; }
    }
    #[test] fn nonneg() { assert!(pulse(0.0, 1.3, 2.0, 5.0) >= 0.0); }
    #[test] fn deterministic() { assert_eq!(pulse(2.0, 0.7, 1.0, 0.5).to_bits(), pulse(2.0, 0.7, 1.0, 0.5).to_bits()); }
}
