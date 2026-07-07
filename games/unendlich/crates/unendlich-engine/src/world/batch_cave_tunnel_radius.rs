//! Perf: batched cave-tunnel radii over sampled noise along a tunnel, bit-identical to ref.

pub fn radius_one(noise: f32, base_radius: f32, variation: f32) -> f32 {
    let n = noise.clamp(-1.0, 1.0);
    let b = base_radius.max(0.0);
    let v = variation.clamp(0.0, 1.0);
    (b * (1.0 + n * v)).max(0.1)
}

pub fn radius_batch(items: &[(f32, f32, f32)]) -> Vec<f32> {
    items.iter().map(|&(n, b, v)| radius_one(n, b, v)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn parity() {
        let items = [(0.0f32, 3.0f32, 0.5f32), (1.0, 3.0, 0.5), (-1.0, 3.0, 0.5), (0.3, 4.0, 0.8)];
        let b = radius_batch(&items);
        for (i, &it) in items.iter().enumerate() { assert_eq!(b[i].to_bits(), radius_one(it.0, it.1, it.2).to_bits()); }
    }
    #[test] fn empty_is_empty() { assert!(radius_batch(&[]).is_empty()); }
    #[test] fn single_eq_batch1() { assert_eq!(radius_batch(&[(0.5, 3.0, 0.5)])[0].to_bits(), radius_one(0.5, 3.0, 0.5).to_bits()); }
    #[test] fn zero_noise_base() { assert_eq!(radius_batch(&[(0.0, 3.0, 0.5)])[0], 3.0); }
    #[test] fn floored() { assert!(radius_batch(&[(-1.0, 0.05, 1.0)])[0] >= 0.1); }
    #[test] fn deterministic() { assert_eq!(radius_batch(&[(0.5, 3.0, 0.5)]), radius_batch(&[(0.5, 3.0, 0.5)])); }
}
