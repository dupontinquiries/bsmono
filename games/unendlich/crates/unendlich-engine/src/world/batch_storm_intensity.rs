//! Perf: batched storm intensity over a weather grid, bit-identical to ref.

pub fn intensity_one(humidity: f32, instability: f32) -> f32 {
    (humidity.clamp(0.0, 1.0) * instability.clamp(0.0, 1.0)).clamp(0.0, 1.0)
}

pub fn intensity_batch(items: &[(f32, f32)]) -> Vec<f32> {
    items.iter().map(|&(h, i)| intensity_one(h, i)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn parity() {
        let items = [(0.0f32, 0.9f32), (0.9, 0.0), (0.9, 0.9), (0.5, 0.7)];
        let b = intensity_batch(&items);
        for (i, &it) in items.iter().enumerate() { assert_eq!(b[i].to_bits(), intensity_one(it.0, it.1).to_bits()); }
    }
    #[test] fn empty_is_empty() { assert!(intensity_batch(&[]).is_empty()); }
    #[test] fn single_eq_batch1() { assert_eq!(intensity_batch(&[(0.7, 0.6)])[0].to_bits(), intensity_one(0.7, 0.6).to_bits()); }
    #[test] fn dry_calm() { assert_eq!(intensity_batch(&[(0.0, 0.9)])[0], 0.0); }
    #[test] fn both_max_one() { assert_eq!(intensity_batch(&[(1.0, 1.0)])[0], 1.0); }
    #[test] fn deterministic() { assert_eq!(intensity_batch(&[(0.7, 0.6)]), intensity_batch(&[(0.7, 0.6)])); }
}
