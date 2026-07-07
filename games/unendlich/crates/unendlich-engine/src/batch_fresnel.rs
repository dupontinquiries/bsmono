//! Perf: batched Schlick Fresnel over many cosines at fixed F0, bit-identical to ref.

pub fn fresnel_one(cos_theta: f32, f0: f32) -> f32 {
    let c = cos_theta.clamp(0.0, 1.0);
    let f0 = f0.clamp(0.0, 1.0);
    let m = 1.0 - c;
    let m5 = m * m * m * m * m;
    (f0 + (1.0 - f0) * m5).clamp(0.0, 1.0)
}

pub fn fresnel_batch(cosines: &[f32], f0: f32) -> Vec<f32> {
    cosines.iter().map(|&c| fresnel_one(c, f0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn parity() {
        let cs = [0.0f32, 0.25, 0.5, 0.75, 1.0];
        let b = fresnel_batch(&cs, 0.04);
        for (i, &c) in cs.iter().enumerate() { assert_eq!(b[i].to_bits(), fresnel_one(c, 0.04).to_bits()); }
    }
    #[test] fn empty_is_empty() { assert!(fresnel_batch(&[], 0.04).is_empty()); }
    #[test] fn single_eq_batch1() { assert_eq!(fresnel_batch(&[0.5], 0.04)[0].to_bits(), fresnel_one(0.5, 0.04).to_bits()); }
    #[test] fn head_on_is_f0() { assert!((fresnel_batch(&[1.0], 0.04)[0] - 0.04).abs() < 1e-6); }
    #[test] fn bounded() { for v in fresnel_batch(&[0.0, 0.5, 1.0], 0.5) { assert!((0.0..=1.0).contains(&v)); } }
    #[test] fn deterministic() { assert_eq!(fresnel_batch(&[0.5], 0.04), fresnel_batch(&[0.5], 0.04)); }
}
