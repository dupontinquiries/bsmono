//! Hanging rope/chain sag depth at `span_fraction` in [0,1] (0,1 = anchors): a
//! parabolic approximation 4x(1−x)·sag_depth, deepest at mid-span. Pure.

pub fn sag(span_fraction: f32, sag_depth: f32) -> f32 {
    let x = span_fraction.clamp(0.0, 1.0);
    let d = sag_depth.max(0.0);
    (4.0 * x * (1.0 - x) * d).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn anchors_zero() { assert_eq!(sag(0.0, 3.0), 0.0); assert_eq!(sag(1.0, 3.0), 0.0); }
    #[test] fn mid_deepest() { assert!((sag(0.5, 3.0) - 3.0).abs() < 1e-5); }
    #[test] fn symmetric() { assert!((sag(0.25, 3.0) - sag(0.75, 3.0)).abs() < 1e-6); }
    #[test] fn deeper_param_more() { assert!(sag(0.5, 5.0) > sag(0.5, 2.0)); }
    #[test] fn bounded() { let mut x = 0.0f32; while x <= 1.0 { assert!((0.0..=3.0).contains(&sag(x, 3.0))); x += 0.05; } }
    #[test] fn deterministic() { assert_eq!(sag(0.3, 3.0).to_bits(), sag(0.3, 3.0).to_bits()); }
}
