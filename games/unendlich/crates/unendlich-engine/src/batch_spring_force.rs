//! Perf: batched Hooke forces over all springs in a rope/cloth, bit-identical to ref.

pub fn force_one(displacement: f32, stiffness: f32) -> f32 {
    -stiffness.max(0.0) * displacement
}

pub fn force_batch(items: &[(f32, f32)]) -> Vec<f32> {
    #[cfg(target_arch = "aarch64")]
    {
        unsafe { force_batch_neon(items) }
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        items.iter().map(|&(d, k)| force_one(d, k)).collect()
    }
}

/// NEON 4-wide path, bit-identical to `force_one` per lane.
/// `-stiffness.max(0.0) * displacement` → `vnegq(vmaxnmq(k, 0)) * d`.
/// Tuple deinterleave via stack arrays (see `batch_verlet_step` for why).
#[cfg(target_arch = "aarch64")]
unsafe fn force_batch_neon(items: &[(f32, f32)]) -> Vec<f32> {
    use core::arch::aarch64::*;

    let len = items.len();
    let mut out = vec![0.0f32; len];
    let mut i = 0usize;
    let zero = vdupq_n_f32(0.0);

    while i + 4 <= len {
        let mut ds = [0.0f32; 4];
        let mut ks = [0.0f32; 4];
        for j in 0..4 {
            let (d, k) = items[i + j];
            ds[j] = d;
            ks[j] = k;
        }
        let d = vld1q_f32(ds.as_ptr());
        let k = vld1q_f32(ks.as_ptr());
        let f = vmulq_f32(vnegq_f32(vmaxnmq_f32(k, zero)), d);
        vst1q_f32(out.as_mut_ptr().add(i), f);
        i += 4;
    }
    while i < len {
        let (d, k) = items[i];
        out[i] = force_one(d, k);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn parity() {
        let items = [(0.0f32, 10.0f32), (2.0, 10.0), (-2.0, 10.0), (4.0, 20.0)];
        let b = force_batch(&items);
        for (i, &it) in items.iter().enumerate() { assert_eq!(b[i].to_bits(), force_one(it.0, it.1).to_bits()); }
    }
    #[test] fn empty_is_empty() { assert!(force_batch(&[]).is_empty()); }
    #[test] fn single_eq_batch1() { assert_eq!(force_batch(&[(2.0, 10.0)])[0].to_bits(), force_one(2.0, 10.0).to_bits()); }
    #[test] fn rest_zero() { assert_eq!(force_batch(&[(0.0, 10.0)])[0], 0.0); }
    #[test] fn stretched_pulls_back() { assert!(force_batch(&[(2.0, 10.0)])[0] < 0.0); }
    #[test] fn deterministic() { assert_eq!(force_batch(&[(2.0, 10.0)]), force_batch(&[(2.0, 10.0)])); }

    #[test]
    fn simd_parity_random_lengths() {
        let mut s: u32 = 0xC0FFEE04;
        let mut rng = || { s ^= s << 13; s ^= s >> 17; s ^= s << 5; (s >> 8) as f32 / (1u32 << 24) as f32 * 20.0 - 10.0 };
        for n in 0..67usize {
            let items: Vec<(f32, f32)> = (0..n).map(|_| (rng(), rng())).collect(); // negative stiffness too
            let b = force_batch(&items);
            for (i, &(d, k)) in items.iter().enumerate() {
                assert_eq!(b[i].to_bits(), force_one(d, k).to_bits(), "n={n} i={i}");
            }
        }
    }
}
