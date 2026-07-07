//! Perf: batched drop chances over a loot table, bit-identical to ref.

pub fn chance_one(base_rate: f32, luck: f32, rarity_penalty: f32) -> f32 {
    let base = base_rate.clamp(0.0, 1.0);
    let l = luck.max(0.0);
    let pen = rarity_penalty.clamp(0.0, 1.0);
    (base * (1.0 + l) * (1.0 - pen)).clamp(0.0, 1.0)
}

// r330-refactor-batch: shared map helper (bit-identical to iter/map/collect).
use crate::util::batch_kernel::map_collect;
pub fn chance_batch(items: &[(f32, f32, f32)]) -> Vec<f32> {
    map_collect(items, |&(b, l, p)| chance_one(b, l, p))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn parity() {
        let items = [(0.0f32, 1.0f32, 0.0f32), (0.2, 1.0, 0.0), (0.5, 0.0, 0.8), (0.9, 5.0, 0.0)];
        let b = chance_batch(&items);
        for (i, &it) in items.iter().enumerate() { assert_eq!(b[i].to_bits(), chance_one(it.0, it.1, it.2).to_bits()); }
    }
    #[test] fn empty_is_empty() { assert!(chance_batch(&[]).is_empty()); }
    #[test] fn single_eq_batch1() { assert_eq!(chance_batch(&[(0.5, 1.0, 0.3)])[0].to_bits(), chance_one(0.5, 1.0, 0.3).to_bits()); }
    #[test] fn zero_base_zero() { assert_eq!(chance_batch(&[(0.0, 1.0, 0.0)])[0], 0.0); }
    #[test] fn capped() { assert_eq!(chance_batch(&[(0.9, 5.0, 0.0)])[0], 1.0); }
    #[test] fn deterministic() { assert_eq!(chance_batch(&[(0.5, 1.0, 0.3)]), chance_batch(&[(0.5, 1.0, 0.3)])); }
}
