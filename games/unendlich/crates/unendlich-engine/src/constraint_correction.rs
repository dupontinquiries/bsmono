//! Distance-constraint correction (mass-spring/cloth): signed per-point move to
//! reach rest_length — negative pulls a stretched link together, positive pushes a
//! compressed one apart — scaled by stiffness. Pure, deterministic.

pub fn correction(current_length: f32, rest_length: f32, stiffness: f32) -> f32 {
    let cl = current_length.max(0.0);
    let rl = rest_length.max(0.0);
    let k = stiffness.clamp(0.0, 1.0);
    -0.5 * (cl - rl) * k
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn at_rest_zero() { assert_eq!(correction(10.0, 10.0, 1.0), 0.0); }
    #[test] fn stretched_pulls_in() { assert!(correction(12.0, 10.0, 1.0) < 0.0); }
    #[test] fn compressed_pushes_out() { assert!(correction(8.0, 10.0, 1.0) > 0.0); }
    #[test] fn stiffness_scales() { assert!(correction(12.0, 10.0, 1.0).abs() > correction(12.0, 10.0, 0.5).abs()); }
    #[test] fn half_of_error() { assert!((correction(12.0, 10.0, 1.0) + 1.0).abs() < 1e-6); }
    #[test] fn deterministic() { assert_eq!(correction(12.0, 10.0, 1.0).to_bits(), correction(12.0, 10.0, 1.0).to_bits()); }
}
