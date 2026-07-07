//! `sim::cull_dispatch_split` — bound the per-dispatch size of the GPU cull
//! pass to cap its worst-case latency.
//!
//! Real M2 data shows the cull pass is bimodal: p50 ≈ 0.04 ms but p99 ≈ 18.8 ms
//! (max ≈ 40 ms), because an occasional giant cull set spikes a single
//! dispatch. Partitioning the work into in-order contiguous sub-ranges, each no
//! larger than `max_per_dispatch`, turns one 40 ms monster into N bounded
//! dispatches and smooths the tail. Composes with `chunk_frustum_cull` (R33)
//! and `frustum_plane_extract` (R36).
//!
//! Pure and deterministic: integer arithmetic only; the only allocation is the
//! returned vectors. No GPU, IO, clock, or RNG; no `.unwrap()`/`.expect()`.

/// A partition of `[0, total)` into contiguous sub-ranges. `offsets[k]` is the
/// start of range `k` and `counts[k]` its length; the two vectors share length
/// (== the dispatch count). The ranges cover `[0, total)` exactly — no gaps,
/// no overlap — and each `counts[k] <= max_per_dispatch` (with `0` treated as
/// `1`).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Split {
    pub offsets: Vec<u32>,
    pub counts: Vec<u32>,
}

/// Effective per-dispatch cap: `max_per_dispatch` with `0` promoted to `1`
/// (a cap of zero would make no progress / divide by zero).
#[inline]
fn effective_max(max_per_dispatch: u32) -> u32 {
    if max_per_dispatch == 0 { 1 } else { max_per_dispatch }
}

/// Number of bounded dispatches needed to cover `total` items —
/// `ceil(total / effective_max)`. `total == 0` ⇒ `0`. Computed in `u64` so the
/// `+ m - 1` ceil step can't overflow `u32`.
pub fn dispatch_count(total: u32, max_per_dispatch: u32) -> u32 {
    if total == 0 {
        return 0;
    }
    let m = effective_max(max_per_dispatch) as u64;
    let t = total as u64;
    ((t + m - 1) / m) as u32
}

/// Partition `total` cullable items into in-order contiguous sub-ranges, each
/// of size `<= max_per_dispatch` (`0` treated as `1`), together covering
/// `[0, total)` exactly. `total == 0` ⇒ an empty split; `max >= total` ⇒ a
/// single range `(0, total)`.
pub fn split(total: u32, max_per_dispatch: u32) -> Split {
    let n = dispatch_count(total, max_per_dispatch) as usize;
    let mut offsets = Vec::with_capacity(n);
    let mut counts = Vec::with_capacity(n);

    if total == 0 {
        return Split { offsets, counts };
    }

    let m = effective_max(max_per_dispatch);
    let mut offset: u32 = 0;
    // `total - offset` is always > 0 on entry and shrinks by `count` each step.
    while offset < total {
        let remaining = total - offset;
        let count = if remaining < m { remaining } else { m };
        offsets.push(offset);
        counts.push(count);
        offset += count; // count >= 1, so this always makes progress
    }

    Split { offsets, counts }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Assert the structural invariants for a split of `[0, total)`.
    fn check(total: u32, max: u32) -> Split {
        let s = split(total, max);
        let m = if max == 0 { 1 } else { max };

        // offsets/counts share length, and it equals dispatch_count.
        assert_eq!(s.offsets.len(), s.counts.len());
        assert_eq!(s.offsets.len() as u32, dispatch_count(total, max));

        if total == 0 {
            assert!(s.offsets.is_empty());
            return s;
        }

        // contiguity, bounds, coverage
        assert_eq!(s.offsets[0], 0, "must start at 0");
        let mut expected = 0u32;
        for k in 0..s.offsets.len() {
            assert_eq!(s.offsets[k], expected, "range {} not contiguous", k);
            assert!(s.counts[k] >= 1, "range {} empty", k);
            assert!(s.counts[k] <= m, "range {} exceeds max ({} > {})", k, s.counts[k], m);
            expected += s.counts[k];
        }
        // exact coverage of [0, total): last offset + its count == total
        assert_eq!(expected, total, "ranges do not cover [0,{}) exactly", total);
        let sum: u64 = s.counts.iter().map(|&c| c as u64).sum();
        assert_eq!(sum, total as u64, "sum of counts != total");
        s
    }

    #[test]
    fn empty_when_total_zero() {
        let s = check(0, 8);
        assert!(s.offsets.is_empty() && s.counts.is_empty());
        assert_eq!(dispatch_count(0, 8), 0);
        assert_eq!(dispatch_count(0, 0), 0);
    }

    #[test]
    fn single_range_when_max_ge_total() {
        let s = check(7, 7);
        assert_eq!(s.offsets, vec![0]);
        assert_eq!(s.counts, vec![7]);
        let s2 = check(7, 100);
        assert_eq!(s2.offsets, vec![0]);
        assert_eq!(s2.counts, vec![7]);
    }

    #[test]
    fn exact_multiple() {
        let s = check(12, 4);
        assert_eq!(s.offsets, vec![0, 4, 8]);
        assert_eq!(s.counts, vec![4, 4, 4]);
        assert_eq!(dispatch_count(12, 4), 3);
    }

    #[test]
    fn remainder_case() {
        let s = check(10, 4);
        assert_eq!(s.offsets, vec![0, 4, 8]);
        assert_eq!(s.counts, vec![4, 4, 2]);
        assert_eq!(dispatch_count(10, 4), 3);
    }

    #[test]
    fn max_zero_treated_as_one() {
        let s = check(5, 0); // no panic, no div-by-zero
        assert_eq!(s.offsets, vec![0, 1, 2, 3, 4]);
        assert_eq!(s.counts, vec![1, 1, 1, 1, 1]);
        assert_eq!(dispatch_count(5, 0), 5);
    }

    #[test]
    fn dispatch_count_matches_offsets_len() {
        for total in [0u32, 1, 2, 3, 7, 8, 9, 100, 1000] {
            for max in [0u32, 1, 2, 3, 4, 7, 64, 256] {
                assert_eq!(dispatch_count(total, max), split(total, max).offsets.len() as u32);
            }
        }
    }

    #[test]
    fn boundary_total_one() {
        let s = check(1, 1);
        assert_eq!(s.offsets, vec![0]);
        assert_eq!(s.counts, vec![1]);
    }

    #[test]
    fn large_values_no_overflow() {
        // ceil arithmetic in u64: total near u32::MAX with a large cap.
        let total = u32::MAX;
        let max = u32::MAX;
        assert_eq!(dispatch_count(total, max), 1);
        let s = split(total, max);
        assert_eq!(s.counts, vec![u32::MAX]);
        // a cap one below total -> exactly two ranges, no overflow in the ceil
        assert_eq!(dispatch_count(total, u32::MAX - 1), 2);
    }

    #[test]
    fn seeded_sweep_invariants_hold() {
        // deterministic seeded sweep (R20)
        let mut x: u64 = 0xC011_5917_u64; // documented seed
        let mut next = || { x = x.wrapping_mul(6364136223846793005).wrapping_add(1); (x >> 33) as u32 };
        for _ in 0..3_000 {
            let total = next() % 5_000;          // keep allocations modest
            let max = next() % 300;               // includes 0
            check(total, max);
        }
    }

    #[test]
    fn deterministic() {
        assert_eq!(split(1000, 37), split(1000, 37));
        assert_eq!(dispatch_count(1000, 37), dispatch_count(1000, 37));
    }
}
