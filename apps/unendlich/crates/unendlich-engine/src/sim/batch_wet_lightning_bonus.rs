//! Perf: batched wet-lightning bonus over all targets in a chain, bit-identical to ref.

pub fn bonus_one(lightning_dmg: f32, wet: f32, bonus_mult: f32) -> f32 {
    lightning_dmg.max(0.0) * (1.0 + wet.clamp(0.0, 1.0) * bonus_mult.max(0.0))
}

pub fn bonus_batch(items: &[(f32, f32, f32)]) -> Vec<f32> {
    items.iter().map(|&(d, w, m)| bonus_one(d, w, m)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn parity() {
        let items = [(100.0f32, 0.0f32, 1.0f32), (100.0, 1.0, 1.0), (100.0, 0.5, 1.0), (50.0, 0.7, 2.0)];
        let b = bonus_batch(&items);
        for (i, &it) in items.iter().enumerate() { assert_eq!(b[i].to_bits(), bonus_one(it.0, it.1, it.2).to_bits()); }
    }
    #[test] fn empty_is_empty() { assert!(bonus_batch(&[]).is_empty()); }
    #[test] fn single_eq_batch1() { assert_eq!(bonus_batch(&[(100.0, 0.5, 1.0)])[0].to_bits(), bonus_one(100.0, 0.5, 1.0).to_bits()); }
    #[test] fn dry_base() { assert_eq!(bonus_batch(&[(100.0, 0.0, 1.0)])[0], 100.0); }
    #[test] fn soaked_amplified() { assert!((bonus_batch(&[(100.0, 1.0, 1.0)])[0] - 200.0).abs() < 1e-4); }
    #[test] fn deterministic() { assert_eq!(bonus_batch(&[(100.0, 0.5, 1.0)]), bonus_batch(&[(100.0, 0.5, 1.0)])); }
}
