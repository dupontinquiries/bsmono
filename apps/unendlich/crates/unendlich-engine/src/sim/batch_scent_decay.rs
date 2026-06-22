//! Perf: batched scent decay, bit-identical to the scalar reference.
//! Mirrors scent_decay: strength · exp(−rate·dt), floored at 0.

pub fn decay_one(strength: f32, rate: f32, dt: f32) -> f32 {
    let s = strength.max(0.0);
    let r = rate.max(0.0);
    let t = dt.max(0.0);
    (s * (-(r * t)).exp()).max(0.0)
}

// r330-refactor-batch: shared map helper (bit-identical to iter/map/collect).
use crate::util::batch_kernel::map_collect;
pub fn decay_batch(items: &[(f32, f32, f32)]) -> Vec<f32> {
    map_collect(items, |&(s, r, t)| decay_one(s, r, t))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn bit_parity() {
        let items = [(1.0f32, 0.5f32, 2.0f32), (10.0, 0.1, 5.0), (0.0, 1.0, 1.0), (5.0, 2.0, 0.0)];
        let b = decay_batch(&items);
        for (i, &it) in items.iter().enumerate() { assert_eq!(b[i].to_bits(), decay_one(it.0, it.1, it.2).to_bits()); }
    }
    #[test] fn empty_is_empty() { assert!(decay_batch(&[]).is_empty()); }
    #[test] fn single_eq_batch1() { assert_eq!(decay_batch(&[(3.0, 0.4, 1.0)])[0].to_bits(), decay_one(3.0, 0.4, 1.0).to_bits()); }
    #[test] fn decays_over_time() { assert!(decay_one(10.0, 0.5, 5.0) < decay_one(10.0, 0.5, 1.0)); }
    #[test] fn bounded_by_strength() { assert!(decay_one(8.0, 0.3, 2.0) <= 8.0); }
    #[test] fn deterministic() { assert_eq!(decay_batch(&[(5.0, 0.5, 2.0)]), decay_batch(&[(5.0, 0.5, 2.0)])); }
}
