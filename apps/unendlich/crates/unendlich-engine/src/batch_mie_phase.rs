//! Perf: batched Mie (Henyey–Greenstein) phase over many cosines at fixed g,
//! bit-identical to the scalar reference.

use std::f32::consts::PI;

pub fn mie_one(cos_theta: f32, g: f32) -> f32 {
    let c = cos_theta.clamp(-1.0, 1.0);
    let g = g.clamp(-0.99, 0.99);
    let num = 1.0 - g * g;
    let denom = 4.0 * PI * (1.0 + g * g - 2.0 * g * c).powf(1.5);
    (num / denom).max(0.0)
}

pub fn mie_batch(cosines: &[f32], g: f32) -> Vec<f32> {
    cosines.iter().map(|&c| mie_one(c, g)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn parity() {
        let cs = [-1.0f32, -0.3, 0.0, 0.4, 1.0];
        let b = mie_batch(&cs, 0.6);
        for (i, &c) in cs.iter().enumerate() { assert_eq!(b[i].to_bits(), mie_one(c, 0.6).to_bits()); }
    }
    #[test] fn empty_is_empty() { assert!(mie_batch(&[], 0.6).is_empty()); }
    #[test] fn single_eq_batch1() { assert_eq!(mie_batch(&[0.5], 0.6)[0].to_bits(), mie_one(0.5, 0.6).to_bits()); }
    #[test] fn isotropic_g0() { let v = mie_batch(&[-0.5, 0.5], 0.0); assert!((v[0] - v[1]).abs() < 1e-7); }
    #[test] fn positive() { for v in mie_batch(&[0.1, 0.9], 0.5) { assert!(v > 0.0); } }
    #[test] fn deterministic() { assert_eq!(mie_batch(&[0.5], 0.6), mie_batch(&[0.5], 0.6)); }
}
