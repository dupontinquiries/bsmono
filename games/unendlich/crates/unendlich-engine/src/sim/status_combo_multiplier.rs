//! Damage multiplier from stacking complementary status effects: 1.0 with ≤1
//! active, +per_combo for each additional simultaneous effect (synergy), capped. Pure.

pub fn multiplier(active_count: u32, per_combo: f32, max_mult: f32) -> f32 {
    let extra = active_count.saturating_sub(1) as f32;
    let pc = per_combo.max(0.0);
    let mm = max_mult.max(1.0);
    (1.0 + extra * pc).min(mm)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn none_base() { assert_eq!(multiplier(0, 0.25, 3.0), 1.0); }
    #[test] fn one_base() { assert_eq!(multiplier(1, 0.25, 3.0), 1.0); }
    #[test] fn two_synergy() { assert!((multiplier(2, 0.25, 3.0) - 1.25).abs() < 1e-6); }
    #[test] fn capped() { assert_eq!(multiplier(100, 0.25, 3.0), 3.0); }
    #[test] fn never_below_one() { assert!(multiplier(0, 0.25, 3.0) >= 1.0); }
    #[test] fn deterministic() { assert_eq!(multiplier(3, 0.25, 3.0).to_bits(), multiplier(3, 0.25, 3.0).to_bits()); }
}
