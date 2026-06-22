//! Screen-shake offset from trauma in [0,1]: amplitude ∝ trauma² (small trauma
//! stays subtle), direction from a deterministic hash of (t, seed). Returns
//! (x,y) each in [-trauma², trauma²]. Pure, deterministic.

pub fn offset(trauma: f32, t: f32, seed: u32) -> [f32; 2] {
    let amp = trauma.clamp(0.0, 1.0).powi(2);
    let h = |k: u32| -> f32 {
        let mut x = t.to_bits()
            .wrapping_mul(2654435761)
            .wrapping_add(seed.wrapping_mul(40503))
            .wrapping_add(k.wrapping_mul(2246822519));
        x ^= x >> 15;
        x = x.wrapping_mul(2246822519);
        x ^= x >> 13;
        (x as f32 / u32::MAX as f32) * 2.0 - 1.0
    };
    [amp * h(1), amp * h(2)]
}

#[cfg(test)]
mod tests {
    use super::*;
    fn mag(v: [f32; 2]) -> f32 { (v[0] * v[0] + v[1] * v[1]).sqrt() }
    #[test] fn zero_trauma_no_shake() { assert_eq!(offset(0.0, 1.0, 42), [0.0, 0.0]); }
    #[test] fn more_trauma_bigger() { assert!(mag(offset(0.9, 1.0, 42)) > mag(offset(0.3, 1.0, 42))); }
    #[test] fn deterministic() { assert_eq!(offset(0.5, 1.0, 42), offset(0.5, 1.0, 42)); }
    #[test] fn bounded() { let o = offset(1.0, 3.7, 7); assert!(o[0].abs() <= 1.0 + 1e-6 && o[1].abs() <= 1.0 + 1e-6); }
    #[test] fn quadratic_falloff() { assert!((mag(offset(0.5, 1.0, 42)) - 0.25 * mag(offset(1.0, 1.0, 42))).abs() < 1e-5); }
    #[test] fn seed_differs() { assert_ne!(offset(0.5, 1.0, 1), offset(0.5, 1.0, 2)); }
}
