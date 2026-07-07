//! Defensive-wall length to enclose a settlement of a given area, modeled as a
//! circle: perimeter = 2·√(π·area). Pure, deterministic.

pub fn perimeter(area: f32) -> f32 {
    2.0 * (std::f32::consts::PI * area.max(0.0)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;
    #[test] fn zero_zero() { assert_eq!(perimeter(0.0), 0.0); }
    #[test] fn bigger_longer() { assert!(perimeter(1000.0) > perimeter(100.0)); }
    #[test] fn circle_consistency() { assert!((perimeter(PI) - 2.0 * PI).abs() < 1e-4); }
    #[test] fn diminishing() {
        let early = perimeter(400.0) - perimeter(0.0);
        let late = perimeter(900.0) - perimeter(500.0);
        assert!(early > late);
    }
    #[test] fn nonneg() { assert!(perimeter(500.0) >= 0.0); }
    #[test] fn deterministic() { assert_eq!(perimeter(500.0).to_bits(), perimeter(500.0).to_bits()); }
}
