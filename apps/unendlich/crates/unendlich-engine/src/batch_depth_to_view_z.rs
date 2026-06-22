//! Perf: batched depth linearization over a depth buffer row, bit-identical to ref.

pub fn z_one(ndc_depth: f32, near: f32, far: f32) -> f32 {
    let d = ndc_depth.clamp(0.0, 1.0);
    let n = near.max(1e-4);
    let f = far.max(n + 1e-4);
    (n * f) / (f - d * (f - n))
}

pub fn z_batch(depths: &[f32], near: f32, far: f32) -> Vec<f32> {
    depths.iter().map(|&d| z_one(d, near, far)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn parity() {
        let ds = [0.0f32, 0.25, 0.5, 0.75, 1.0];
        let b = z_batch(&ds, 0.1, 100.0);
        for (i, &d) in ds.iter().enumerate() { assert_eq!(b[i].to_bits(), z_one(d, 0.1, 100.0).to_bits()); }
    }
    #[test] fn empty_is_empty() { assert!(z_batch(&[], 0.1, 100.0).is_empty()); }
    #[test] fn single_eq_batch1() { assert_eq!(z_batch(&[0.5], 0.1, 100.0)[0].to_bits(), z_one(0.5, 0.1, 100.0).to_bits()); }
    #[test] fn depth0_near() { assert!((z_batch(&[0.0], 0.1, 100.0)[0] - 0.1).abs() < 1e-3); }
    #[test] fn depth1_far() { assert!((z_batch(&[1.0], 0.1, 100.0)[0] - 100.0).abs() < 1e-1); }
    #[test] fn deterministic() { assert_eq!(z_batch(&[0.5], 0.1, 100.0), z_batch(&[0.5], 0.1, 100.0)); }
}
