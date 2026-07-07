//! Perf: batched health-bar fill for many on-screen entities, bit-identical to ref.

pub fn fill_one(current: f32, max: f32) -> f32 {
    if max <= 0.0 {
        return 0.0;
    }
    (current / max).clamp(0.0, 1.0)
}

pub fn fill_batch(items: &[(f32, f32)]) -> Vec<f32> {
    items.iter().map(|&(c, m)| fill_one(c, m)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn parity() {
        let items = [(100.0f32, 100.0f32), (50.0, 100.0), (0.0, 100.0), (150.0, 100.0)];
        let b = fill_batch(&items);
        for (i, &it) in items.iter().enumerate() { assert_eq!(b[i].to_bits(), fill_one(it.0, it.1).to_bits()); }
    }
    #[test] fn empty_is_empty() { assert!(fill_batch(&[]).is_empty()); }
    #[test] fn single_eq_batch1() { assert_eq!(fill_batch(&[(50.0, 100.0)])[0].to_bits(), fill_one(50.0, 100.0).to_bits()); }
    #[test] fn full_is_one() { assert_eq!(fill_batch(&[(100.0, 100.0)])[0], 1.0); }
    #[test] fn zero_max_guarded() { assert_eq!(fill_batch(&[(50.0, 0.0)])[0], 0.0); }
    #[test] fn deterministic() { assert_eq!(fill_batch(&[(50.0, 100.0)]), fill_batch(&[(50.0, 100.0)])); }
}
