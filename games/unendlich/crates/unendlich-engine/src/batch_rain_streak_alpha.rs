//! Perf: batched rain-streak alpha over all streaks in view, bit-identical to ref.

pub fn alpha_one(intensity: f32, distance: f32, fade_dist: f32) -> f32 {
    let i = intensity.clamp(0.0, 1.0);
    let d = distance.max(0.0);
    let fd = fade_dist.max(1e-4);
    (i * (1.0 - d / fd).clamp(0.0, 1.0)).clamp(0.0, 1.0)
}

pub fn alpha_batch(items: &[(f32, f32, f32)]) -> Vec<f32> {
    items.iter().map(|&(i, d, f)| alpha_one(i, d, f)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn parity() {
        let items = [(0.0f32, 1.0f32, 50.0f32), (1.0, 0.0, 50.0), (1.0, 40.0, 50.0), (0.6, 20.0, 80.0)];
        let b = alpha_batch(&items);
        for (i, &it) in items.iter().enumerate() { assert_eq!(b[i].to_bits(), alpha_one(it.0, it.1, it.2).to_bits()); }
    }
    #[test] fn empty_is_empty() { assert!(alpha_batch(&[]).is_empty()); }
    #[test] fn single_eq_batch1() { assert_eq!(alpha_batch(&[(0.5, 10.0, 50.0)])[0].to_bits(), alpha_one(0.5, 10.0, 50.0).to_bits()); }
    #[test] fn no_rain_clear() { assert_eq!(alpha_batch(&[(0.0, 1.0, 50.0)])[0], 0.0); }
    #[test] fn beyond_fade_zero() { assert_eq!(alpha_batch(&[(1.0, 60.0, 50.0)])[0], 0.0); }
    #[test] fn deterministic() { assert_eq!(alpha_batch(&[(0.5, 10.0, 50.0)]), alpha_batch(&[(0.5, 10.0, 50.0)])); }
}
