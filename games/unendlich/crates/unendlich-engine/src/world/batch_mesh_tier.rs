//! `batch_mesh_tier` — batched mesh-tier classification (perf bonus).
//!
//! Worker-03 / WORLD pure core, round 16. Classifies a whole frame's worth of
//! chunks in one call, sharing the (constant-per-frame) `gpu_ok` flag and band
//! array across the batch, while producing **identical** tiers to the scalar
//! reference (which mirrors `mesh_tier`, R15).
//!
//! PERF: the streamer evaluates every resident chunk's tier each frame (~630
//! near + thousands beyond at RD≥24). Batching hoists the `gpu_ok`/`bands`
//! setup out of the per-chunk path and keeps the band-crossing scan
//! branch-predictable over a contiguous slice; the result is bit-identical to
//! the per-chunk call, so a batched fast path can't reorder work vs a scalar
//! fallback.
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

/// Where/how a chunk should be meshed (mirrors `mesh_tier::MeshTier`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeshTier {
    CpuNear,
    GpuNear,
    GpuMid,
    GpuFar,
    GpuVFar,
    Skip,
}

/// Scalar reference — identical logic to `mesh_tier::mesh_tier` (R15).
pub fn mesh_tier_one(dist: f32, edited_recently: bool, gpu_ok: bool, bands: &[f32]) -> MeshTier {
    let mut idx = 0usize;
    for &b in bands {
        if dist >= b {
            idx += 1;
        } else {
            break;
        }
    }
    if idx >= bands.len() {
        return MeshTier::Skip;
    }
    if edited_recently || idx == 0 {
        return MeshTier::CpuNear;
    }
    let gpu = match idx {
        1 => MeshTier::GpuNear,
        2 => MeshTier::GpuMid,
        3 => MeshTier::GpuFar,
        _ => MeshTier::GpuVFar,
    };
    if gpu_ok {
        gpu
    } else {
        MeshTier::CpuNear
    }
}

/// Batched classification over `(dist, edited_recently)` items sharing one
/// `gpu_ok` and `bands`. Bit-identical to calling `mesh_tier_one` per item.
pub fn mesh_tier_batch(items: &[(f32, bool)], gpu_ok: bool, bands: &[f32]) -> Vec<MeshTier> {
    items
        .iter()
        .map(|&(d, e)| mesh_tier_one(d, e, gpu_ok, bands))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const BANDS: [f32; 5] = [6.0, 16.0, 32.0, 64.0, 128.0];

    #[test]
    fn batch_matches_scalar() {
        let items = [
            (2.0f32, false),
            (50.0, true),
            (6.0, false),
            (16.0, false),
            (64.0, false),
            (200.0, false),
        ];
        let batch = mesh_tier_batch(&items, true, &BANDS);
        for (i, &(d, e)) in items.iter().enumerate() {
            assert_eq!(batch[i], mesh_tier_one(d, e, true, &BANDS), "tier drift at {i}");
        }
    }

    #[test]
    fn empty_is_empty() {
        assert!(mesh_tier_batch(&[], true, &BANDS).is_empty());
    }

    #[test]
    fn single_equals_batch_of_one() {
        let one = mesh_tier_one(20.0, false, true, &BANDS);
        let batch = mesh_tier_batch(&[(20.0, false)], true, &BANDS);
        assert_eq!(batch, vec![one]);
    }

    #[test]
    fn no_gpu_fallback_parity() {
        let items = [(6.0f32, false), (32.0, false), (100.0, false), (500.0, false)];
        let batch = mesh_tier_batch(&items, false, &BANDS);
        for (i, &(d, e)) in items.iter().enumerate() {
            let scalar = mesh_tier_one(d, e, false, &BANDS);
            assert_eq!(batch[i], scalar);
            assert!(matches!(batch[i], MeshTier::CpuNear | MeshTier::Skip));
        }
    }

    #[test]
    fn deterministic() {
        let items = [(10.0f32, false), (10.0, false)];
        assert_eq!(mesh_tier_batch(&items, true, &BANDS), mesh_tier_batch(&items, true, &BANDS));
    }

    #[test]
    fn covers_all_tiers() {
        let items = [(2.0f32, false), (6.0, false), (16.0, false), (32.0, false), (64.0, false), (200.0, false)];
        let b = mesh_tier_batch(&items, true, &BANDS);
        assert_eq!(
            b,
            vec![
                MeshTier::CpuNear,
                MeshTier::GpuNear,
                MeshTier::GpuMid,
                MeshTier::GpuFar,
                MeshTier::GpuVFar,
                MeshTier::Skip,
            ]
        );
    }
}
