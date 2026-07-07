//! Perf: batched tidal height over many sample times, bit-identical to ref.

use std::f32::consts::PI;

pub fn level_one(time: f32, period: f32, amplitude: f32) -> f32 {
    let p = period.max(1e-6);
    amplitude.max(0.0) * (2.0 * PI * time / p).sin()
}

pub fn level_batch(times: &[f32], period: f32, amplitude: f32) -> Vec<f32> {
    times.iter().map(|&t| level_one(t, period, amplitude)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn parity() {
        let ts = [0.0f32, 3.0, 6.0, 9.0, 12.0];
        let b = level_batch(&ts, 12.0, 2.0);
        for (i, &t) in ts.iter().enumerate() { assert_eq!(b[i].to_bits(), level_one(t, 12.0, 2.0).to_bits()); }
    }
    #[test] fn empty_is_empty() { assert!(level_batch(&[], 12.0, 2.0).is_empty()); }
    #[test] fn single_eq_batch1() { assert_eq!(level_batch(&[3.0], 12.0, 2.0)[0].to_bits(), level_one(3.0, 12.0, 2.0).to_bits()); }
    #[test] fn zero_amp_zeros() { assert_eq!(level_batch(&[1.0, 2.0], 12.0, 0.0), vec![0.0, 0.0]); }
    #[test] fn bounded() { for v in level_batch(&[0.0, 3.0, 6.0], 12.0, 2.0) { assert!(v.abs() <= 2.0 + 1e-6); } }
    #[test] fn deterministic() { assert_eq!(level_batch(&[3.0], 12.0, 2.0), level_batch(&[3.0], 12.0, 2.0)); }
}
