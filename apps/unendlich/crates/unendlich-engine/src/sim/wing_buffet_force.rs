//! Knockback force from a wing-buffet AoE: power falling off linearly with
//! distance to max_range. Pure, deterministic.

pub fn force(distance: f32, max_range: f32, power: f32) -> f32 {
    let prox = (1.0 - distance.max(0.0) / max_range.max(1e-4)).clamp(0.0, 1.0);
    power.max(0.0) * prox
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn source_full() { assert!((force(0.0, 8.0, 50.0) - 50.0).abs() < 1e-4); }
    #[test] fn edge_zero() { assert_eq!(force(8.0, 8.0, 50.0), 0.0); }
    #[test] fn beyond_zero() { assert_eq!(force(20.0, 8.0, 50.0), 0.0); }
    #[test] fn farther_less() { assert!(force(6.0, 8.0, 50.0) < force(2.0, 8.0, 50.0)); }
    #[test] fn stronger_more() { assert!(force(2.0, 8.0, 80.0) > force(2.0, 8.0, 40.0)); }
    #[test] fn deterministic() { assert_eq!(force(2.0, 8.0, 50.0).to_bits(), force(2.0, 8.0, 50.0).to_bits()); }
}
