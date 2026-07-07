//! Perf: batched Gerstner-wave evaluation over many phases, bit-identical to ref.

pub fn wave_one(phase: f32, steepness: f32, amplitude: f32) -> (f32, f32) {
    let q = steepness.clamp(0.0, 1.0);
    let a = amplitude.max(0.0);
    (q * a * phase.cos(), a * phase.sin())
}

pub fn wave_batch(phases: &[f32], steepness: f32, amplitude: f32) -> Vec<(f32, f32)> {
    phases.iter().map(|&p| wave_one(p, steepness, amplitude)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn parity() {
        let ps = [0.0f32, 1.0, 2.0, 3.14, 5.0];
        let b = wave_batch(&ps, 0.5, 2.0);
        for (i, &p) in ps.iter().enumerate() {
            let s = wave_one(p, 0.5, 2.0);
            assert_eq!(b[i].0.to_bits(), s.0.to_bits());
            assert_eq!(b[i].1.to_bits(), s.1.to_bits());
        }
    }
    #[test] fn empty_is_empty() { assert!(wave_batch(&[], 0.5, 2.0).is_empty()); }
    #[test] fn single_eq_batch1() { assert_eq!(wave_batch(&[1.0], 0.5, 2.0)[0].1.to_bits(), wave_one(1.0, 0.5, 2.0).1.to_bits()); }
    #[test] fn zero_amp_zeros() { assert_eq!(wave_batch(&[1.0, 2.0], 0.5, 0.0), vec![(0.0, 0.0), (0.0, 0.0)]); }
    #[test] fn height_bounded() { for (_, h) in wave_batch(&[0.0, 1.5, 3.0], 0.5, 2.0) { assert!(h.abs() <= 2.0 + 1e-6); } }
    #[test] fn deterministic() { assert_eq!(wave_batch(&[1.0], 0.5, 2.0), wave_batch(&[1.0], 0.5, 2.0)); }
}
