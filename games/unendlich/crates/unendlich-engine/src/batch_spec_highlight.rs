//! batch_spec_highlight.rs — pure batched Blinn-Phong specular (microopt of R17).
//!
//! Scalar `spec_one` (mirroring R17 `spec_highlight::spec`) plus a batched
//! `spec_batch` that is **bit-for-bit identical** to the scalar path applied per
//! `(n_dot_h, power)` item. Pure, deterministic, panic-free (R1/R11/R21).
//!
//! Perf note: shading many lights/fragments per frame, the batch hoists only the
//! output allocation (`with_capacity` + a tight loop), running the identical
//! per-item `clamp.powf`, so the highlights match the scalar reference
//! bit-for-bit.

/// Scalar reference: `clamp(n_dot_h, 0, 1)^power`, in `[0,1]`.
pub fn spec_one(n_dot_h: f32, power: f32) -> f32 {
    let ndh = n_dot_h.max(0.0).min(1.0);
    let p = power.max(0.0);
    ndh.powf(p).max(0.0).min(1.0)
}

/// Batched form over `(n_dot_h, power)` items, routed through `spec_one`
/// (bit-parity).
pub fn spec_batch(items: &[(f32, f32)]) -> Vec<f32> {
    let mut out = Vec::with_capacity(items.len());
    for &(ndh, p) in items {
        out.push(spec_one(ndh, p));
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
        let mut seed = 0x1957_0001u32;
        let items: Vec<(f32, f32)> = (0..1000)
            .map(|_| (lcg(&mut seed) * 2.0 - 0.5, lcg(&mut seed) * 128.0))
            .collect();
        let batched = spec_batch(&items);
        assert_eq!(batched.len(), items.len());
        for (i, &(ndh, p)) in items.iter().enumerate() {
            assert_eq!(batched[i].to_bits(), spec_one(ndh, p).to_bits(), "mismatch at {}", i);
        }
    }

    #[test]
    fn empty_is_empty() {
        assert!(spec_batch(&[]).is_empty());
    }

    #[test]
    fn single_equals_batch_of_one() {
        assert_eq!(spec_batch(&[(0.8, 32.0)])[0].to_bits(), spec_one(0.8, 32.0).to_bits());
    }

    #[test]
    fn bounded_unit_range() {
        let mut seed = 0x1957_0002u32;
        let items: Vec<(f32, f32)> = (0..1000)
            .map(|_| (lcg(&mut seed) * 2.0 - 0.5, lcg(&mut seed) * 256.0))
            .collect();
        for s in spec_batch(&items) {
            assert!((0.0..=1.0).contains(&s), "spec oob {}", s);
        }
    }

    #[test]
    fn deterministic() {
        let mut seed = 0x1957_0003u32;
        let items: Vec<(f32, f32)> = (0..64).map(|_| (lcg(&mut seed), lcg(&mut seed) * 64.0)).collect();
        assert_eq!(spec_batch(&items), spec_batch(&items));
    }

    #[test]
    fn negative_alignment_parity() {
        // back-facing n·h clamps to 0 in both paths
        assert_eq!(spec_batch(&[(-0.4, 16.0)])[0].to_bits(), spec_one(-0.4, 16.0).to_bits());
    }
}
