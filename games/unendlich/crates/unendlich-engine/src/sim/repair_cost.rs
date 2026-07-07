//! Cost to repair an item: proportional to the durability missing × a per-point
//! rate. Full durability ⇒ 0. Pure, deterministic.

pub fn repair_cost(max_durability: f32, current: f32, rate: f32) -> f32 {
    let max = max_durability.max(0.0);
    let cur = current.clamp(0.0, max);
    (max - cur) * rate.max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn full_durability_free() { assert_eq!(repair_cost(100.0, 100.0, 2.0), 0.0); }
    #[test] fn more_missing_more() { assert!(repair_cost(100.0, 10.0, 2.0) > repair_cost(100.0, 80.0, 2.0)); }
    #[test] fn rate_scales() { assert!(repair_cost(100.0, 50.0, 4.0) > repair_cost(100.0, 50.0, 2.0)); }
    #[test] fn nonneg_over_max() { assert_eq!(repair_cost(100.0, 200.0, 2.0), 0.0); }
    #[test] fn bounded() { assert!(repair_cost(100.0, 0.0, 2.0) >= 0.0); }
    #[test] fn deterministic() { assert_eq!(repair_cost(100.0, 50.0, 2.0).to_bits(), repair_cost(100.0, 50.0, 2.0).to_bits()); }
}
