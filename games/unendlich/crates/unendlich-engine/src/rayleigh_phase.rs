//! Rayleigh scattering phase function: (3/16π)(1 + cos²θ). `cos_theta` is the
//! cosine of the scattering angle. Symmetric; min perpendicular, max along axis.

use std::f32::consts::PI;

pub fn phase(cos_theta: f32) -> f32 {
    let c = cos_theta.clamp(-1.0, 1.0);
    (3.0 / (16.0 * PI)) * (1.0 + c * c)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn symmetric() { assert!((phase(0.5) - phase(-0.5)).abs() < 1e-7); }
    #[test] fn max_along_axis() { assert!(phase(1.0) > phase(0.3)); }
    #[test] fn min_perpendicular() { assert!(phase(0.0) <= phase(0.7) && phase(0.0) <= phase(1.0)); }
    #[test] fn positive() {
        let mut c = -1.0f32;
        while c <= 1.0 { assert!(phase(c) > 0.0); c += 0.1; }
    }
    #[test] fn bounded() { assert!(phase(1.0) <= (3.0 / (16.0 * PI)) * 2.0 + 1e-6); }
    #[test] fn deterministic() { assert_eq!(phase(0.5).to_bits(), phase(0.5).to_bits()); }
}
