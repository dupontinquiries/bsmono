//! Perf: batched ember-glow intensity over all burning entities, bit-identical to ref.

use std::f32::consts::PI;

pub fn intensity_one(stacks: u32, t: f32, max_stacks: u32) -> f32 {
    let ms = max_stacks.max(1);
    let s = (stacks.min(ms) as f32) / (ms as f32);
    let flicker = 0.85 + 0.15 * (2.0 * PI * 6.0 * t).sin();
    (s * flicker).clamp(0.0, 1.0)
}

pub fn intensity_batch(items: &[(u32, f32)], max_stacks: u32) -> Vec<f32> {
    items.iter().map(|&(st, t)| intensity_one(st, t, max_stacks)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn parity() {
        let items = [(0u32, 0.0f32), (5, 0.0), (1, 0.05), (3, 0.2)];
        let b = intensity_batch(&items, 5);
        for (i, &it) in items.iter().enumerate() { assert_eq!(b[i].to_bits(), intensity_one(it.0, it.1, 5).to_bits()); }
    }
    #[test] fn empty_is_empty() { assert!(intensity_batch(&[], 5).is_empty()); }
    #[test] fn single_eq_batch1() { assert_eq!(intensity_batch(&[(3, 0.1)], 5)[0].to_bits(), intensity_one(3, 0.1, 5).to_bits()); }
    #[test] fn zero_stacks_dark() { assert_eq!(intensity_batch(&[(0, 0.0)], 5)[0], 0.0); }
    #[test] fn capped_at_max() { assert_eq!(intensity_batch(&[(100, 0.0)], 5)[0], intensity_one(5, 0.0, 5)); }
    #[test] fn deterministic() { assert_eq!(intensity_batch(&[(3, 0.1)], 5), intensity_batch(&[(3, 0.1)], 5)); }
}
