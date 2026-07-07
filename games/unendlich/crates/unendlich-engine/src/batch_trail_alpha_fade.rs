//! Perf: batched trail-alpha fade over all trail segments, bit-identical to ref.

pub fn alpha_one(age: f32, lifetime: f32) -> f32 {
    let a = age.max(0.0);
    let l = lifetime.max(1e-4);
    (1.0 - a / l).clamp(0.0, 1.0)
}

pub fn alpha_batch(items: &[(f32, f32)]) -> Vec<f32> {
    items.iter().map(|&(a, l)| alpha_one(a, l)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn parity() {
        let items = [(0.0f32, 2.0f32), (1.0, 2.0), (2.0, 2.0), (0.5, 3.0)];
        let b = alpha_batch(&items);
        for (i, &it) in items.iter().enumerate() { assert_eq!(b[i].to_bits(), alpha_one(it.0, it.1).to_bits()); }
    }
    #[test] fn empty_is_empty() { assert!(alpha_batch(&[]).is_empty()); }
    #[test] fn single_eq_batch1() { assert_eq!(alpha_batch(&[(1.0, 2.0)])[0].to_bits(), alpha_one(1.0, 2.0).to_bits()); }
    #[test] fn fresh_opaque() { assert_eq!(alpha_batch(&[(0.0, 2.0)])[0], 1.0); }
    #[test] fn expired_clear() { assert_eq!(alpha_batch(&[(2.0, 2.0)])[0], 0.0); }
    #[test] fn deterministic() { assert_eq!(alpha_batch(&[(1.0, 2.0)]), alpha_batch(&[(1.0, 2.0)])); }
}
