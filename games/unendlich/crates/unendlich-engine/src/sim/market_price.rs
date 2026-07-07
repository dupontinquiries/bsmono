//! Market price = base · rarity-multiplier · demand, floored at 1. Rarity tier
//! 0..=10 scales (0→1×, 5→3.5×); demand in [0,2] (1 = neutral). Pure.

pub fn price(base: f32, rarity_tier: u8, demand: f32) -> f32 {
    let b = base.max(0.0);
    let r = 1.0 + (rarity_tier.min(10) as f32) * 0.5;
    let d = demand.clamp(0.0, 2.0);
    (b * r * d).max(1.0)
}

/// Live market price for an item whose intrinsic value has ALREADY been appraised
/// (`item_appraise`): apply market `demand` and the price floor to that value.
/// Rarity is already priced into `appraised_value`, so it is NOT re-applied here
/// (tier 0) — avoiding the double-count a naive `price(appraised, rarity, demand)`
/// would introduce. Pure; the intended bridge from appraisal into the market.
pub fn market_value(appraised_value: f32, demand: f32) -> f32 {
    price(appraised_value, 0, demand)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn higher_rarity_more() { assert!(price(100.0, 5, 1.0) > price(100.0, 0, 1.0)); }
    #[test] fn more_demand_more() { assert!(price(100.0, 2, 1.5) > price(100.0, 2, 0.5)); }
    #[test] fn floored_at_one() { assert_eq!(price(100.0, 0, 0.0), 1.0); }
    #[test] fn base_scales() { assert!(price(200.0, 2, 1.0) > price(100.0, 2, 1.0)); }
    #[test] fn tier_capped() { assert_eq!(price(100.0, 200, 1.0), price(100.0, 10, 1.0)); }
    #[test] fn deterministic() { assert_eq!(price(100.0, 3, 1.2).to_bits(), price(100.0, 3, 1.2).to_bits()); }

    #[test] fn market_value_applies_demand_without_re_adding_rarity() {
        // identical appraised value, only demand differs.
        assert!(market_value(100.0, 1.5) > market_value(100.0, 0.5));
        // rarity is NOT re-applied: market_value == price at tier 0.
        assert_eq!(market_value(100.0, 1.0), price(100.0, 0, 1.0));
        // floored at 1 and never panics on degenerate input.
        assert_eq!(market_value(0.0, 0.0), 1.0);
        assert!(market_value(50.0, 1.0).is_finite());
    }
}
