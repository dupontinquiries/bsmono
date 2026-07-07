//! Perf: batched cloud coverage over a weather grid, bit-identical to ref.

pub fn cover_one(humidity: f32, temp_norm: f32, altitude_norm: f32) -> f32 {
    let h = humidity.clamp(0.0, 1.0);
    let t = temp_norm.clamp(0.0, 1.0);
    let alt = altitude_norm.clamp(0.0, 1.0);
    (h * (1.0 - 0.5 * t) * (1.0 - 0.4 * alt)).clamp(0.0, 1.0)
}

pub fn cover_batch(items: &[(f32, f32, f32)]) -> Vec<f32> {
    items.iter().map(|&(h, t, a)| cover_one(h, t, a)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn parity() {
        let items = [(0.0f32, 0.5f32, 0.5f32), (1.0, 0.0, 0.0), (0.8, 0.9, 0.2), (0.6, 0.3, 0.7)];
        let b = cover_batch(&items);
        for (i, &it) in items.iter().enumerate() { assert_eq!(b[i].to_bits(), cover_one(it.0, it.1, it.2).to_bits()); }
    }
    #[test] fn empty_is_empty() { assert!(cover_batch(&[]).is_empty()); }
    #[test] fn single_eq_batch1() { assert_eq!(cover_batch(&[(0.8, 0.5, 0.3)])[0].to_bits(), cover_one(0.8, 0.5, 0.3).to_bits()); }
    #[test] fn dry_clear() { assert_eq!(cover_batch(&[(0.0, 0.5, 0.5)])[0], 0.0); }
    #[test] fn bounded() { for v in cover_batch(&[(0.8, 0.5, 0.3), (1.0, 0.0, 0.0)]) { assert!((0.0..=1.0).contains(&v)); } }
    #[test] fn deterministic() { assert_eq!(cover_batch(&[(0.8, 0.5, 0.3)]), cover_batch(&[(0.8, 0.5, 0.3)])); }
}
