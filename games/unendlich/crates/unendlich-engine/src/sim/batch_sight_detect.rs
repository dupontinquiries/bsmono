//! Perf: batched visual detection over many targets, bit-identical to ref.

pub fn detect_one(distance: f32, target_dir_dot: f32, fov_cos: f32, max_range: f32) -> f32 {
    let mr = max_range.max(1e-6);
    let d = distance.max(0.0);
    if target_dir_dot < fov_cos || d >= mr { 0.0 } else { (1.0 - d / mr).clamp(0.0, 1.0) }
}

pub fn detect_batch(items: &[(f32, f32)], fov_cos: f32, max_range: f32) -> Vec<f32> {
    items.iter().map(|&(d, dot)| detect_one(d, dot, fov_cos, max_range)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn parity() {
        let items = [(1.0f32, 0.9f32), (1.0, 0.0), (60.0, 0.9), (5.0, 0.5)];
        let b = detect_batch(&items, 0.5, 50.0);
        for (i, &it) in items.iter().enumerate() { assert_eq!(b[i].to_bits(), detect_one(it.0, it.1, 0.5, 50.0).to_bits()); }
    }
    #[test] fn empty_is_empty() { assert!(detect_batch(&[], 0.5, 50.0).is_empty()); }
    #[test] fn single_eq_batch1() { assert_eq!(detect_batch(&[(5.0, 0.9)], 0.5, 50.0)[0].to_bits(), detect_one(5.0, 0.9, 0.5, 50.0).to_bits()); }
    #[test] fn front_near_strong() { assert!(detect_batch(&[(1.0, 0.9)], 0.5, 50.0)[0] > 0.9); }
    #[test] fn behind_zero() { assert_eq!(detect_batch(&[(1.0, 0.0)], 0.5, 50.0)[0], 0.0); }
    #[test] fn deterministic() { assert_eq!(detect_batch(&[(5.0, 0.9)], 0.5, 50.0), detect_batch(&[(5.0, 0.9)], 0.5, 50.0)); }
}
