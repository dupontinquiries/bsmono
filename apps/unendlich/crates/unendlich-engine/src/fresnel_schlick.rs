//! Schlick Fresnel approximation: F0 + (1−F0)(1−cosθ)^5. cos_theta = N·V.
//! Reflectance rises toward grazing angles. Pure, deterministic.

pub fn fresnel(cos_theta: f32, f0: f32) -> f32 {
    let c = crate::util::mathx::clampf(cos_theta, 0.0, 1.0);
    let f0 = crate::util::mathx::clampf(f0, 0.0, 1.0);
    let m = 1.0 - c;
    let m5 = m * m * m * m * m;
    crate::util::mathx::clampf(f0 + (1.0 - f0) * m5, 0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn head_on_is_f0() { assert!((fresnel(1.0, 0.04) - 0.04).abs() < 1e-6); }
    #[test] fn grazing_is_one() { assert!((fresnel(0.0, 0.04) - 1.0).abs() < 1e-6); }
    #[test] fn monotone_toward_grazing() {
        let (mut prev, mut c) = (f32::MAX, 0.0f32);
        while c <= 1.0 { let f = fresnel(c, 0.04); assert!(f <= prev + 1e-6); prev = f; c += 0.05; }
    }
    #[test] fn bounded() {
        let mut c = 0.0f32;
        while c <= 1.0 { assert!((0.0..=1.0).contains(&fresnel(c, 0.5))); c += 0.05; }
    }
    #[test] fn f0_one_always_one() { assert!((fresnel(0.5, 1.0) - 1.0).abs() < 1e-6); }
    #[test] fn deterministic() { assert_eq!(fresnel(0.5, 0.04).to_bits(), fresnel(0.5, 0.04).to_bits()); }
}
