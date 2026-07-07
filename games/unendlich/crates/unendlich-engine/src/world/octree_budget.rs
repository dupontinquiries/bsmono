//! LOD-aware octree budget — the guard against the octree-pool exhaustion wall
//! (the RD192 run: pool 256MB full, `largest_free=0`, uploads failing, because
//! every chunk — near or very-far — held a full ~183KB octree). Two fixes, both
//! pure policy the GPU pool consumes:
//!
//! 1. [`target_octree_bytes`] — distant chunks don't need full detail. A coarser
//!    LOD should store a downsampled octree (far/vfar shrink sharply), so the
//!    same pool holds many more chunks.
//! 2. [`plan_evictions`] — when residents would still overflow, shed the
//!    least-valuable first: coarsest LOD, then farthest, until it fits. Keeps the
//!    near, in-front chunks the player actually sees.
//!
//! Pure, deterministic.

use crate::world::lod::LodLevel;

/// Estimated octree bytes a chunk *should* occupy at `lod`, downsampled from its
/// full-detail size. Near keeps full detail; each coarser level quarters it
/// (a 2× spatial downsample drops surface-node count ~4×). So Near=full,
/// Mid=full/4, Far=full/16, VeryFar=full/64 — the far field nearly free.
pub fn target_octree_bytes(full_bytes: u32, lod: LodLevel) -> u32 {
    full_bytes >> (2 * lod.index() as u32)
}

/// A chunk currently resident in the octree pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Resident {
    pub chunk: [i32; 3],
    pub lod: LodLevel,
    pub bytes: u32,
    /// Squared distance to the camera — nearer is more valuable.
    pub dist2: i64,
}

/// Total bytes the residents occupy.
pub fn total_bytes(residents: &[Resident]) -> u64 {
    residents.iter().map(|r| r.bytes as u64).sum()
}

/// Chunks to evict (in eviction order) so the rest fit `capacity_bytes`. The
/// least valuable go first: coarsest LOD, then farthest. Empty if it already
/// fits. Deterministic — ties broken by chunk coord.
pub fn plan_evictions(residents: &[Resident], capacity_bytes: u64) -> Vec<[i32; 3]> {
    let mut total = total_bytes(residents);
    if total <= capacity_bytes {
        return Vec::new();
    }
    let mut order: Vec<&Resident> = residents.iter().collect();
    // Most-evictable first: coarsest LOD, then farthest, then coord for determinism.
    order.sort_by(|a, b| {
        b.lod
            .index()
            .cmp(&a.lod.index())
            .then(b.dist2.cmp(&a.dist2))
            .then(a.chunk.cmp(&b.chunk))
    });
    let mut evict = Vec::new();
    for r in order {
        if total <= capacity_bytes {
            break;
        }
        evict.push(r.chunk);
        total -= r.bytes as u64;
    }
    evict
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::lod::LodLevel::*;

    #[test]
    fn coarser_lods_store_far_smaller_octrees() {
        let full = 183 * 1024;
        assert_eq!(target_octree_bytes(full, Near), full);
        assert_eq!(target_octree_bytes(full, Mid), full / 4);
        assert_eq!(target_octree_bytes(full, Far), full / 16);
        assert_eq!(target_octree_bytes(full, VeryFar), full / 64);
    }

    #[test]
    fn nothing_evicted_when_it_fits() {
        let r = [
            Resident { chunk: [0, 0, 0], lod: Near, bytes: 1000, dist2: 4 },
            Resident { chunk: [1, 0, 0], lod: Far, bytes: 500, dist2: 900 },
        ];
        assert!(plan_evictions(&r, 10_000).is_empty());
    }

    #[test]
    fn evicts_coarsest_and_farthest_first() {
        let r = [
            Resident { chunk: [0, 0, 0], lod: Near, bytes: 1000, dist2: 4 },     // keep
            Resident { chunk: [9, 0, 0], lod: VeryFar, bytes: 1000, dist2: 9999 }, // evict first
            Resident { chunk: [5, 0, 0], lod: Far, bytes: 1000, dist2: 2500 },    // evict second
        ];
        // Capacity fits only the single near chunk (1000 of 3000 bytes).
        let evict = plan_evictions(&r, 1000);
        assert_eq!(evict, vec![[9, 0, 0], [5, 0, 0]], "shed the far/coarse, keep near");
        assert!(!evict.contains(&[0, 0, 0]), "the near chunk survives");
    }

    #[test]
    fn among_same_lod_the_farther_goes_first() {
        let r = [
            Resident { chunk: [1, 0, 0], lod: Far, bytes: 1000, dist2: 100 },
            Resident { chunk: [2, 0, 0], lod: Far, bytes: 1000, dist2: 9000 },
        ];
        let evict = plan_evictions(&r, 1000);
        assert_eq!(evict, vec![[2, 0, 0]], "the farther Far chunk is evicted");
    }

    #[test]
    fn lod_sizing_keeps_the_pool_within_budget_where_full_sizing_blows_it() {
        // RD192-shaped: 100 near + 2000 far/vfar chunks, 256MB pool.
        let full = 183 * 1024u32;
        let cap = 256 * 1024 * 1024u64;
        // Full-detail everywhere: 2100 × 183KB ≈ 375MB — overflows.
        let full_total = 2100u64 * full as u64;
        assert!(full_total > cap, "full-detail far field overflows the pool");
        // LOD-sized: near full, the 2000 distant at VeryFar (full/64).
        let sized: u64 = 100 * full as u64 + 2000 * target_octree_bytes(full, VeryFar) as u64;
        assert!(sized < cap, "LOD-sized far field fits comfortably: {sized} < {cap}");
    }
}
