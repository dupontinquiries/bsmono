//! GGX / Trowbridge–Reitz normal distribution function. n_dot_h = N·H,
//! roughness in [0,1] (clamped away from 0). Pure, deterministic.

use std::f32::consts::PI;

pub fn ndf(n_dot_h: f32, roughness: f32) -> f32 {
    let nh = n_dot_h.clamp(0.0, 1.0);
    let r = roughness.clamp(1e-3, 1.0);
    let a = r * r;
    let a2 = a * a;
    let d = nh * nh * (a2 - 1.0) + 1.0;
    a2 / (PI * d * d).max(1e-12)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn peak_at_aligned() { assert!(ndf(1.0, 0.3) > ndf(0.5, 0.3)); }
    #[test] fn sharper_when_smooth() { assert!(ndf(1.0, 0.1) > ndf(1.0, 0.5)); }
    #[test] fn positive() {
        let mut nh = 0.0f32;
        while nh <= 1.0 { assert!(ndf(nh, 0.4) > 0.0); nh += 0.05; }
    }
    #[test] fn finite_at_zero_roughness() { assert!(ndf(1.0, 0.0).is_finite()); }
    #[test] fn finite() { assert!(ndf(0.7, 0.3).is_finite()); }
    #[test] fn deterministic() { assert_eq!(ndf(0.7, 0.3).to_bits(), ndf(0.7, 0.3).to_bits()); }
}
