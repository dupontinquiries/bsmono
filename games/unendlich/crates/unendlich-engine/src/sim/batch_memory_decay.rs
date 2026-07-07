//! Perf: batched memory decay over an agent's whole memory store, bit-identical to ref.

pub fn decay_one(strength: f32, dt: f32, half_life: f32) -> f32 {
    let s = strength.max(0.0);
    let t = dt.max(0.0);
    let hl = half_life.max(1e-4);
    s * (-(std::f32::consts::LN_2 / hl * t)).exp()
}

// r330-refactor-batch: shared map helper (bit-identical to iter/map/collect).
use crate::util::batch_kernel::map_collect;
pub fn decay_batch(items: &[(f32, f32, f32)]) -> Vec<f32> {
    map_collect(items, |&(s, t, h)| decay_one(s, t, h))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn parity() {
        let items = [(1.0f32, 0.0f32, 10.0f32), (1.0, 10.0, 10.0), (0.5, 20.0, 5.0), (0.8, 3.0, 15.0)];
        let b = decay_batch(&items);
        for (i, &it) in items.iter().enumerate() { assert_eq!(b[i].to_bits(), decay_one(it.0, it.1, it.2).to_bits()); }
    }
    #[test] fn empty_is_empty() { assert!(decay_batch(&[]).is_empty()); }
    #[test] fn single_eq_batch1() { assert_eq!(decay_batch(&[(1.0, 5.0, 10.0)])[0].to_bits(), decay_one(1.0, 5.0, 10.0).to_bits()); }
    #[test] fn zero_dt_unchanged() { assert_eq!(decay_batch(&[(1.0, 0.0, 10.0)])[0], 1.0); }
    #[test] fn half_life_halves() { assert!((decay_batch(&[(1.0, 10.0, 10.0)])[0] - 0.5).abs() < 1e-4); }
    #[test] fn deterministic() { assert_eq!(decay_batch(&[(1.0, 5.0, 10.0)]), decay_batch(&[(1.0, 5.0, 10.0)])); }
}
