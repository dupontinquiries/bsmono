//! `sim::indirect_batch` — coalesce draws into multi-draw-indirect runs.
//!
//! Real M2 data (RD=8) shows the main pass draw-bound (~1357 chunks, p50 23 ms,
//! 56% over budget). Draws that share a pipeline+material can be issued as one
//! multi-draw-indirect command, cutting bind/dispatch count and lifting
//! throughput (UNENDLICH_INDIRECT=1). This collapses each maximal contiguous
//! run of equal `(pipeline, material)` into a single [`Batch`] indexing the
//! ordered draw array.
//!
//! Pairs with `draw_order_sort` (R34), which already groups equal
//! `(pipeline, material)` contiguously — this module assumes that ordering and
//! does the run-length coalescing. Distinct from `draw_metadata_pack`
//! (per-chunk byte packing).
//!
//! Pure and deterministic: a single pass, no allocation beyond the caller's
//! `out` vector. No GPU, IO, clock, or RNG; no `.unwrap()`/`.expect()`.

/// One draw's batching-relevant fields. Only `(pipeline, material)` decide
/// batching; the rest ride along for the eventual indirect command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DrawRef {
    pub pipeline: u16,
    pub material: u16,
    pub slot: u32,
    pub index_count: u32,
    pub first_index: u32,
    pub base_vertex: i32,
}

/// A multi-draw run covering `ordered[first .. first + count]`, all sharing
/// `(pipeline, material)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Batch {
    pub pipeline: u16,
    pub material: u16,
    pub first: u32,
    pub count: u32,
}

#[inline]
fn key(d: &DrawRef) -> (u16, u16) {
    (d.pipeline, d.material)
}

/// Coalesce each maximal contiguous run of equal `(pipeline, material)` in
/// `ordered` into one [`Batch`]. `out` is cleared first, then filled so the
/// batches tile `[0, ordered.len())` exactly — contiguous `first`s, no gaps,
/// no overlap. Assumes `ordered` is already grouped by key (e.g. the output of
/// `draw_order_sort`); a key that recurs non-contiguously simply yields
/// separate batches.
pub fn batch(ordered: &[DrawRef], out: &mut Vec<Batch>) {
    out.clear();
    if ordered.is_empty() {
        return;
    }

    let mut run_key = key(&ordered[0]);
    let mut first: usize = 0;

    for i in 1..ordered.len() {
        let k = key(&ordered[i]);
        if k != run_key {
            out.push(Batch {
                pipeline: run_key.0,
                material: run_key.1,
                first: first as u32,
                count: (i - first) as u32,
            });
            run_key = k;
            first = i;
        }
    }
    // flush the final run
    out.push(Batch {
        pipeline: run_key.0,
        material: run_key.1,
        first: first as u32,
        count: (ordered.len() - first) as u32,
    });
}

/// How many batches [`batch`] would produce for `ordered`, without allocating —
/// the number of maximal contiguous `(pipeline, material)` runs.
pub fn batch_count(ordered: &[DrawRef]) -> usize {
    if ordered.is_empty() {
        return 0;
    }
    let mut runs = 1usize;
    for i in 1..ordered.len() {
        if key(&ordered[i]) != key(&ordered[i - 1]) {
            runs += 1;
        }
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(pipeline: u16, material: u16, slot: u32) -> DrawRef {
        DrawRef { pipeline, material, slot, index_count: slot * 3, first_index: slot, base_vertex: slot as i32 }
    }

    /// Structural invariants of a batching, returns the batches.
    fn check(ordered: &[DrawRef]) -> Vec<Batch> {
        let mut out = vec![Batch { pipeline: 9, material: 9, first: 999, count: 999 }]; // junk
        batch(ordered, &mut out);
        assert_eq!(out.len(), batch_count(ordered), "batch_count disagrees with out.len()");

        if ordered.is_empty() {
            assert!(out.is_empty(), "out not cleared on empty input");
            return out;
        }
        // tiling: contiguous firsts, no gaps/overlap, full coverage
        let mut expected_first = 0u32;
        for (bi, b) in out.iter().enumerate() {
            assert_eq!(b.first, expected_first, "batch {} not contiguous", bi);
            assert!(b.count >= 1, "batch {} empty", bi);
            // every draw in the run carries this batch's key
            for j in b.first..(b.first + b.count) {
                assert_eq!(key(&ordered[j as usize]), (b.pipeline, b.material),
                           "draw {} key != batch {} key", j, bi);
            }
            // maximality: adjacent batches must differ in key
            if bi > 0 {
                let prev = out[bi - 1];
                assert!((prev.pipeline, prev.material) != (b.pipeline, b.material),
                        "adjacent batches share key (not maximal)");
            }
            expected_first += b.count;
        }
        assert_eq!(expected_first as usize, ordered.len(), "batches don't tile [0,len)");
        let sum: usize = out.iter().map(|b| b.count as usize).sum();
        assert_eq!(sum, ordered.len(), "sum of counts != len");
        out
    }

    #[test]
    fn empty_is_empty() {
        let out = check(&[]);
        assert!(out.is_empty());
        assert_eq!(batch_count(&[]), 0);
    }

    #[test]
    fn single_draw_one_batch() {
        let out = check(&[d(3, 7, 0)]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0], Batch { pipeline: 3, material: 7, first: 0, count: 1 });
    }

    #[test]
    fn one_run_one_batch() {
        let ord = [d(1, 2, 0), d(1, 2, 5), d(1, 2, 9), d(1, 2, 12)];
        let out = check(&ord);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0], Batch { pipeline: 1, material: 2, first: 0, count: 4 });
    }

    #[test]
    fn key_change_starts_new_batch() {
        // runs: (1,1)x2, (1,2)x1, (2,2)x3
        let ord = [
            d(1, 1, 0), d(1, 1, 1),
            d(1, 2, 2),
            d(2, 2, 3), d(2, 2, 4), d(2, 2, 5),
        ];
        let out = check(&ord);
        assert_eq!(out, vec![
            Batch { pipeline: 1, material: 1, first: 0, count: 2 },
            Batch { pipeline: 1, material: 2, first: 2, count: 1 },
            Batch { pipeline: 2, material: 2, first: 3, count: 3 },
        ]);
    }

    #[test]
    fn pipeline_or_material_change_both_split() {
        // same pipeline, material differs -> split; then material same, pipeline differs -> split
        let ord = [d(1, 1, 0), d(1, 2, 1), d(2, 2, 2)];
        assert_eq!(batch_count(&ord), 3);
        check(&ord);
    }

    #[test]
    fn recurring_key_noncontiguous_is_separate_batches() {
        // A,A,B,A — the trailing A is a *new* run (we only coalesce contiguous).
        let ord = [d(1, 1, 0), d(1, 1, 1), d(2, 2, 2), d(1, 1, 3)];
        let out = check(&ord);
        assert_eq!(out.len(), 3);
        assert_eq!(out[2], Batch { pipeline: 1, material: 1, first: 3, count: 1 });
    }

    #[test]
    fn out_is_cleared_first() {
        let mut out = vec![Batch { pipeline: 0, material: 0, first: 42, count: 42 }; 5];
        batch(&[d(4, 4, 0)], &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].first, 0);
    }

    #[test]
    fn seeded_sweep_tiles_and_is_maximal() {
        // deterministic seeded sweep (R20): build arrays as concatenated runs of
        // random keys (guaranteeing the "already grouped" precondition), then
        // verify tiling, maximality, and batch_count agreement.
        let mut x: u64 = 0x1D1_8A7C_u64; // documented seed
        let mut next = || { x = x.wrapping_mul(6364136223846793005).wrapping_add(1); (x >> 33) as u32 };
        for _ in 0..2_000 {
            let n_runs = (next() % 12) as usize;
            let mut ord: Vec<DrawRef> = Vec::new();
            let mut last_key: Option<(u16, u16)> = None;
            let mut slot = 0u32;
            for _ in 0..n_runs {
                // pick a key distinct from the previous run so runs are maximal
                let mut k = ((next() % 3) as u16, (next() % 3) as u16);
                if Some(k) == last_key { k.0 = k.0.wrapping_add(1); }
                last_key = Some(k);
                let run_len = 1 + (next() % 5) as usize;
                for _ in 0..run_len {
                    ord.push(d(k.0, k.1, slot));
                    slot += 1;
                }
            }
            let out = check(&ord);
            // because we made runs maximal, batch count == number of runs
            assert_eq!(out.len(), n_runs);
        }
    }

    #[test]
    fn deterministic() {
        let ord = [d(1, 1, 0), d(1, 2, 1), d(1, 2, 2), d(3, 0, 3)];
        let mut a = Vec::new();
        let mut b = Vec::new();
        batch(&ord, &mut a);
        batch(&ord, &mut b);
        assert_eq!(a, b);
        assert_eq!(batch_count(&ord), a.len());
    }
}
