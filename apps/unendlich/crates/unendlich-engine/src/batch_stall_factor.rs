//! Perf: batched stall factor over wing strip samples, bit-identical to ref.

pub fn stall_one(angle_of_attack: f32, stall_angle: f32) -> f32 {
    let aoa = angle_of_attack.abs();
    let sa = stall_angle.max(1e-4);
    if aoa <= sa { 1.0 } else { (1.0 - (aoa - sa) / sa).clamp(0.0, 1.0) }
}

pub fn stall_batch(items: &[(f32, f32)]) -> Vec<f32> {
    items.iter().map(|&(a, s)| stall_one(a, s)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn parity() {
        let items = [(0.2f32, 0.5f32), (0.5, 0.5), (0.7, 0.5), (1.5, 0.5)];
        let b = stall_batch(&items);
        for (i, &it) in items.iter().enumerate() { assert_eq!(b[i].to_bits(), stall_one(it.0, it.1).to_bits()); }
    }
    #[test] fn empty_is_empty() { assert!(stall_batch(&[]).is_empty()); }
    #[test] fn single_eq_batch1() { assert_eq!(stall_batch(&[(0.7, 0.5)])[0].to_bits(), stall_one(0.7, 0.5).to_bits()); }
    #[test] fn below_stall_full() { assert_eq!(stall_batch(&[(0.2, 0.5)])[0], 1.0); }
    #[test] fn deep_stall_zero() { assert_eq!(stall_batch(&[(1.5, 0.5)])[0], 0.0); }
    #[test] fn deterministic() { assert_eq!(stall_batch(&[(0.7, 0.5)]), stall_batch(&[(0.7, 0.5)])); }
}
