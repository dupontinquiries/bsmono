//! Probability [0,1] an item drops: base rate boosted by luck, reduced by a rarity
//! penalty (rarer items less likely). Pure, deterministic.

pub fn drop_chance(base_rate: f32, luck: f32, rarity_penalty: f32) -> f32 {
    let base = base_rate.clamp(0.0, 1.0);
    let l = luck.max(0.0);
    let pen = rarity_penalty.clamp(0.0, 1.0);
    (base * (1.0 + l) * (1.0 - pen)).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn zero_base_zero() { assert_eq!(drop_chance(0.0, 1.0, 0.0), 0.0); }
    #[test] fn luck_raises() { assert!(drop_chance(0.2, 1.0, 0.0) > drop_chance(0.2, 0.0, 0.0)); }
    #[test] fn penalty_lowers() { assert!(drop_chance(0.5, 0.0, 0.8) < drop_chance(0.5, 0.0, 0.2)); }
    #[test] fn full_penalty_zero() { assert_eq!(drop_chance(0.5, 5.0, 1.0), 0.0); }
    #[test] fn bounded_capped() { assert_eq!(drop_chance(0.9, 5.0, 0.0), 1.0); }
    #[test] fn deterministic() { assert_eq!(drop_chance(0.5, 1.0, 0.3).to_bits(), drop_chance(0.5, 1.0, 0.3).to_bits()); }
}
