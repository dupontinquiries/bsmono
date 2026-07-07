//! batch_depth_fade.rs — pure batched soft-particle fade (microopt of R23).
//!
//! Scalar `soft_one` (mirroring R23 `depth_fade::soft`) plus a batched
//! `soft_batch` that is **bit-for-bit identical** to the scalar path applied per
//! `(particle_z, scene_z)` item. Pure, deterministic, panic-free (R1/R11/R21).
//!
//! Perf note: soft-fading many particles against the depth buffer per frame, the
//! batch hoists only the output allocation (`with_capacity` + a tight loop),
//! running the identical per-item ramp with a shared `fade`, so the opacities
//! match the scalar reference bit-for-bit.

/// Scalar reference: soft-particle opacity in `[0,1]` (0 touching, 1 far in
/// front), larger depth = farther.
pub fn soft_one(particle_z: f32, scene_z: f32, fade: f32) -> f32 {
    let f = fade.max(1e-4);
    let gap = scene_z - particle_z;
    (gap / f).max(0.0).min(1.0)
}

/// Batched form over `(particle_z, scene_z)` items sharing one `fade`, routed
/// through `soft_one` (bit-parity).
pub fn soft_batch(items: &[(f32, f32)], fade: f32) -> Vec<f32> {
    let mut out = Vec::with_capacity(items.len());
    for &(pz, sz) in items {
        out.push(soft_one(pz, sz, fade));
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
        let mut seed = 0x2457_0001u32;
        let items: Vec<(f32, f32)> = (0..1000).map(|_| (lcg(&mut seed) * 100.0, lcg(&mut seed) * 100.0)).collect();
        let fade = 3.0f32;
        let batched = soft_batch(&items, fade);
        assert_eq!(batched.len(), items.len());
        for (i, &(pz, sz)) in items.iter().enumerate() {
            assert_eq!(batched[i].to_bits(), soft_one(pz, sz, fade).to_bits(), "mismatch at {}", i);
        }
    }

    #[test]
    fn empty_is_empty() {
        assert!(soft_batch(&[], 2.0).is_empty());
    }

    #[test]
    fn single_equals_batch_of_one() {
        assert_eq!(soft_batch(&[(1.0, 3.0)], 2.0)[0].to_bits(), soft_one(1.0, 3.0, 2.0).to_bits());
    }

    #[test]
    fn bounded_unit_range() {
        let mut seed = 0x2457_0002u32;
        let items: Vec<(f32, f32)> = (0..1000).map(|_| (lcg(&mut seed) * 100.0, lcg(&mut seed) * 100.0)).collect();
        for s in soft_batch(&items, 2.5) {
            assert!((0.0..=1.0).contains(&s), "soft oob {}", s);
        }
    }

    #[test]
    fn deterministic() {
        let mut seed = 0x2457_0003u32;
        let items: Vec<(f32, f32)> = (0..64).map(|_| (lcg(&mut seed) * 50.0, lcg(&mut seed) * 50.0)).collect();
        assert_eq!(soft_batch(&items, 2.0), soft_batch(&items, 2.0));
    }

    #[test]
    fn endpoints_parity() {
        for &(pz, sz) in &[(10.0f32, 10.0f32), (0.0, 100.0)] {
            assert_eq!(soft_batch(&[(pz, sz)], 2.0)[0].to_bits(), soft_one(pz, sz, 2.0).to_bits());
        }
    }
}
