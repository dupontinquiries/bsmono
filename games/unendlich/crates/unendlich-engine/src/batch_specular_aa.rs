//! Perf: batched geometric specular-AA roughness clamp, bit-identical to scalar ref.
//! Mirrors specular_aa: √(r² + variance), clamped, never below input roughness.

pub fn clamp_one(roughness: f32, normal_variance: f32) -> f32 {
    let r = roughness.clamp(0.0, 1.0);
    let v = normal_variance.max(0.0);
    (r * r + v).sqrt().clamp(0.0, 1.0).max(r)
}

pub fn clamp_batch(items: &[(f32, f32)]) -> Vec<f32> {
    items.iter().map(|&(r, v)| clamp_one(r, v)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn parity() {
        let items = [(0.3f32, 0.0f32), (0.5, 0.2), (0.9, 0.5), (0.1, 1.0)];
        let b = clamp_batch(&items);
        for (i, &it) in items.iter().enumerate() { assert_eq!(b[i].to_bits(), clamp_one(it.0, it.1).to_bits()); }
    }
    #[test] fn empty_is_empty() { assert!(clamp_batch(&[]).is_empty()); }
    #[test] fn single_eq_batch1() { assert_eq!(clamp_batch(&[(0.4, 0.1)])[0].to_bits(), clamp_one(0.4, 0.1).to_bits()); }
    #[test] fn no_variance_unchanged() { assert!((clamp_one(0.3, 0.0) - 0.3).abs() < 1e-6); }
    #[test] fn bounded() { for v in clamp_batch(&[(0.5, 5.0), (0.2, 0.3)]) { assert!((0.0..=1.0).contains(&v)); } }
    #[test] fn deterministic() { assert_eq!(clamp_batch(&[(0.3, 0.2)]), clamp_batch(&[(0.3, 0.2)])); }
}
