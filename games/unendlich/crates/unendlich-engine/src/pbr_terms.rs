//! pbr_terms.rs — pure PBR helper curves/tables (photorealism groundwork).
//!
//! Generates lookup tables for the cheap analytic PBR terms the lighting pass
//! reuses: a Fresnel-Schlick reflectance curve and a GGX normal-distribution
//! curve. Pure math, deterministic, panic-free (R1/R11/R21). The shader samples
//! these; this module only produces the data.

use core::f32::consts::PI;

/// Fresnel-Schlick reflectance at `cos_theta` (angle between view and normal)
/// for base reflectance `f0`: `F = F0 + (1 - F0)*(1 - cos)^5`.
pub fn fresnel_schlick(cos_theta: f32, f0: f32) -> f32 {
    let c = cos_theta.clamp(0.0, 1.0);
    let f0 = f0.clamp(0.0, 1.0);
    let m = 1.0 - c;
    let m5 = m * m * m * m * m;
    (f0 + (1.0 - f0) * m5).clamp(0.0, 1.0)
}

/// Fresnel LUT over `cos_theta` in `[0, 1]` (`n` clamped to >= 2). Entry 0 is
/// grazing (`cos = 0`), the last entry is normal incidence (`cos = 1`).
/// Endpoints are pinned exactly: `lut[0] == 1.0`, `lut[n-1] == f0`. Monotonically
/// non-increasing across the table; every value lies in `[0, 1]`.
pub fn fresnel_lut(n: usize, f0: f32) -> Vec<f32> {
    let n = n.max(2);
    let f0c = f0.clamp(0.0, 1.0);
    let mut lut = Vec::with_capacity(n);
    for i in 0..n {
        let cos_theta = i as f32 / (n - 1) as f32;
        lut.push(fresnel_schlick(cos_theta, f0c));
    }
    lut[0] = 1.0; // grazing: full reflectance
    let last = n - 1;
    lut[last] = f0c; // normal incidence: base reflectance
    lut
}

/// GGX / Trowbridge-Reitz normal distribution term for half-vector angle with
/// cosine `n_dot_h` and roughness `alpha` (= roughness^2 typically).
pub fn ggx_ndf(n_dot_h: f32, alpha: f32) -> f32 {
    let a = alpha.clamp(1e-3, 1.0);
    let a2 = a * a;
    let c = n_dot_h.clamp(0.0, 1.0);
    let d = c * c * (a2 - 1.0) + 1.0;
    a2 / (PI * d * d)
}

/// GGX NDF sampled over `n_dot_h` in `[0, 1]` (`n` clamped to >= 2) at a fixed
/// `alpha`. All entries are finite and non-negative.
pub fn ggx_lut(n: usize, alpha: f32) -> Vec<f32> {
    let n = n.max(2);
    (0..n)
        .map(|i| ggx_ndf(i as f32 / (n - 1) as f32, alpha))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const F0S: &[f32] = &[0.02, 0.04, 0.2, 0.9, -1.0, 2.0];
    const SIZES: &[usize] = &[2, 3, 16, 64, 256];

    #[test]
    fn prop_fresnel_bounded_and_finite() {
        for &f0 in F0S {
            for &n in SIZES {
                for &v in &fresnel_lut(n, f0) {
                    assert!(v.is_finite() && v >= 0.0 && v <= 1.0, "fresnel oob {} f0={}", v, f0);
                }
            }
        }
    }

    #[test]
    fn prop_fresnel_endpoints_exact() {
        for &f0 in F0S {
            for &n in SIZES {
                let lut = fresnel_lut(n, f0);
                assert_eq!(lut[0], 1.0, "grazing not 1 (f0={}, n={})", f0, n);
                assert_eq!(lut[lut.len() - 1], f0.clamp(0.0, 1.0), "normal not f0 (f0={}, n={})", f0, n);
            }
        }
    }

    #[test]
    fn prop_fresnel_monotone_non_increasing() {
        // From grazing (cos=0, index 0) to normal (cos=1), reflectance falls.
        for &f0 in F0S {
            let lut = fresnel_lut(64, f0);
            for i in 1..lut.len() {
                assert!(lut[i] <= lut[i - 1] + 1e-6, "fresnel rose at i={} f0={}", i, f0);
            }
        }
    }

    #[test]
    fn prop_ggx_finite_nonnegative() {
        for &alpha in &[0.0f32, 0.1, 0.5, 1.0, 2.0] {
            for &n in SIZES {
                for &v in &ggx_lut(n, alpha) {
                    assert!(v.is_finite() && v >= 0.0, "ggx oob {} a={}", v, alpha);
                }
            }
        }
    }

    #[test]
    fn min_size_is_two() {
        assert_eq!(fresnel_lut(0, 0.04).len(), 2);
        assert_eq!(ggx_lut(1, 0.5).len(), 2);
    }
}
