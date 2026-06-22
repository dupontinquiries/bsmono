//! batch_fog_blend.rs — pure batched exp2 fog (microopt of R18 fog_blend).
//!
//! Scalar `fog_one` (mirroring R18 `fog_blend::fog`: `1 - 2^(-(density·dist)²)`)
//! plus a batched `fog_batch` that is **bit-for-bit identical** to the scalar
//! path applied per `(dist, density)` item. Pure, deterministic, panic-free
//! (R1/R11/R21).
//!
//! Perf note: applying fog across many samples/fragments per frame, the batch
//! hoists only the output allocation (`with_capacity` + a tight loop), running
//! the identical per-item `exp2`, so the factors match the scalar reference
//! bit-for-bit.

/// Scalar reference: GL_EXP2 fog amount in `[0,1]`.
pub fn fog_one(dist: f32, density: f32) -> f32 {
    let z = density.max(0.0) * dist.max(0.0);
    (1.0 - (-(z * z)).exp2()).max(0.0).min(1.0)
}

/// Batched form over `(dist, density)` items, routed through `fog_one`
/// (bit-parity).
pub fn fog_batch(items: &[(f32, f32)]) -> Vec<f32> {
    let mut out = Vec::with_capacity(items.len());
    for &(dist, density) in items {
        out.push(fog_one(dist, density));
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
        let mut seed = 0x2057_0001u32;
        let items: Vec<(f32, f32)> = (0..1000).map(|_| (lcg(&mut seed) * 1000.0, lcg(&mut seed) * 0.2)).collect();
        let batched = fog_batch(&items);
        assert_eq!(batched.len(), items.len());
        for (i, &(d, den)) in items.iter().enumerate() {
            assert_eq!(batched[i].to_bits(), fog_one(d, den).to_bits(), "mismatch at {}", i);
        }
    }

    #[test]
    fn empty_is_empty() {
        assert!(fog_batch(&[]).is_empty());
    }

    #[test]
    fn single_equals_batch_of_one() {
        assert_eq!(fog_batch(&[(50.0, 0.05)])[0].to_bits(), fog_one(50.0, 0.05).to_bits());
    }

    #[test]
    fn bounded_unit_range() {
        let mut seed = 0x2057_0002u32;
        let items: Vec<(f32, f32)> = (0..1000).map(|_| (lcg(&mut seed) * 2000.0 - 100.0, lcg(&mut seed) * 0.5 - 0.1)).collect();
        for f in fog_batch(&items) {
            assert!((0.0..=1.0).contains(&f), "fog oob {}", f);
        }
    }

    #[test]
    fn deterministic() {
        let mut seed = 0x2057_0003u32;
        let items: Vec<(f32, f32)> = (0..64).map(|_| (lcg(&mut seed) * 500.0, lcg(&mut seed) * 0.1)).collect();
        assert_eq!(fog_batch(&items), fog_batch(&items));
    }

    #[test]
    fn endpoints_parity() {
        for &(d, den) in &[(0.0f32, 0.1f32), (1e6, 0.05)] {
            assert_eq!(fog_batch(&[(d, den)])[0].to_bits(), fog_one(d, den).to_bits());
        }
    }
}
