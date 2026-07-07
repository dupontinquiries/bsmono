//! Perf: batched Beer–Lambert transmittance over ray samples, bit-identical to ref.

pub fn trans_one(density: f32, distance: f32) -> f32 {
    let d = density.max(0.0);
    let dist = distance.max(0.0);
    (-(d * dist)).exp().clamp(0.0, 1.0)
}

pub fn trans_batch(items: &[(f32, f32)]) -> Vec<f32> {
    items.iter().map(|&(d, dist)| trans_one(d, dist)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn parity() {
        let items = [(0.0f32, 100.0f32), (0.3, 10.0), (1.0, 100.0), (0.2, 5.0)];
        let b = trans_batch(&items);
        for (i, &it) in items.iter().enumerate() { assert_eq!(b[i].to_bits(), trans_one(it.0, it.1).to_bits()); }
    }
    #[test] fn empty_is_empty() { assert!(trans_batch(&[]).is_empty()); }
    #[test] fn single_eq_batch1() { assert_eq!(trans_batch(&[(0.3, 10.0)])[0].to_bits(), trans_one(0.3, 10.0).to_bits()); }
    #[test] fn zero_density_clear() { assert_eq!(trans_batch(&[(0.0, 100.0)])[0], 1.0); }
    #[test] fn dense_far_opaque() { assert!(trans_batch(&[(1.0, 100.0)])[0] < 0.01); }
    #[test] fn deterministic() { assert_eq!(trans_batch(&[(0.3, 10.0)]), trans_batch(&[(0.3, 10.0)])); }
}
