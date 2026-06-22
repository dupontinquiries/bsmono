//! Perf: batched depth-of-field circle-of-confusion, bit-identical to scalar ref.
//! Mirrors dof_blur: clamp01(|depth − focus| / range).

pub fn coc_one(depth: f32, focus: f32, range: f32) -> f32 {
    let r = range.max(1e-6);
    ((depth - focus).abs() / r).clamp(0.0, 1.0)
}

pub fn coc_batch(items: &[(f32, f32, f32)]) -> Vec<f32> {
    items.iter().map(|&(d, f, r)| coc_one(d, f, r)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn bit_parity() {
        let items = [(10.0f32, 5.0f32, 8.0f32), (5.0, 5.0, 2.0), (100.0, 5.0, 8.0), (0.0, 5.0, 4.0)];
        let b = coc_batch(&items);
        for (i, &it) in items.iter().enumerate() { assert_eq!(b[i].to_bits(), coc_one(it.0, it.1, it.2).to_bits()); }
    }
    #[test] fn empty_is_empty() { assert!(coc_batch(&[]).is_empty()); }
    #[test] fn single_eq_batch1() { assert_eq!(coc_batch(&[(7.0, 5.0, 4.0)])[0].to_bits(), coc_one(7.0, 5.0, 4.0).to_bits()); }
    #[test] fn at_focus_zero() { assert_eq!(coc_one(5.0, 5.0, 8.0), 0.0); }
    #[test] fn bounded() {
        let mut d = -50.0f32;
        while d < 50.0 { let c = coc_one(d, 5.0, 8.0); assert!((0.0..=1.0).contains(&c)); d += 2.0; }
    }
    #[test] fn deterministic() { assert_eq!(coc_batch(&[(9.0, 5.0, 4.0)]), coc_batch(&[(9.0, 5.0, 4.0)])); }
}
