//! Perf: batched per-tile path cost over a frontier, bit-identical to ref.

pub fn cost_one(base: f32, terrain_mult: f32, slope: f32) -> f32 {
    let b = base.max(0.0);
    let tm = terrain_mult.max(0.0);
    let s = slope.max(0.0);
    (b * tm * (1.0 + s)).max(0.01)
}

// r330-refactor-batch: shared map helper (bit-identical to iter/map/collect).
use crate::util::batch_kernel::map_collect;
pub fn cost_batch(items: &[(f32, f32, f32)]) -> Vec<f32> {
    map_collect(items, |&(b, t, s)| cost_one(b, t, s))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn parity() {
        let items = [(1.0f32, 1.0f32, 0.0f32), (1.0, 3.0, 0.5), (2.0, 1.0, 2.0), (0.0, 0.0, 0.0)];
        let b = cost_batch(&items);
        for (i, &it) in items.iter().enumerate() { assert_eq!(b[i].to_bits(), cost_one(it.0, it.1, it.2).to_bits()); }
    }
    #[test] fn empty_is_empty() { assert!(cost_batch(&[]).is_empty()); }
    #[test] fn single_eq_batch1() { assert_eq!(cost_batch(&[(1.0, 2.0, 1.0)])[0].to_bits(), cost_one(1.0, 2.0, 1.0).to_bits()); }
    #[test] fn flat_base() { assert!((cost_batch(&[(1.0, 1.0, 0.0)])[0] - 1.0).abs() < 1e-6); }
    #[test] fn floored() { assert!(cost_batch(&[(0.0, 0.0, 0.0)])[0] >= 0.01); }
    #[test] fn deterministic() { assert_eq!(cost_batch(&[(1.0, 2.0, 1.0)]), cost_batch(&[(1.0, 2.0, 1.0)])); }
}
