//! batch_light_falloff.rs — pure batched point-light attenuation (microopt of R22).
//!
//! Scalar `atten_one` (mirroring R22 `light_falloff::atten`: inverse-square core
//! × a smooth window to 0 at `radius`) plus a batched `atten_batch` that is
//! **bit-for-bit identical** to the scalar path applied per distance. Pure,
//! deterministic, panic-free (R1/R11/R21).
//!
//! Perf note: attenuating one light over many points/fragments per frame, the
//! batch hoists only the output allocation (`with_capacity` + a tight loop),
//! running the identical per-distance math, so the factors match the scalar
//! reference bit-for-bit.

/// Scalar reference: attenuation in `[0,1]`, 1 at the source → 0 at `radius`.
pub fn atten_one(dist: f32, radius: f32) -> f32 {
    let r = radius.max(1e-4);
    let d = dist.max(0.0);
    if d >= r {
        return 0.0;
    }
    let inv = 1.0 / (1.0 + d * d);
    let w = (1.0 - (d / r).powi(4)).max(0.0);
    (inv * w * w).max(0.0).min(1.0)
}

/// Batched form over `dists` sharing one `radius`, routed through `atten_one`
/// (bit-parity).
pub fn atten_batch(dists: &[f32], radius: f32) -> Vec<f32> {
    let mut out = Vec::with_capacity(dists.len());
    for &d in dists {
        out.push(atten_one(d, radius));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> f32 {
        *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (*seed >> 8) as f32 / ((1u32 << 24) as f32)
    }

    #[test]
    fn batch_is_bit_parity_with_scalar() {
        let mut seed = 0x2357_0001u32;
        let dists: Vec<f32> = (0..1000).map(|_| lcg(&mut seed) * 25.0 - 2.0).collect();
        let r = 20.0f32;
        let batched = atten_batch(&dists, r);
        assert_eq!(batched.len(), dists.len());
        for (i, &d) in dists.iter().enumerate() {
            assert_eq!(batched[i].to_bits(), atten_one(d, r).to_bits(), "mismatch at {}", i);
        }
    }

    #[test]
    fn empty_is_empty() {
        assert!(atten_batch(&[], 10.0).is_empty());
    }

    #[test]
    fn single_equals_batch_of_one() {
        assert_eq!(atten_batch(&[3.0], 10.0)[0].to_bits(), atten_one(3.0, 10.0).to_bits());
    }

    #[test]
    fn bounded_unit_range() {
        let mut seed = 0x2357_0002u32;
        let dists: Vec<f32> = (0..1000).map(|_| lcg(&mut seed) * 40.0 - 5.0).collect();
        for a in atten_batch(&dists, 15.0) {
            assert!((0.0..=1.0).contains(&a), "atten oob {}", a);
        }
    }

    #[test]
    fn deterministic() {
        let mut seed = 0x2357_0003u32;
        let dists: Vec<f32> = (0..64).map(|_| lcg(&mut seed) * 20.0).collect();
        assert_eq!(atten_batch(&dists, 10.0), atten_batch(&dists, 10.0));
    }

    #[test]
    fn endpoints_parity() {
        for &d in &[0.0f32, 5.0, 10.0, 20.0] {
            assert_eq!(atten_batch(&[d], 10.0)[0].to_bits(), atten_one(d, 10.0).to_bits());
        }
    }
}
