//! Perf: batched particle alpha over a whole emitter, bit-identical to ref.

pub fn alpha_one(age: f32, lifetime: f32) -> f32 {
    let lt = lifetime.max(1e-6);
    let t = (age.max(0.0) / lt).clamp(0.0, 1.0);
    let a = if t < 0.15 { t / 0.15 } else if t > 0.7 { (1.0 - t) / 0.3 } else { 1.0 };
    a.clamp(0.0, 1.0)
}

pub fn alpha_batch(items: &[(f32, f32)]) -> Vec<f32> {
    items.iter().map(|&(age, lt)| alpha_one(age, lt)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn parity() {
        let items = [(0.0f32, 10.0f32), (1.0, 10.0), (5.0, 10.0), (9.0, 10.0), (10.0, 10.0)];
        let b = alpha_batch(&items);
        for (i, &it) in items.iter().enumerate() { assert_eq!(b[i].to_bits(), alpha_one(it.0, it.1).to_bits()); }
    }
    #[test] fn empty_is_empty() { assert!(alpha_batch(&[]).is_empty()); }
    #[test] fn single_eq_batch1() { assert_eq!(alpha_batch(&[(5.0, 10.0)])[0].to_bits(), alpha_one(5.0, 10.0).to_bits()); }
    #[test] fn mid_full() { assert!((alpha_batch(&[(5.0, 10.0)])[0] - 1.0).abs() < 1e-6); }
    #[test] fn death_zero() { assert!(alpha_batch(&[(10.0, 10.0)])[0].abs() < 1e-6); }
    #[test] fn deterministic() { assert_eq!(alpha_batch(&[(5.0, 10.0)]), alpha_batch(&[(5.0, 10.0)])); }
}
