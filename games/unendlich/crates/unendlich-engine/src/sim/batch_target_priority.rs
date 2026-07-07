//! Perf: batched target-priority scores over all candidate targets, bit-identical to ref.

pub fn priority_one(threat: f32, distance: f32, target_hp_frac: f32, low_hp_weight: f32) -> f32 {
    let t = threat.max(0.0);
    let prox = 1.0 / (1.0 + distance.max(0.0));
    let hp = target_hp_frac.clamp(0.0, 1.0);
    let finisher = 1.0 + low_hp_weight.max(0.0) * (1.0 - hp);
    t * prox * finisher
}

pub fn priority_batch(items: &[(f32, f32, f32, f32)]) -> Vec<f32> {
    items.iter().map(|&(t, d, h, w)| priority_one(t, d, h, w)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn parity() {
        let items = [(10.0f32, 5.0f32, 1.0f32, 1.0f32), (2.0, 5.0, 1.0, 1.0), (10.0, 1.0, 0.1, 1.0), (5.0, 3.0, 0.5, 2.0)];
        let b = priority_batch(&items);
        for (i, &it) in items.iter().enumerate() { assert_eq!(b[i].to_bits(), priority_one(it.0, it.1, it.2, it.3).to_bits()); }
    }
    #[test] fn empty_is_empty() { assert!(priority_batch(&[]).is_empty()); }
    #[test] fn single_eq_batch1() { assert_eq!(priority_batch(&[(10.0, 5.0, 0.5, 1.0)])[0].to_bits(), priority_one(10.0, 5.0, 0.5, 1.0).to_bits()); }
    #[test] fn zero_threat_zero() { assert_eq!(priority_batch(&[(0.0, 5.0, 0.5, 1.0)])[0], 0.0); }
    #[test] fn higher_threat_more() { assert!(priority_batch(&[(10.0, 5.0, 1.0, 1.0)])[0] > priority_batch(&[(2.0, 5.0, 1.0, 1.0)])[0]); }
    #[test] fn deterministic() { assert_eq!(priority_batch(&[(10.0, 5.0, 0.5, 1.0)]), priority_batch(&[(10.0, 5.0, 0.5, 1.0)])); }
}
