//! Perf: batched stat totals over a character's whole stat block, bit-identical to ref.

pub fn total_one(base: f32, equipment_bonus: f32, buff_mult: f32) -> f32 {
    (base.max(0.0) + equipment_bonus.max(0.0)) * buff_mult.max(0.0)
}

pub fn total_batch(items: &[(f32, f32, f32)]) -> Vec<f32> {
    items.iter().map(|&(b, e, m)| total_one(b, e, m)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn parity() {
        let items = [(10.0f32, 0.0f32, 1.0f32), (10.0, 5.0, 1.0), (10.0, 0.0, 2.0), (20.0, 8.0, 1.5)];
        let b = total_batch(&items);
        for (i, &it) in items.iter().enumerate() { assert_eq!(b[i].to_bits(), total_one(it.0, it.1, it.2).to_bits()); }
    }
    #[test] fn empty_is_empty() { assert!(total_batch(&[]).is_empty()); }
    #[test] fn single_eq_batch1() { assert_eq!(total_batch(&[(10.0, 5.0, 1.5)])[0].to_bits(), total_one(10.0, 5.0, 1.5).to_bits()); }
    #[test] fn base_only() { assert_eq!(total_batch(&[(10.0, 0.0, 1.0)])[0], 10.0); }
    #[test] fn equipment_adds() { assert_eq!(total_batch(&[(10.0, 5.0, 1.0)])[0], 15.0); }
    #[test] fn deterministic() { assert_eq!(total_batch(&[(10.0, 5.0, 1.5)]), total_batch(&[(10.0, 5.0, 1.5)])); }
}
