//! Perf: batched breath falloff over all targets in the cone, bit-identical to ref.

pub fn falloff_one(distance: f32, max_range: f32) -> f32 {
    let d = distance.max(0.0);
    let r = max_range.max(1e-4);
    (1.0 - d / r).clamp(0.0, 1.0)
}

pub fn falloff_batch(items: &[(f32, f32)]) -> Vec<f32> {
    items.iter().map(|&(d, r)| falloff_one(d, r)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn parity() {
        let items = [(0.0f32, 10.0f32), (5.0, 10.0), (10.0, 10.0), (3.0, 12.0)];
        let b = falloff_batch(&items);
        for (i, &it) in items.iter().enumerate() { assert_eq!(b[i].to_bits(), falloff_one(it.0, it.1).to_bits()); }
    }
    #[test] fn empty_is_empty() { assert!(falloff_batch(&[]).is_empty()); }
    #[test] fn single_eq_batch1() { assert_eq!(falloff_batch(&[(5.0, 10.0)])[0].to_bits(), falloff_one(5.0, 10.0).to_bits()); }
    #[test] fn mouth_full() { assert_eq!(falloff_batch(&[(0.0, 10.0)])[0], 1.0); }
    #[test] fn edge_zero() { assert_eq!(falloff_batch(&[(10.0, 10.0)])[0], 0.0); }
    #[test] fn deterministic() { assert_eq!(falloff_batch(&[(5.0, 10.0)]), falloff_batch(&[(5.0, 10.0)])); }
}
