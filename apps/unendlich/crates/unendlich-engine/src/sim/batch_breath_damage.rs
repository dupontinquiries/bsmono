//! Perf: batched breath damage over many targets in the cone, bit-identical to ref.

pub fn dmg_one(distance: f32, max_range: f32, base: f32) -> f32 {
    let d = distance.max(0.0);
    let mr = max_range.max(1e-6);
    let b = base.max(0.0);
    if d >= mr { 0.0 } else { b * (1.0 - d / mr) }
}

pub fn dmg_batch(items: &[(f32, f32, f32)]) -> Vec<f32> {
    items.iter().map(|&(d, m, b)| dmg_one(d, m, b)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn parity() {
        let items = [(0.0f32, 20.0f32, 50.0f32), (10.0, 20.0, 50.0), (25.0, 20.0, 50.0), (5.0, 30.0, 80.0)];
        let b = dmg_batch(&items);
        for (i, &it) in items.iter().enumerate() { assert_eq!(b[i].to_bits(), dmg_one(it.0, it.1, it.2).to_bits()); }
    }
    #[test] fn empty_is_empty() { assert!(dmg_batch(&[]).is_empty()); }
    #[test] fn single_eq_batch1() { assert_eq!(dmg_batch(&[(5.0, 20.0, 50.0)])[0].to_bits(), dmg_one(5.0, 20.0, 50.0).to_bits()); }
    #[test] fn at_mouth_full() { assert!((dmg_batch(&[(0.0, 20.0, 50.0)])[0] - 50.0).abs() < 1e-4); }
    #[test] fn beyond_zero() { assert_eq!(dmg_batch(&[(25.0, 20.0, 50.0)])[0], 0.0); }
    #[test] fn deterministic() { assert_eq!(dmg_batch(&[(5.0, 20.0, 50.0)]), dmg_batch(&[(5.0, 20.0, 50.0)])); }
}
