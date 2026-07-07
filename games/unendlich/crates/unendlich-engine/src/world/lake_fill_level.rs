//! Lake water level toward equilibrium: net (inflow − outflow) added to current
//! level, clamped to [0, capacity]. Pure, deterministic.

pub fn fill_level(current: f32, inflow: f32, outflow: f32, capacity: f32) -> f32 {
    let c = current.max(0.0);
    let cap = capacity.max(0.0);
    (c + inflow.max(0.0) - outflow.max(0.0)).clamp(0.0, cap)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn inflow_raises() { assert!(fill_level(5.0, 3.0, 0.0, 100.0) > 5.0); }
    #[test] fn outflow_lowers() { assert!(fill_level(5.0, 0.0, 3.0, 100.0) < 5.0); }
    #[test] fn capped() { assert_eq!(fill_level(99.0, 100.0, 0.0, 100.0), 100.0); }
    #[test] fn floored() { assert_eq!(fill_level(1.0, 0.0, 100.0, 100.0), 0.0); }
    #[test] fn balanced_unchanged() { assert_eq!(fill_level(5.0, 2.0, 2.0, 100.0), 5.0); }
    #[test] fn deterministic() { assert_eq!(fill_level(5.0, 3.0, 1.0, 100.0).to_bits(), fill_level(5.0, 3.0, 1.0, 100.0).to_bits()); }
}
