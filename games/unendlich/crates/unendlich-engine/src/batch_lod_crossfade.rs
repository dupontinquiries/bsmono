//! batch_lod_crossfade.rs — pure batched LOD crossfade (microopt of R18).
//!
//! Scalar `fade_one` (mirroring R18 `lod_crossfade::fade`: smoothstep over
//! `[band_lo, band_hi]`) plus a batched `fade_batch` that is **bit-for-bit
//! identical** to the scalar path applied per distance. Pure, deterministic,
//! panic-free (R1/R11/R21).
//!
//! Perf note: crossfading many instances against the same band per frame, the
//! batch hoists only the output allocation (`with_capacity` + a tight loop),
//! running the identical per-distance smoothstep, so the weights match the
//! scalar reference bit-for-bit.

/// Scalar reference: smoothstep crossfade weight (0 below `band_lo`, 1 above
/// `band_hi`), with `band_hi` guarded to exceed `band_lo`.
pub fn fade_one(dist: f32, band_lo: f32, band_hi: f32) -> f32 {
    let lo = band_lo;
    let hi = band_hi.max(lo + 1e-4);
    if dist <= lo {
        0.0
    } else if dist >= hi {
        1.0
    } else {
        let t = (dist - lo) / (hi - lo);
        (t * t * (3.0 - 2.0 * t)).max(0.0).min(1.0)
    }
}

/// Batched form over `dists` sharing one band, routed through `fade_one`
/// (bit-parity).
pub fn fade_batch(dists: &[f32], band_lo: f32, band_hi: f32) -> Vec<f32> {
    let mut out = Vec::with_capacity(dists.len());
    for &d in dists {
        out.push(fade_one(d, band_lo, band_hi));
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
        let mut seed = 0x2157_0001u32;
        let dists: Vec<f32> = (0..1000).map(|_| lcg(&mut seed) * 60.0 - 5.0).collect();
        let (lo, hi) = (10.0f32, 40.0f32);
        let batched = fade_batch(&dists, lo, hi);
        assert_eq!(batched.len(), dists.len());
        for (i, &d) in dists.iter().enumerate() {
            assert_eq!(batched[i].to_bits(), fade_one(d, lo, hi).to_bits(), "mismatch at {}", i);
        }
    }

    #[test]
    fn empty_is_empty() {
        assert!(fade_batch(&[], 10.0, 40.0).is_empty());
    }

    #[test]
    fn single_equals_batch_of_one() {
        assert_eq!(fade_batch(&[25.0], 10.0, 40.0)[0].to_bits(), fade_one(25.0, 10.0, 40.0).to_bits());
    }

    #[test]
    fn bounded_unit_range() {
        let mut seed = 0x2157_0002u32;
        let dists: Vec<f32> = (0..1000).map(|_| lcg(&mut seed) * 100.0 - 20.0).collect();
        for f in fade_batch(&dists, 5.0, 30.0) {
            assert!((0.0..=1.0).contains(&f), "fade oob {}", f);
        }
    }

    #[test]
    fn deterministic() {
        let mut seed = 0x2157_0003u32;
        let dists: Vec<f32> = (0..64).map(|_| lcg(&mut seed) * 50.0).collect();
        assert_eq!(fade_batch(&dists, 10.0, 40.0), fade_batch(&dists, 10.0, 40.0));
    }

    #[test]
    fn endpoints_parity() {
        for &d in &[0.0f32, 10.0, 25.0, 40.0, 100.0] {
            assert_eq!(fade_batch(&[d], 10.0, 40.0)[0].to_bits(), fade_one(d, 10.0, 40.0).to_bits());
        }
    }
}
