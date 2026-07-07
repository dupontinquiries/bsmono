//! Rarity selection weight under magic-find: a tier's base weight scaled so higher
//! magic_find shifts weight toward rarer tiers (tier multiplies the bonus). Pure.

pub fn weight(base_weight: f32, rarity_tier: u32, magic_find: f32) -> f32 {
    let bw = base_weight.max(0.0);
    let mf = magic_find.max(0.0);
    bw * (1.0 + mf * rarity_tier as f32)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn tier0_unaffected() { assert_eq!(weight(10.0, 0, 5.0), 10.0); }
    #[test] fn higher_tier_more() { assert!(weight(10.0, 3, 1.0) > weight(10.0, 1, 1.0)); }
    #[test] fn more_mf_more() { assert!(weight(10.0, 2, 2.0) > weight(10.0, 2, 0.5)); }
    #[test] fn zero_base_zero() { assert_eq!(weight(0.0, 3, 5.0), 0.0); }
    #[test] fn monotone_in_mf() { assert!(weight(10.0, 2, 3.0) > weight(10.0, 2, 1.0)); }
    #[test] fn deterministic() { assert_eq!(weight(10.0, 2, 1.5).to_bits(), weight(10.0, 2, 1.5).to_bits()); }
}
