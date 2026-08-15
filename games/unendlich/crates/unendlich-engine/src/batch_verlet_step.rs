//! Perf: batched Verlet integration over all cloth particles, bit-identical to ref.

pub fn step_one(pos: f32, prev_pos: f32, accel: f32, dt: f32) -> f32 {
    let t = dt.max(0.0);
    pos + (pos - prev_pos) + accel * t * t
}

pub fn step_batch(items: &[(f32, f32, f32)], dt: f32) -> Vec<f32> {
    #[cfg(target_arch = "aarch64")]
    {
        unsafe { step_batch_neon(items, dt) }
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        items.iter().map(|&(p, pp, a)| step_one(p, pp, a, dt)).collect()
    }
}

/// NEON 4-wide path, bit-identical to `step_one` per lane.
///
/// Deinterleave note: input is AoS tuples. `(f32, f32, f32)` tuple layout is
/// not guaranteed by the language, so no pointer-casting into the slice; and
/// `vld3q_f32` spent years unstable in stdarch, so no structured loads either.
/// Lanes are gathered through small stack arrays + `vld1q_f32` — costs a few
/// moves per block but stays safe, stable-Rust, and MSRV-friendly, and the
/// arithmetic (the actual win) is still 4-wide.
///
/// Association mirrors the scalar exactly: `(p + (p - pp)) + ((a*t)*t)` —
/// note `(a*t)*t`, NOT `a*(t*t)`; f32 mul is non-associative. No FMA.
#[cfg(target_arch = "aarch64")]
unsafe fn step_batch_neon(items: &[(f32, f32, f32)], dt: f32) -> Vec<f32> {
    use core::arch::aarch64::*;

    let t = dt.max(0.0);
    let len = items.len();
    let mut out = vec![0.0f32; len];
    let mut i = 0usize;

    let t_v = vdupq_n_f32(t);

    while i + 4 <= len {
        let mut ps = [0.0f32; 4];
        let mut pps = [0.0f32; 4];
        let mut accs = [0.0f32; 4];
        for k in 0..4 {
            let (p, pp, a) = items[i + k];
            ps[k] = p;
            pps[k] = pp;
            accs[k] = a;
        }
        let p = vld1q_f32(ps.as_ptr());
        let pp = vld1q_f32(pps.as_ptr());
        let a = vld1q_f32(accs.as_ptr());

        // pos + (pos - prev_pos) + accel * t * t  →  (p + (p - pp)) + ((a*t)*t)
        let att = vmulq_f32(vmulq_f32(a, t_v), t_v);
        let res = vaddq_f32(vaddq_f32(p, vsubq_f32(p, pp)), att);
        vst1q_f32(out.as_mut_ptr().add(i), res);
        i += 4;
    }
    while i < len {
        let (p, pp, a) = items[i];
        out[i] = step_one(p, pp, a, dt);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn parity() {
        let items = [(5.0f32, 5.0f32, 0.0f32), (6.0, 5.0, 0.0), (5.0, 5.0, 10.0), (3.0, 4.0, -9.8)];
        let b = step_batch(&items, 0.1);
        for (i, &it) in items.iter().enumerate() { assert_eq!(b[i].to_bits(), step_one(it.0, it.1, it.2, 0.1).to_bits()); }
    }
    #[test] fn empty_is_empty() { assert!(step_batch(&[], 0.1).is_empty()); }
    #[test] fn single_eq_batch1() { assert_eq!(step_batch(&[(6.0, 5.0, 0.0)], 0.1)[0].to_bits(), step_one(6.0, 5.0, 0.0, 0.1).to_bits()); }
    #[test] fn at_rest_stays() { assert_eq!(step_batch(&[(5.0, 5.0, 0.0)], 0.1)[0], 5.0); }
    #[test] fn momentum_continues() { assert!((step_batch(&[(6.0, 5.0, 0.0)], 0.1)[0] - 7.0).abs() < 1e-6); }
    #[test] fn deterministic() { assert_eq!(step_batch(&[(6.0, 5.0, 0.0)], 0.1), step_batch(&[(6.0, 5.0, 0.0)], 0.1)); }

    #[test]
    fn simd_parity_random_lengths() {
        let mut s: u32 = 0xC0FFEE03;
        let mut rng = || { s ^= s << 13; s ^= s >> 17; s ^= s << 5; (s >> 8) as f32 / (1u32 << 24) as f32 * 20.0 - 10.0 };
        for n in 0..67usize {
            let items: Vec<(f32, f32, f32)> = (0..n).map(|_| (rng(), rng(), rng())).collect();
            for dt in [0.0f32, 0.016, 0.1, -1.0] {
                let b = step_batch(&items, dt);
                for (i, &(p, pp, a)) in items.iter().enumerate() {
                    assert_eq!(b[i].to_bits(), step_one(p, pp, a, dt).to_bits(), "n={n} i={i} dt={dt}");
                }
            }
        }
    }
}
