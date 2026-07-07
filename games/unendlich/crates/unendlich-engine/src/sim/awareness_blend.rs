//! Combine sense channels into overall awareness in [0,1] via probabilistic OR
//! (1 − ∏(1−s)): any strong sense raises awareness and multiple senses stack. Pure.

pub fn awareness(sight: f32, hearing: f32, smell: f32) -> f32 {
    let s = sight.clamp(0.0, 1.0);
    let h = hearing.clamp(0.0, 1.0);
    let m = smell.clamp(0.0, 1.0);
    (1.0 - (1.0 - s) * (1.0 - h) * (1.0 - m)).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn all_zero_zero() { assert_eq!(awareness(0.0, 0.0, 0.0), 0.0); }
    #[test] fn any_full_one() { assert!((awareness(1.0, 0.0, 0.0) - 1.0).abs() < 1e-6); }
    #[test] fn senses_stack() { assert!(awareness(0.5, 0.5, 0.0) > 0.5); }
    #[test] fn bounded() {
        for &(a, b, c) in &[(0.0f32, 0.0f32, 0.0f32), (1.0, 1.0, 1.0), (0.3, 0.6, 0.2)] { let v = awareness(a, b, c); assert!((0.0..=1.0).contains(&v)); }
    }
    #[test] fn symmetric() { assert!((awareness(0.3, 0.6, 0.0) - awareness(0.6, 0.3, 0.0)).abs() < 1e-6); }
    #[test] fn deterministic() { assert_eq!(awareness(0.3, 0.6, 0.2).to_bits(), awareness(0.3, 0.6, 0.2).to_bits()); }
}
