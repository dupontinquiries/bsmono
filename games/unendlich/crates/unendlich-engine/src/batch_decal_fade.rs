//! Perf: batched decal fade over many projected fragments, bit-identical to ref.

pub fn fade_one(distance_to_surface: f32, thickness: f32) -> f32 {
    let t = thickness.max(1e-6);
    let d = distance_to_surface.abs();
    if d >= t { 0.0 } else { (1.0 - d / t).clamp(0.0, 1.0) }
}

pub fn fade_batch(items: &[(f32, f32)]) -> Vec<f32> {
    #[cfg(target_arch = "aarch64")]
    {
        unsafe { fade_batch_neon(items) }
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        items.iter().map(|&(d, t)| fade_one(d, t)).collect()
    }
}

/// NEON 4-wide path, bit-identical to `fade_one` per lane.
/// The scalar early-return `d >= t → 0.0` becomes a select; `.clamp(0,1)` is
/// mirrored as the scalar's compare/select chain. Tuple deinterleave via stack
/// arrays (see `batch_verlet_step`). No FMA, no reciprocal estimates.
#[cfg(target_arch = "aarch64")]
unsafe fn fade_batch_neon(items: &[(f32, f32)]) -> Vec<f32> {
    use core::arch::aarch64::*;

    let len = items.len();
    let mut out = vec![0.0f32; len];
    let mut i = 0usize;
    let zero = vdupq_n_f32(0.0);
    let one = vdupq_n_f32(1.0);
    let eps = vdupq_n_f32(1e-6);

    while i + 4 <= len {
        let mut ds = [0.0f32; 4];
        let mut ts = [0.0f32; 4];
        for j in 0..4 {
            let (d, t) = items[i + j];
            ds[j] = d;
            ts[j] = t;
        }
        let t = vmaxnmq_f32(vld1q_f32(ts.as_ptr()), eps); // thickness.max(1e-6)
        let d = vabsq_f32(vld1q_f32(ds.as_ptr())); // distance.abs()
        let cut = vcgeq_f32(d, t); // d >= t → 0.0

        // (1.0 - d / t).clamp(0.0, 1.0)
        let raw = vsubq_f32(one, vdivq_f32(d, t));
        let clamped = vbslq_f32(
            vcltq_f32(raw, zero),
            zero,
            vbslq_f32(vcgtq_f32(raw, one), one, raw),
        );
        vst1q_f32(out.as_mut_ptr().add(i), vbslq_f32(cut, zero, clamped));
        i += 4;
    }
    while i < len {
        let (d, t) = items[i];
        out[i] = fade_one(d, t);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn parity() {
        let items = [(0.0f32, 0.5f32), (0.25, 0.5), (0.5, 0.5), (1.0, 0.5)];
        let b = fade_batch(&items);
        for (i, &it) in items.iter().enumerate() { assert_eq!(b[i].to_bits(), fade_one(it.0, it.1).to_bits()); }
    }
    #[test] fn empty_is_empty() { assert!(fade_batch(&[]).is_empty()); }
    #[test] fn single_eq_batch1() { assert_eq!(fade_batch(&[(0.2, 0.5)])[0].to_bits(), fade_one(0.2, 0.5).to_bits()); }
    #[test] fn on_surface_full() { assert!((fade_batch(&[(0.0, 0.5)])[0] - 1.0).abs() < 1e-6); }
    #[test] fn beyond_zero() { assert_eq!(fade_batch(&[(1.0, 0.5)])[0], 0.0); }
    #[test] fn deterministic() { assert_eq!(fade_batch(&[(0.2, 0.5)]), fade_batch(&[(0.2, 0.5)])); }

    #[test]
    fn simd_parity_random_lengths() {
        let mut s: u32 = 0xC0FFEE05;
        let mut rng = || { s ^= s << 13; s ^= s >> 17; s ^= s << 5; (s >> 8) as f32 / (1u32 << 24) as f32 };
        for n in 0..67usize {
            let items: Vec<(f32, f32)> = (0..n).map(|_| (rng() * 2.0 - 1.0, rng() * 0.8 - 0.1)).collect(); // thickness includes <=0
            let b = fade_batch(&items);
            for (i, &(d, t)) in items.iter().enumerate() {
                assert_eq!(b[i].to_bits(), fade_one(d, t).to_bits(), "n={n} i={i}");
            }
        }
    }
}
