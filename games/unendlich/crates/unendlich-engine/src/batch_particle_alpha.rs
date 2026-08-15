//! Perf: batched particle alpha over a whole emitter, bit-identical to ref.

pub fn alpha_one(age: f32, lifetime: f32) -> f32 {
    let lt = lifetime.max(1e-6);
    let t = (age.max(0.0) / lt).clamp(0.0, 1.0);
    let a = if t < 0.15 { t / 0.15 } else if t > 0.7 { (1.0 - t) / 0.3 } else { 1.0 };
    a.clamp(0.0, 1.0)
}

pub fn alpha_batch(items: &[(f32, f32)]) -> Vec<f32> {
    #[cfg(target_arch = "aarch64")]
    {
        unsafe { alpha_batch_neon(items) }
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        items.iter().map(|&(age, lt)| alpha_one(age, lt)).collect()
    }
}

/// NEON 4-wide path, bit-identical to `alpha_one` per lane.
/// The scalar piecewise `if t < 0.15 {..} else if t > 0.7 {..} else { 1.0 }`
/// becomes nested selects with the same branch order; both ramps are computed
/// speculatively (t is clamped to [0,1] first, so both are finite). Divisions
/// stay `vdivq_f32` — multiplying by 1/0.15 or 1/0.3 would change bits.
/// Tuple deinterleave via stack arrays (see `batch_verlet_step`).
#[cfg(target_arch = "aarch64")]
unsafe fn alpha_batch_neon(items: &[(f32, f32)]) -> Vec<f32> {
    use core::arch::aarch64::*;

    let len = items.len();
    let mut out = vec![0.0f32; len];
    let mut i = 0usize;
    let zero = vdupq_n_f32(0.0);
    let one = vdupq_n_f32(1.0);
    let eps = vdupq_n_f32(1e-6);
    let lo = vdupq_n_f32(0.15);
    let hi = vdupq_n_f32(0.7);
    let fall = vdupq_n_f32(0.3);

    // clamp(a, b) mirrored as the scalar compare/select chain (NaN-preserving).
    #[inline(always)]
    unsafe fn clamp01(x: float32x4_t, zero: float32x4_t, one: float32x4_t) -> float32x4_t {
        vbslq_f32(
            vcltq_f32(x, zero),
            zero,
            vbslq_f32(vcgtq_f32(x, one), one, x),
        )
    }

    while i + 4 <= len {
        let mut ages = [0.0f32; 4];
        let mut lts = [0.0f32; 4];
        for j in 0..4 {
            let (age, lt) = items[i + j];
            ages[j] = age;
            lts[j] = lt;
        }
        let lt = vmaxnmq_f32(vld1q_f32(lts.as_ptr()), eps); // lifetime.max(1e-6)
        let age = vmaxnmq_f32(vld1q_f32(ages.as_ptr()), zero); // age.max(0.0)
        let t = clamp01(vdivq_f32(age, lt), zero, one);

        // if t < 0.15 { t/0.15 } else if t > 0.7 { (1-t)/0.3 } else { 1.0 }
        let ramp_in = vdivq_f32(t, lo);
        let ramp_out = vdivq_f32(vsubq_f32(one, t), fall);
        let a = vbslq_f32(
            vcltq_f32(t, lo),
            ramp_in,
            vbslq_f32(vcgtq_f32(t, hi), ramp_out, one),
        );
        vst1q_f32(out.as_mut_ptr().add(i), clamp01(a, zero, one));
        i += 4;
    }
    while i < len {
        let (age, lt) = items[i];
        out[i] = alpha_one(age, lt);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn parity() {
        let items = [(0.0f32, 10.0f32), (1.0, 10.0), (5.0, 10.0), (9.0, 10.0), (10.0, 10.0)];
        let b = alpha_batch(&items);
        for (i, &it) in items.iter().enumerate() { assert_eq!(b[i].to_bits(), alpha_one(it.0, it.1).to_bits()); }
    }
    #[test] fn empty_is_empty() { assert!(alpha_batch(&[]).is_empty()); }
    #[test] fn single_eq_batch1() { assert_eq!(alpha_batch(&[(5.0, 10.0)])[0].to_bits(), alpha_one(5.0, 10.0).to_bits()); }
    #[test] fn mid_full() { assert!((alpha_batch(&[(5.0, 10.0)])[0] - 1.0).abs() < 1e-6); }
    #[test] fn death_zero() { assert!(alpha_batch(&[(10.0, 10.0)])[0].abs() < 1e-6); }
    #[test] fn deterministic() { assert_eq!(alpha_batch(&[(5.0, 10.0)]), alpha_batch(&[(5.0, 10.0)])); }

    #[test]
    fn simd_parity_random_lengths() {
        let mut s: u32 = 0xC0FFEE06;
        let mut rng = || { s ^= s << 13; s ^= s >> 17; s ^= s << 5; (s >> 8) as f32 / (1u32 << 24) as f32 };
        for n in 0..67usize {
            // ages straddle every ramp region; lifetimes include <=0 (eps-clamped path)
            let items: Vec<(f32, f32)> = (0..n).map(|_| (rng() * 12.0 - 1.0, rng() * 11.0 - 0.5)).collect();
            let b = alpha_batch(&items);
            for (i, &(age, lt)) in items.iter().enumerate() {
                assert_eq!(b[i].to_bits(), alpha_one(age, lt).to_bits(), "n={n} i={i}");
            }
        }
    }
}
