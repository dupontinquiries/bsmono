//! Perf: batched arch-curve sampling along a span, bit-identical to ref.

pub fn h_one(span_fraction: f32, max_rise: f32) -> f32 {
    let x = span_fraction.clamp(0.0, 1.0);
    let r = max_rise.max(0.0);
    (4.0 * x * (1.0 - x) * r).max(0.0)
}

pub fn h_batch(items: &[(f32, f32)]) -> Vec<f32> {
    items.iter().map(|&(x, r)| h_one(x, r)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn parity() {
        let items = [(0.0f32, 5.0f32), (0.5, 5.0), (0.25, 5.0), (1.0, 8.0)];
        let b = h_batch(&items);
        for (i, &it) in items.iter().enumerate() { assert_eq!(b[i].to_bits(), h_one(it.0, it.1).to_bits()); }
    }
    #[test] fn empty_is_empty() { assert!(h_batch(&[]).is_empty()); }
    #[test] fn single_eq_batch1() { assert_eq!(h_batch(&[(0.3, 5.0)])[0].to_bits(), h_one(0.3, 5.0).to_bits()); }
    #[test] fn ends_zero() { assert_eq!(h_batch(&[(0.0, 5.0)])[0], 0.0); }
    #[test] fn mid_peak() { assert!((h_batch(&[(0.5, 5.0)])[0] - 5.0).abs() < 1e-5); }
    #[test] fn deterministic() { assert_eq!(h_batch(&[(0.3, 5.0)]), h_batch(&[(0.3, 5.0)])); }
}
