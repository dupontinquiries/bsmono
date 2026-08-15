//! Perf: batched depth linearization over a depth buffer row, bit-identical to ref.

pub fn z_one(ndc_depth: f32, near: f32, far: f32) -> f32 {
    let d = ndc_depth.clamp(0.0, 1.0);
    let n = near.max(1e-4);
    let f = far.max(n + 1e-4);
    (n * f) / (f - d * (f - n))
}

pub fn z_batch(depths: &[f32], near: f32, far: f32) -> Vec<f32> {
    #[cfg(target_arch = "aarch64")]
    {
        unsafe { z_batch_neon(depths, near, far) }
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        depths.iter().map(|&d| z_one(d, near, far)).collect()
    }
}

/// NEON 4-wide path, bit-identical to `z_one` per lane.
///
/// Parity rules (apply to every NEON kernel in this family):
/// - no `vfmaq_f32`: the scalar path compiles unfused mul+add, FMA changes bits
/// - no `vrecpeq`/`vrsqrteq` estimates: `vdivq_f32` is IEEE-exact, estimates aren't
/// - `.clamp(a, b)` mirrored as the same compare/select chain the scalar uses
///   (NaN in → NaN out), not as max-then-min
/// - identical association order per expression
///
/// `n*f` and `f-n` involve only the scalar `near`/`far` and are hoisted —
/// same bits as computing them per element.
#[cfg(target_arch = "aarch64")]
unsafe fn z_batch_neon(depths: &[f32], near: f32, far: f32) -> Vec<f32> {
    use core::arch::aarch64::*;

    let n = near.max(1e-4);
    let f = far.max(n + 1e-4);
    let num = n * f; // scalar-hoisted, bit-identical to per-element n*f
    let fmn = f - n;

    let len = depths.len();
    let mut out = vec![0.0f32; len];
    let mut i = 0usize;

    let zero = vdupq_n_f32(0.0);
    let one = vdupq_n_f32(1.0);
    let num_v = vdupq_n_f32(num);
    let f_v = vdupq_n_f32(f);
    let fmn_v = vdupq_n_f32(fmn);

    while i + 4 <= len {
        let d_raw = vld1q_f32(depths.as_ptr().add(i));
        // d = ndc_depth.clamp(0.0, 1.0): if d < 0 { 0 } else if d > 1 { 1 } else { d }
        let d = vbslq_f32(
            vcltq_f32(d_raw, zero),
            zero,
            vbslq_f32(vcgtq_f32(d_raw, one), one, d_raw),
        );
        // (n*f) / (f - d*(f-n))
        let den = vsubq_f32(f_v, vmulq_f32(d, fmn_v));
        let z = vdivq_f32(num_v, den);
        vst1q_f32(out.as_mut_ptr().add(i), z);
        i += 4;
    }
    while i < len {
        out[i] = z_one(depths[i], near, far);
        i += 1;
    }
    out
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

    #[test]
    fn simd_parity_random_lengths() {
        // Exercise full 4-lane blocks and every tail remainder; on aarch64 this
        // is a NEON-vs-scalar differential test, elsewhere it's a no-op check.
        let mut s: u32 = 0xC0FFEE01;
        let mut rng = || { s ^= s << 13; s ^= s >> 17; s ^= s << 5; (s >> 8) as f32 / (1u32 << 24) as f32 };
        for n in 0..67usize {
            let ds: Vec<f32> = (0..n).map(|_| rng() * 1.4 - 0.2).collect(); // includes out-of-[0,1]
            let b = z_batch(&ds, 0.1, 100.0);
            for (i, &d) in ds.iter().enumerate() {
                assert_eq!(b[i].to_bits(), z_one(d, 0.1, 100.0).to_bits(), "n={n} i={i}");
            }
        }
    }
}
