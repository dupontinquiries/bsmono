//! Viscous damping force opposing motion: F = −damping·velocity. Pure.

pub fn force(velocity: f32, damping: f32) -> f32 {
    -damping.max(0.0) * velocity
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn rest_zero() { assert_eq!(force(0.0, 0.5), 0.0); }
    #[test] fn opposes_positive() { assert!(force(3.0, 0.5) < 0.0); }
    #[test] fn opposes_negative() { assert!(force(-3.0, 0.5) > 0.0); }
    #[test] fn more_damping_stronger() { assert!(force(3.0, 1.0).abs() > force(3.0, 0.5).abs()); }
    #[test] fn linear() { assert!((force(6.0, 0.5) - 2.0 * force(3.0, 0.5)).abs() < 1e-5); }
    #[test] fn deterministic() { assert_eq!(force(3.0, 0.5).to_bits(), force(3.0, 0.5).to_bits()); }
}
