//! Perf: batched market price over a whole stall/inventory, bit-identical to ref.

pub fn price_one(base: f32, rarity_tier: u8, demand: f32) -> f32 {
    let b = base.max(0.0);
    let r = 1.0 + (rarity_tier.min(10) as f32) * 0.5;
    let d = demand.clamp(0.0, 2.0);
    (b * r * d).max(1.0)
}

pub fn price_batch(items: &[(f32, u8, f32)]) -> Vec<f32> {
    items.iter().map(|&(b, r, d)| price_one(b, r, d)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn parity() {
        let items = [(100.0f32, 5u8, 1.0f32), (50.0, 0, 0.5), (200.0, 3, 1.5), (10.0, 10, 2.0)];
        let b = price_batch(&items);
        for (i, &it) in items.iter().enumerate() { assert_eq!(b[i].to_bits(), price_one(it.0, it.1, it.2).to_bits()); }
    }
    #[test] fn empty_is_empty() { assert!(price_batch(&[]).is_empty()); }
    #[test] fn single_eq_batch1() { assert_eq!(price_batch(&[(100.0, 5, 1.0)])[0].to_bits(), price_one(100.0, 5, 1.0).to_bits()); }
    #[test] fn rarity_scales() { assert!(price_batch(&[(100.0, 5, 1.0)])[0] > price_batch(&[(100.0, 0, 1.0)])[0]); }
    #[test] fn floored() { assert_eq!(price_batch(&[(100.0, 0, 0.0)])[0], 1.0); }
    #[test] fn deterministic() { assert_eq!(price_batch(&[(100.0, 5, 1.0)]), price_batch(&[(100.0, 5, 1.0)])); }
}
