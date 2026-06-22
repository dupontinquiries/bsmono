//! Hooke's-law restoring force: F = −stiffness·displacement. Stretched (positive
//! displacement) ⇒ negative pull-back force. Pure, deterministic.

pub fn force(displacement: f32, stiffness: f32) -> f32 {
    -stiffness.max(0.0) * displacement
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn rest_zero() { assert_eq!(force(0.0, 10.0), 0.0); }
    #[test] fn stretched_pulls_back() { assert!(force(2.0, 10.0) < 0.0); }
    #[test] fn compressed_pushes_out() { assert!(force(-2.0, 10.0) > 0.0); }
    #[test] fn stiffer_stronger() { assert!(force(2.0, 20.0).abs() > force(2.0, 10.0).abs()); }
    #[test] fn linear() { assert!((force(4.0, 10.0) - 2.0 * force(2.0, 10.0)).abs() < 1e-5); }
    #[test] fn deterministic() { assert_eq!(force(2.0, 10.0).to_bits(), force(2.0, 10.0).to_bits()); }
}
