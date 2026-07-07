//! Perf: batched Hooke forces over all springs in a rope/cloth, bit-identical to ref.

pub fn force_one(displacement: f32, stiffness: f32) -> f32 {
    -stiffness.max(0.0) * displacement
}

pub fn force_batch(items: &[(f32, f32)]) -> Vec<f32> {
    items.iter().map(|&(d, k)| force_one(d, k)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn parity() {
        let items = [(0.0f32, 10.0f32), (2.0, 10.0), (-2.0, 10.0), (4.0, 20.0)];
        let b = force_batch(&items);
        for (i, &it) in items.iter().enumerate() { assert_eq!(b[i].to_bits(), force_one(it.0, it.1).to_bits()); }
    }
    #[test] fn empty_is_empty() { assert!(force_batch(&[]).is_empty()); }
    #[test] fn single_eq_batch1() { assert_eq!(force_batch(&[(2.0, 10.0)])[0].to_bits(), force_one(2.0, 10.0).to_bits()); }
    #[test] fn rest_zero() { assert_eq!(force_batch(&[(0.0, 10.0)])[0], 0.0); }
    #[test] fn stretched_pulls_back() { assert!(force_batch(&[(2.0, 10.0)])[0] < 0.0); }
    #[test] fn deterministic() { assert_eq!(force_batch(&[(2.0, 10.0)]), force_batch(&[(2.0, 10.0)])); }
}
