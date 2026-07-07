//! Perf: batched damage-number scales over all floaters this frame, bit-identical to ref.

pub fn scale_one(damage: f32, ref_damage: f32, min_scale: f32, max_scale: f32) -> f32 {
    let d = damage.max(0.0);
    let rd = ref_damage.max(1e-4);
    let mn = min_scale.max(0.0);
    let mx = max_scale.max(mn);
    mn + (mx - mn) * (d / rd).clamp(0.0, 1.0)
}

pub fn scale_batch(items: &[(f32, f32, f32, f32)]) -> Vec<f32> {
    items.iter().map(|&(d, rd, mn, mx)| scale_one(d, rd, mn, mx)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn parity() {
        let items = [(0.0f32, 100.0f32, 0.5f32, 2.0f32), (100.0, 100.0, 0.5, 2.0), (500.0, 100.0, 0.5, 2.0), (40.0, 80.0, 0.6, 1.8)];
        let b = scale_batch(&items);
        for (i, &it) in items.iter().enumerate() { assert_eq!(b[i].to_bits(), scale_one(it.0, it.1, it.2, it.3).to_bits()); }
    }
    #[test] fn empty_is_empty() { assert!(scale_batch(&[]).is_empty()); }
    #[test] fn single_eq_batch1() { assert_eq!(scale_batch(&[(80.0, 100.0, 0.5, 2.0)])[0].to_bits(), scale_one(80.0, 100.0, 0.5, 2.0).to_bits()); }
    #[test] fn zero_is_min() { assert_eq!(scale_batch(&[(0.0, 100.0, 0.5, 2.0)])[0], 0.5); }
    #[test] fn over_capped() { assert_eq!(scale_batch(&[(500.0, 100.0, 0.5, 2.0)])[0], 2.0); }
    #[test] fn deterministic() { assert_eq!(scale_batch(&[(80.0, 100.0, 0.5, 2.0)]), scale_batch(&[(80.0, 100.0, 0.5, 2.0)])); }
}
