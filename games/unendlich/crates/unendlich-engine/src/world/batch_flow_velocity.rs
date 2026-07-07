//! Perf: batched shallow-water flow speed over a height grid, bit-identical to ref.

pub fn vel_one(height_high: f32, height_low: f32, distance: f32) -> f32 {
    let drop = (height_high - height_low).max(0.0);
    let dist = distance.max(1e-4);
    (drop / dist).max(0.0).sqrt()
}

pub fn vel_batch(items: &[(f32, f32, f32)]) -> Vec<f32> {
    items.iter().map(|&(h, l, d)| vel_one(h, l, d)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn parity() {
        let items = [(10.0f32, 10.0f32, 1.0f32), (20.0, 10.0, 1.0), (5.0, 10.0, 1.0), (20.0, 10.0, 5.0)];
        let b = vel_batch(&items);
        for (i, &it) in items.iter().enumerate() { assert_eq!(b[i].to_bits(), vel_one(it.0, it.1, it.2).to_bits()); }
    }
    #[test] fn empty_is_empty() { assert!(vel_batch(&[]).is_empty()); }
    #[test] fn single_eq_batch1() { assert_eq!(vel_batch(&[(20.0, 10.0, 1.0)])[0].to_bits(), vel_one(20.0, 10.0, 1.0).to_bits()); }
    #[test] fn no_drop_zero() { assert_eq!(vel_batch(&[(10.0, 10.0, 1.0)])[0], 0.0); }
    #[test] fn steeper_faster() { assert!(vel_batch(&[(20.0, 10.0, 1.0)])[0] > vel_batch(&[(12.0, 10.0, 1.0)])[0]); }
    #[test] fn deterministic() { assert_eq!(vel_batch(&[(20.0, 10.0, 1.0)]), vel_batch(&[(20.0, 10.0, 1.0)])); }
}
