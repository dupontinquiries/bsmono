//! Perf: batched affix magnitudes over an item's affix list, bit-identical to ref.

pub fn mag_one(base: f32, item_level: u32, roll: f32) -> f32 {
    let b = base.max(0.0);
    let lvl = 1.0 + item_level as f32 / 100.0;
    let r = 0.5 + 0.5 * roll.clamp(0.0, 1.0);
    b * lvl * r
}

pub fn mag_batch(items: &[(f32, u32, f32)]) -> Vec<f32> {
    items.iter().map(|&(b, l, r)| mag_one(b, l, r)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn parity() {
        let items = [(100.0f32, 0u32, 0.0f32), (100.0, 0, 1.0), (100.0, 100, 1.0), (50.0, 30, 0.6)];
        let b = mag_batch(&items);
        for (i, &it) in items.iter().enumerate() { assert_eq!(b[i].to_bits(), mag_one(it.0, it.1, it.2).to_bits()); }
    }
    #[test] fn empty_is_empty() { assert!(mag_batch(&[]).is_empty()); }
    #[test] fn single_eq_batch1() { assert_eq!(mag_batch(&[(100.0, 50, 0.7)])[0].to_bits(), mag_one(100.0, 50, 0.7).to_bits()); }
    #[test] fn min_roll_half() { assert!((mag_batch(&[(100.0, 0, 0.0)])[0] - 50.0).abs() < 1e-4); }
    #[test] fn level_scales() { assert!((mag_batch(&[(100.0, 100, 1.0)])[0] - 200.0).abs() < 1e-4); }
    #[test] fn deterministic() { assert_eq!(mag_batch(&[(100.0, 50, 0.7)]), mag_batch(&[(100.0, 50, 0.7)])); }
}
