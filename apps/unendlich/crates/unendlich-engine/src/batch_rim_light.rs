//! batch_rim_light.rs — pure batched Fresnel rim (microopt of R19 rim_light).
//!
//! Scalar `rim_one` (mirroring R19 `rim_light::rim`: `(1 - clamp(n·v,0,1))^power`)
//! plus a batched `rim_batch` that is **bit-for-bit identical** to the scalar
//! path applied per `(n_dot_v, power)` item. Pure, deterministic, panic-free
//! (R1/R11/R21).
//!
//! Perf note: shading rim light over many fragments per frame, the batch hoists
//! only the output allocation (`with_capacity` + a tight loop), running the
//! identical per-item `clamp.powf`, so the rim values match the scalar reference
//! bit-for-bit.

/// Scalar reference: `(1 - clamp(n_dot_v, 0, 1))^power`, in `[0,1]`.
pub fn rim_one(n_dot_v: f32, power: f32) -> f32 {
    let ndv = n_dot_v.max(0.0).min(1.0);
    let p = power.max(0.0);
    (1.0 - ndv).powf(p).max(0.0).min(1.0)
}

/// Batched form over `(n_dot_v, power)` items, routed through `rim_one`
/// (bit-parity).
pub fn rim_batch(items: &[(f32, f32)]) -> Vec<f32> {
    let mut out = Vec::with_capacity(items.len());
    for &(ndv, p) in items {
        out.push(rim_one(ndv, p));
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
        let mut seed = 0x2257_0001u32;
        let items: Vec<(f32, f32)> = (0..1000).map(|_| (lcg(&mut seed) * 2.0 - 0.5, lcg(&mut seed) * 16.0)).collect();
        let batched = rim_batch(&items);
        assert_eq!(batched.len(), items.len());
        for (i, &(ndv, p)) in items.iter().enumerate() {
            assert_eq!(batched[i].to_bits(), rim_one(ndv, p).to_bits(), "mismatch at {}", i);
        }
    }

    #[test]
    fn empty_is_empty() {
        assert!(rim_batch(&[]).is_empty());
    }

    #[test]
    fn single_equals_batch_of_one() {
        assert_eq!(rim_batch(&[(0.3, 5.0)])[0].to_bits(), rim_one(0.3, 5.0).to_bits());
    }

    #[test]
    fn bounded_unit_range() {
        let mut seed = 0x2257_0002u32;
        let items: Vec<(f32, f32)> = (0..1000).map(|_| (lcg(&mut seed) * 2.0 - 0.5, lcg(&mut seed) * 32.0)).collect();
        for r in rim_batch(&items) {
            assert!((0.0..=1.0).contains(&r), "rim oob {}", r);
        }
    }

    #[test]
    fn deterministic() {
        let mut seed = 0x2257_0003u32;
        let items: Vec<(f32, f32)> = (0..64).map(|_| (lcg(&mut seed), lcg(&mut seed) * 8.0)).collect();
        assert_eq!(rim_batch(&items), rim_batch(&items));
    }

    #[test]
    fn endpoints_parity() {
        for &(ndv, p) in &[(0.0f32, 3.0f32), (1.0, 3.0), (-0.5, 3.0)] {
            assert_eq!(rim_batch(&[(ndv, p)])[0].to_bits(), rim_one(ndv, p).to_bits());
        }
    }
}
