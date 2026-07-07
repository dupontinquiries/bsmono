//! `sim::lod_budget` — budget-aware LOD assignment (triangle governor).
//!
//! Real M2 data (RD=8): the main pass is the wall — p50 23ms, p99 82ms, 56% of
//! frames over the 8.3ms budget. The lever is a *triangle budget*: when the
//! visible set is too heavy, coarsen the farthest / heaviest chunks one tier at
//! a time until the estimated triangle total fits. `lod.rs` owns the bands and
//! the downsample factors `[1,2,4,8]` (Near/Mid/Far/VeryFar); this is the
//! assignment core that consumes them. Pure, deterministic, no allocation beyond
//! `out` (R1/R6).
//!
//! Per-tier triangle assumption: `lod.rs` doubles the downsample factor each
//! tier, and a chunk's surface-triangle count scales with `1/factor²`, so each
//! coarser tier multiplies estimated triangles by `1/4`. In integer terms,
//! `tris(lod) = est_tris >> (2 · (lod − base_lod))` (saturating to 0). This is an
//! estimate for budgeting, not an exact mesh count.
//!
//! Mobile: this *is* the mobile knob — a constrained 8GB SoC simply passes a
//! lower `max_tris` and the same scene auto-coarsens to fit, with no per-device
//! code branches. Suggested `max_tris`: desktop ~6–8M, mobile ~1.5–2.5M. The
//! coarser far LOD also shrinks *resident mesh memory* (the RD=8 log shows
//! ~182KB/chunk), so a tighter triangle budget doubles as a VRAM/RAM budget on
//! memory-constrained devices.

/// One visible chunk's LOD input. `est_tris` is the estimate at `base_lod`
/// (coarser LODs have fewer).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct ChunkLodInput {
    /// Squared distance to the camera (farther = coarsened first).
    pub dist2: u32,
    /// The chunk's natural LOD from the distance bands; the floor for assignment.
    pub base_lod: u8,
    /// Estimated triangles at `base_lod`.
    pub est_tris: u32,
}

/// Device triangle budget.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Budget {
    /// Maximum total estimated triangles for the visible set.
    pub max_tris: u64,
    /// Coarsest LOD tier allowed (the assignment ceiling).
    pub max_lod: u8,
}

/// Estimated triangles for a chunk coarsened `steps` tiers past its base
/// (`1/4` per tier, saturating to 0). `steps` huge ⇒ 0 (no shift overflow).
fn tris_at(est_tris: u32, steps: u8) -> u64 {
    let shift = (steps as u32) * 2;
    if shift >= 64 {
        0
    } else {
        (est_tris as u64) >> shift
    }
}

/// Total estimated triangles for `chunks` at the per-chunk LODs in `lods`
/// (parallel to `chunks`; a missing entry is treated as that chunk's `base_lod`).
/// Each chunk's LOD is floored at its `base_lod`.
pub fn est_total_tris(chunks: &[ChunkLodInput], lods: &[u8]) -> u64 {
    let mut total: u64 = 0;
    for (i, c) in chunks.iter().enumerate() {
        let lod = match lods.get(i) {
            Some(&l) => l,
            None => c.base_lod,
        };
        let steps = lod.saturating_sub(c.base_lod);
        total = total.saturating_add(tris_at(c.est_tris, steps));
    }
    total
}

/// Assign each chunk a final LOD in `[base_lod, max(max_lod, base_lod)]` so the
/// total estimated triangles fit `budget.max_tris`, coarsening the farthest then
/// heaviest chunks one tier at a time. Writes final LODs into `out` (cleared
/// first; `out.len() == chunks.len()`). If the set fits at base, every chunk
/// keeps `base_lod`; if it can't fit even with everything at `max_lod`, it stops
/// there (best effort).
pub fn assign(chunks: &[ChunkLodInput], budget: Budget, out: &mut Vec<u8>) {
    out.clear();
    for c in chunks {
        out.push(c.base_lod); // start every chunk at its natural LOD
    }
    if chunks.is_empty() {
        return;
    }

    let mut total = est_total_tris(chunks, out);

    // Greedy: while over budget, coarsen the farthest (then heaviest, then
    // lowest-index) chunk that isn't yet at its ceiling, one tier at a time.
    // O(n) scan per coarsen, no auxiliary allocation.
    while total > budget.max_tris {
        let mut best: Option<usize> = None;
        let mut best_key: (u32, u32) = (0, 0); // (dist2, est_tris)
        for (i, c) in chunks.iter().enumerate() {
            let ceil = budget.max_lod.max(c.base_lod); // never below the floor
            if out[i] >= ceil {
                continue; // already as coarse as allowed
            }
            let key = (c.dist2, c.est_tris);
            let take = match best {
                None => true,
                // strictly greater on (dist2, est_tris) wins; equal ⇒ keep the
                // lower index already held (deterministic).
                Some(_) => key.0 > best_key.0 || (key.0 == best_key.0 && key.1 > best_key.1),
            };
            if take {
                best = Some(i);
                best_key = key;
            }
        }
        match best {
            None => break, // everything at its ceiling — best effort, stop
            Some(i) => {
                let base = chunks[i].base_lod;
                let before = tris_at(chunks[i].est_tris, out[i].saturating_sub(base));
                out[i] += 1;
                let after = tris_at(chunks[i].est_tris, out[i].saturating_sub(base));
                // after <= before; keep the running total in sync
                total = total.saturating_sub(before).saturating_add(after);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(dist2: u32, base_lod: u8, est_tris: u32) -> ChunkLodInput {
        ChunkLodInput { dist2, base_lod, est_tris }
    }

    #[test]
    fn already_under_budget_keeps_base() {
        let chunks = [c(10, 0, 1000), c(20, 1, 2000)];
        let mut out = Vec::new();
        assign(&chunks, Budget { max_tris: 1_000_000, max_lod: 3 }, &mut out);
        assert_eq!(out, vec![0, 1], "under budget ⇒ keep base_lod");
    }

    #[test]
    fn over_budget_coarsens_and_fits_or_maxes() {
        let chunks: Vec<ChunkLodInput> = (0..8).map(|i| c(i * 100, 0, 100_000)).collect();
        let budget = Budget { max_tris: 200_000, max_lod: 3 };
        let mut out = Vec::new();
        assign(&chunks, budget, &mut out);
        let total = est_total_tris(&chunks, &out);
        let all_maxed = out.iter().all(|&l| l == 3);
        assert!(total <= budget.max_tris || all_maxed, "must fit or be fully coarsened: {}", total);
    }

    #[test]
    fn coarsens_farthest_heaviest_first() {
        // near+light (idx 0) and far+heavy (idx 1); one coarsen of idx1 (100k→25k)
        // brings total 110k+... under budget while idx0 stays at base.
        let chunks = [c(10, 0, 10_000), c(9000, 0, 100_000)];
        // base total = 110_000; coarsen far one tier → 10_000 + 25_000 = 35_000
        let budget = Budget { max_tris: 40_000, max_lod: 3 };
        let mut out = Vec::new();
        assign(&chunks, budget, &mut out);
        assert_eq!(out[0], 0, "near/light chunk should stay at base");
        assert!(out[1] >= 1, "far/heavy chunk should be coarsened first");
        assert!(est_total_tris(&chunks, &out) <= budget.max_tris);
    }

    #[test]
    fn est_total_non_increasing_as_lods_coarsen() {
        let chunks = [c(1, 0, 64_000), c(2, 0, 64_000)];
        let base = vec![0u8, 0];
        let one = vec![1u8, 0];
        let two = vec![1u8, 1];
        let t0 = est_total_tris(&chunks, &base);
        let t1 = est_total_tris(&chunks, &one);
        let t2 = est_total_tris(&chunks, &two);
        assert!(t1 < t0 && t2 < t1, "coarsening must strictly reduce here: {} {} {}", t0, t1, t2);
        // 1/4 per tier: 64000 -> 16000
        assert_eq!(t1, 64_000 + 16_000);
    }

    #[test]
    fn out_cleared_len_and_bounds() {
        let mut out = vec![9, 9, 9, 9, 9]; // junk, wrong length
        let chunks: Vec<ChunkLodInput> = (0..6).map(|i| c(i * 50, (i % 2) as u8, 80_000)).collect();
        let budget = Budget { max_tris: 1, max_lod: 2 }; // force max coarsening
        assign(&chunks, budget, &mut out);
        assert_eq!(out.len(), chunks.len(), "len must match");
        for (i, &l) in out.iter().enumerate() {
            let ceil = budget.max_lod.max(chunks[i].base_lod);
            assert!(l >= chunks[i].base_lod && l <= ceil, "out[{}]={} out of [{},{}]", i, l, chunks[i].base_lod, ceil);
        }
    }

    #[test]
    fn empty_is_empty() {
        let mut out = vec![1, 2, 3];
        assign(&[], Budget { max_tris: 100, max_lod: 3 }, &mut out);
        assert!(out.is_empty());
        assert_eq!(est_total_tris(&[], &[]), 0);
    }

    #[test]
    fn deterministic() {
        let mut s = 0x38FA_u64; // documented seed (R20)
        let mut rng = || {
            s = s.wrapping_mul(6364136223846793005).wrapping_add(1);
            s
        };
        for _ in 0..1000 {
            let n = (rng() % 30) as u32;
            let chunks: Vec<ChunkLodInput> = (0..n)
                .map(|_| c((rng() % 100_000) as u32, (rng() % 3) as u8, (rng() % 200_000) as u32))
                .collect();
            let budget = Budget { max_tris: (rng() % 5_000_000), max_lod: (rng() % 4) as u8 };
            let mut a = Vec::new();
            let mut b = Vec::new();
            assign(&chunks, budget, &mut a);
            assign(&chunks, budget, &mut b);
            assert_eq!(a, b, "non-deterministic");
            assert_eq!(a.len(), chunks.len());
            for (i, &l) in a.iter().enumerate() {
                let ceil = budget.max_lod.max(chunks[i].base_lod);
                assert!(l >= chunks[i].base_lod && l <= ceil);
            }
        }
    }
}
