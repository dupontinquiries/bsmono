//! Perf: batched armor mitigation over all hits in a frame, bit-identical to ref.

pub fn mitigated_one(raw: f32, armor: f32, k: f32) -> f32 {
    let r = raw.max(0.0);
    let a = armor.max(0.0);
    let kk = k.max(1e-4);
    r * kk / (a + kk)
}

pub fn mitigated_batch(items: &[(f32, f32, f32)]) -> Vec<f32> {
    items.iter().map(|&(r, a, k)| mitigated_one(r, a, k)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn parity() {
        let items = [(100.0f32, 0.0f32, 50.0f32), (100.0, 100.0, 50.0), (100.0, 20.0, 50.0), (80.0, 60.0, 40.0)];
        let b = mitigated_batch(&items);
        for (i, &it) in items.iter().enumerate() { assert_eq!(b[i].to_bits(), mitigated_one(it.0, it.1, it.2).to_bits()); }
    }
    #[test] fn empty_is_empty() { assert!(mitigated_batch(&[]).is_empty()); }
    #[test] fn single_eq_batch1() { assert_eq!(mitigated_batch(&[(100.0, 50.0, 50.0)])[0].to_bits(), mitigated_one(100.0, 50.0, 50.0).to_bits()); }
    #[test] fn zero_armor_full() { assert_eq!(mitigated_batch(&[(100.0, 0.0, 50.0)])[0], 100.0); }
    #[test] fn more_armor_less() { assert!(mitigated_batch(&[(100.0, 100.0, 50.0)])[0] < mitigated_batch(&[(100.0, 20.0, 50.0)])[0]); }
    #[test] fn deterministic() { assert_eq!(mitigated_batch(&[(100.0, 50.0, 50.0)]), mitigated_batch(&[(100.0, 50.0, 50.0)])); }
}
