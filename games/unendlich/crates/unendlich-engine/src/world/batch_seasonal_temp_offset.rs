//! Perf: batched seasonal temperature offset across regions, bit-identical to ref.

pub fn off_one(season_phase: f32, amplitude: f32) -> f32 {
    season_phase.clamp(-1.0, 1.0) * amplitude.max(0.0)
}

pub fn off_batch(items: &[(f32, f32)]) -> Vec<f32> {
    items.iter().map(|&(s, a)| off_one(s, a)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn parity() {
        let items = [(1.0f32, 15.0f32), (-1.0, 15.0), (0.0, 20.0), (0.5, 8.0)];
        let b = off_batch(&items);
        for (i, &it) in items.iter().enumerate() { assert_eq!(b[i].to_bits(), off_one(it.0, it.1).to_bits()); }
    }
    #[test] fn empty_is_empty() { assert!(off_batch(&[]).is_empty()); }
    #[test] fn single_eq_batch1() { assert_eq!(off_batch(&[(0.5, 15.0)])[0].to_bits(), off_one(0.5, 15.0).to_bits()); }
    #[test] fn summer_positive() { assert!(off_batch(&[(1.0, 15.0)])[0] > 0.0); }
    #[test] fn equinox_zero() { assert_eq!(off_batch(&[(0.0, 15.0)])[0], 0.0); }
    #[test] fn deterministic() { assert_eq!(off_batch(&[(0.5, 15.0)]), off_batch(&[(0.5, 15.0)])); }
}
