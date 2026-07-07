//! Mie scattering phase (Henyey–Greenstein): (1−g²) / (4π·(1+g²−2g·cosθ)^1.5).
//! g ∈ (−1,1): forward-scattering haze for g>0. Pure, deterministic.

use std::f32::consts::PI;

pub fn phase(cos_theta: f32, g: f32) -> f32 {
    let c = cos_theta.clamp(-1.0, 1.0);
    let g = g.clamp(-0.99, 0.99);
    let num = 1.0 - g * g;
    let denom = 4.0 * PI * (1.0 + g * g - 2.0 * g * c).powf(1.5);
    (num / denom).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn isotropic_when_g_zero() {
        let a = phase(-0.5, 0.0);
        assert!((phase(0.5, 0.0) - a).abs() < 1e-7 && (phase(1.0, 0.0) - a).abs() < 1e-7);
    }
    #[test] fn forward_peak_for_positive_g() { assert!(phase(1.0, 0.6) > phase(-1.0, 0.6)); }
    #[test] fn backward_peak_for_negative_g() { assert!(phase(-1.0, -0.6) > phase(1.0, -0.6)); }
    #[test] fn positive() { assert!(phase(0.2, 0.5) > 0.0); }
    #[test] fn finite_near_extreme_g() { assert!(phase(1.0, 0.99).is_finite()); }
    #[test] fn deterministic() { assert_eq!(phase(0.5, 0.6).to_bits(), phase(0.5, 0.6).to_bits()); }
}
