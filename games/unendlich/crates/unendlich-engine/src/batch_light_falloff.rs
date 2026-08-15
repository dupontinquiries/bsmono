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
    #[cfg(target_arch = "aarch64")]
    {
        unsafe { atten_batch_neon(dists, radius) }
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        let mut out = Vec::with_capacity(dists.len());
        for &d in dists {
            out.push(atten_one(d, radius));
        }
        out
    }
}

/// NEON 4-wide path, bit-identical to `atten_one` per lane.
///
/// - `powi(4)` in the scalar path lowers to repeated squaring `(q*q)*(q*q)`;
///   mirrored explicitly here.
/// - `.max()`/`.min()` map to `vmaxnmq`/`vminnmq` (IEEE maxNum, same
///   NaN-ignoring semantics as Rust's `f32::max`/`min`).
/// - The scalar early-return `d >= r → 0.0` becomes a select; the speculatively
///   computed lanes are finite for all inputs (d=∞ gives inv=0, w=0), and the
///   select masks them regardless. No FMA, no reciprocal estimates.
#[cfg(target_arch = "aarch64")]
unsafe fn atten_batch_neon(dists: &[f32], radius: f32) -> Vec<f32> {
    use core::arch::aarch64::*;

    let r = radius.max(1e-4);
    let len = dists.len();
    let mut out = vec![0.0f32; len];
    let mut i = 0usize;

    let zero = vdupq_n_f32(0.0);
    let one = vdupq_n_f32(1.0);
    let r_v = vdupq_n_f32(r);

    while i + 4 <= len {
        let d_raw = vld1q_f32(dists.as_ptr().add(i));
        let d = vmaxnmq_f32(d_raw, zero); // dist.max(0.0)
        let cut = vcgeq_f32(d, r_v); // d >= r → 0.0

        // inv = 1.0 / (1.0 + d * d)
        let inv = vdivq_f32(one, vaddq_f32(one, vmulq_f32(d, d)));
        // w = (1.0 - (d/r).powi(4)).max(0.0), powi(4) = (q*q)*(q*q)
        let q = vdivq_f32(d, r_v);
        let q2 = vmulq_f32(q, q);
        let q4 = vmulq_f32(q2, q2);
        let w = vmaxnmq_f32(vsubq_f32(one, q4), zero);
        // (inv * w * w).max(0.0).min(1.0) — left-assoc (inv*w)*w
        let a = vminnmq_f32(vmaxnmq_f32(vmulq_f32(vmulq_f32(inv, w), w), zero), one);

        vst1q_f32(out.as_mut_ptr().add(i), vbslq_f32(cut, zero, a));
        i += 4;
    }
    while i < len {
        out[i] = atten_one(dists[i], radius);
        i += 1;
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

    #[test]
    fn simd_parity_random_lengths() {
        let mut s: u32 = 0xC0FFEE02;
        let mut rng = || { s ^= s << 13; s ^= s >> 17; s ^= s << 5; (s >> 8) as f32 / (1u32 << 24) as f32 };
        for n in 0..67usize {
            let ds: Vec<f32> = (0..n).map(|_| rng() * 30.0 - 3.0).collect(); // negatives + beyond radius
            let b = atten_batch(&ds, 20.0);
            for (i, &d) in ds.iter().enumerate() {
                assert_eq!(b[i].to_bits(), atten_one(d, 20.0).to_bits(), "n={n} i={i}");
            }
        }
    }
}
