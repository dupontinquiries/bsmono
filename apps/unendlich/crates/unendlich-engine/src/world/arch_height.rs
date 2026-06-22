//! Arch/bridge curve height at `span_fraction` in [0,1] (0,1 = abutments): a
//! parabola 4x(1−x) peaking at mid-span at `max_rise`. Pure, deterministic.

pub fn height(span_fraction: f32, max_rise: f32) -> f32 {
    let x = span_fraction.clamp(0.0, 1.0);
    let r = max_rise.max(0.0);
    (4.0 * x * (1.0 - x) * r).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn ends_zero() { assert_eq!(height(0.0, 5.0), 0.0); assert_eq!(height(1.0, 5.0), 0.0); }
    #[test] fn mid_peak() { assert!((height(0.5, 5.0) - 5.0).abs() < 1e-5); }
    #[test] fn symmetric() { assert!((height(0.25, 5.0) - height(0.75, 5.0)).abs() < 1e-6); }
    #[test] fn quarter_below_mid() { assert!(height(0.25, 5.0) < height(0.5, 5.0)); }
    #[test] fn bounded() {
        let mut x = 0.0f32;
        while x <= 1.0 { assert!((0.0..=5.0).contains(&height(x, 5.0))); x += 0.05; }
    }
    #[test] fn deterministic() { assert_eq!(height(0.3, 5.0).to_bits(), height(0.3, 5.0).to_bits()); }
}
