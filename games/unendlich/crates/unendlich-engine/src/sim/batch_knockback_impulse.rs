//! Perf: batched knockback over many hit entities, bit-identical to ref.

pub fn imp_one(damage: f32, mass: f32, resist: f32) -> f32 {
    let d = damage.max(0.0);
    let m = mass.max(0.1);
    let r = resist.clamp(0.0, 1.0);
    (d / m) * (1.0 - r)
}

pub fn imp_batch(items: &[(f32, f32, f32)]) -> Vec<f32> {
    items.iter().map(|&(d, m, r)| imp_one(d, m, r)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn parity() {
        let items = [(100.0f32, 10.0f32, 0.0f32), (20.0, 50.0, 0.5), (100.0, 10.0, 1.0), (50.0, 5.0, 0.2)];
        let b = imp_batch(&items);
        for (i, &it) in items.iter().enumerate() { assert_eq!(b[i].to_bits(), imp_one(it.0, it.1, it.2).to_bits()); }
    }
    #[test] fn empty_is_empty() { assert!(imp_batch(&[]).is_empty()); }
    #[test] fn single_eq_batch1() { assert_eq!(imp_batch(&[(100.0, 10.0, 0.5)])[0].to_bits(), imp_one(100.0, 10.0, 0.5).to_bits()); }
    #[test] fn more_damage_more() { assert!(imp_batch(&[(100.0, 10.0, 0.0)])[0] > imp_batch(&[(20.0, 10.0, 0.0)])[0]); }
    #[test] fn full_resist_zero() { assert_eq!(imp_batch(&[(100.0, 10.0, 1.0)])[0], 0.0); }
    #[test] fn deterministic() { assert_eq!(imp_batch(&[(100.0, 10.0, 0.5)]), imp_batch(&[(100.0, 10.0, 0.5)])); }
}
