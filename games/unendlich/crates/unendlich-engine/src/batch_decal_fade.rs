//! Perf: batched decal fade over many projected fragments, bit-identical to ref.

pub fn fade_one(distance_to_surface: f32, thickness: f32) -> f32 {
    let t = thickness.max(1e-6);
    let d = distance_to_surface.abs();
    if d >= t { 0.0 } else { (1.0 - d / t).clamp(0.0, 1.0) }
}

pub fn fade_batch(items: &[(f32, f32)]) -> Vec<f32> {
    items.iter().map(|&(d, t)| fade_one(d, t)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn parity() {
        let items = [(0.0f32, 0.5f32), (0.25, 0.5), (0.5, 0.5), (1.0, 0.5)];
        let b = fade_batch(&items);
        for (i, &it) in items.iter().enumerate() { assert_eq!(b[i].to_bits(), fade_one(it.0, it.1).to_bits()); }
    }
    #[test] fn empty_is_empty() { assert!(fade_batch(&[]).is_empty()); }
    #[test] fn single_eq_batch1() { assert_eq!(fade_batch(&[(0.2, 0.5)])[0].to_bits(), fade_one(0.2, 0.5).to_bits()); }
    #[test] fn on_surface_full() { assert!((fade_batch(&[(0.0, 0.5)])[0] - 1.0).abs() < 1e-6); }
    #[test] fn beyond_zero() { assert_eq!(fade_batch(&[(1.0, 0.5)])[0], 0.0); }
    #[test] fn deterministic() { assert_eq!(fade_batch(&[(0.2, 0.5)]), fade_batch(&[(0.2, 0.5)])); }
}
