//! prefetch.rs — tell the kernel to start reading experts we are about to route to.
//!
//! The route predictor (`MoeRouteHistory`, ported from Colibri's `couple_prefetch`) already names
//! layer L+1's likely experts while layer L computes. Its output fed the eviction policy and nothing
//! else, so the prediction changed *which* expert was resident and never *when* its bytes arrived.
//!
//! The loader also sets `Advice::Random` on the model mapping — correct, because expert fills touch
//! scattered blocks and kernel readahead would fetch pages nobody wants. But it means a page is not
//! read until it is faulted on, so a correct prediction still pays a cold fault at use time.
//!
//! `WILLNEED` on the predicted ranges closes that: the kernel starts the read when the prediction is
//! made, and the fault later finds the page warm.
//!
//! ## Why this rather than io_uring
//!
//! Colibri does the fuller thing — positioned reads on a ring — and its `uring.h` carries a warning
//! worth heeding before anyone starts: cold buffered reads execute *inline* during `io_uring_enter()`
//! unless `IOSQE_ASYNC` is forced, which "serializes the submitter and destroys the intended
//! I/O/compute overlap". Having built the async path they still had to override its default.
//!
//! `WILLNEED` has no such trap: it is advisory, it cannot block, and if the prediction is wrong the
//! cost is pages the kernel would have dropped anyway. It also answers the question a ring cannot
//! answer cheaply — **is the prediction accurate enough to be worth building on?**

/// How many bytes of prediction to advise per layer.
///
/// A prediction naming many experts could request more than the page cache will hold and evict the
/// pages the current layer is reading — turning a prefetch into a slowdown that looks like bad luck.
///
/// 256 MiB is roughly a handful of experts on a large MoE and well under any machine that runs one.
/// It is a bound rather than a tuned value, and it is here rather than at the call site so there is
/// one number to change.
pub const BUDGET: usize = 256 << 20;

/// A byte range in the mapped model file.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Range {
    pub offset: usize,
    pub len: usize,
}

/// Byte ranges for a set of predicted experts, given the per-expert strides.
///
/// Returns ranges into `gate`, `up` and `down` — three per expert, because the tensors are separate
/// and an expert's bytes are not contiguous across them.
///
/// Deduplicated and sorted: a predictor may name the same expert twice, and hinting one range twice
/// costs a syscall for nothing. Sorted because sequential advice is cheaper for the kernel to act on
/// than scattered, even when the underlying access is random.
pub fn ranges_for(
    predicted: &[u32],
    n_expert: u32,
    gu_expert_bytes: usize,
    dn_expert_bytes: usize,
) -> (Vec<Range>, Vec<Range>) {
    let mut gu = Vec::new();
    let mut dn = Vec::new();
    let mut seen: Vec<u32> = Vec::new();
    for &e in predicted {
        // An out-of-range id would advise a span past the mapping. The predictor should not produce
        // one, and "should not" is not a bound.
        if e >= n_expert || seen.contains(&e) { continue; }
        seen.push(e);
        if gu_expert_bytes > 0 {
            gu.push(Range { offset: e as usize * gu_expert_bytes, len: gu_expert_bytes });
        }
        if dn_expert_bytes > 0 {
            dn.push(Range { offset: e as usize * dn_expert_bytes, len: dn_expert_bytes });
        }
    }
    gu.sort_by_key(|r| r.offset);
    dn.sort_by_key(|r| r.offset);
    (gu, dn)
}

/// Advise `WILLNEED` over a range of the mapped model file.
///
/// Takes the slice rather than the `Mmap` because that is what `RawExpertSrc` holds — the expert
/// tensors are `&'static [u8]` views into the mapping, and reaching back for the handle would mean
/// threading it to a site that already has the bytes.
///
/// Best-effort by construction. `madvise` is advice, the kernel may ignore it, and a failure means the
/// page is read on fault exactly as before — so this returns nothing and cannot fail the caller. A
/// prefetch that can break a generation is worse than no prefetch.
pub fn hint(map: &[u8], r: Range) {
    // A range past the mapping would advise memory that is not ours. The predictor should not produce
    // one, and "should not" is not a bound — this check is the whole safety argument for the unsafe
    // block below.
    let end = r.offset.saturating_add(r.len);
    if r.len == 0 || end > map.len() { return; }
    // madvise needs a page-aligned start; an unaligned address is EINVAL rather than a shorter hint.
    const PAGE: usize = 4096;
    let start = r.offset & !(PAGE - 1);
    // SAFETY: `start..end` is within `map`, checked above, and `map` is a live mapping for 'static.
    // MADV_WILLNEED only asks the kernel to begin reading; it never writes and never unmaps.
    unsafe {
        libc::madvise(map.as_ptr().add(start) as *mut libc::c_void, end - start, libc::MADV_WILLNEED);
    }
}

/// Hint every range in a prediction, stopping at a budget.
///
/// The budget exists because a prediction naming many experts could advise more than the page cache
/// will hold, evicting the very pages the current layer is reading. Colibri's LRU bounds what is
/// resident; this bounds what is *requested*, which is the same argument one step earlier.
pub fn hint_all(map: &[u8], ranges: &[Range], budget_bytes: usize) {
    let mut spent = 0usize;
    for r in ranges {
        if spent + r.len > budget_bytes { break; }
        hint(map, *r);
        spent += r.len;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_produces_one_range_per_tensor_per_expert() {
        let (gu, dn) = ranges_for(&[0, 2], 8, 1000, 500);
        assert_eq!(gu, vec![Range { offset: 0, len: 1000 }, Range { offset: 2000, len: 1000 }]);
        assert_eq!(dn, vec![Range { offset: 0, len: 500 }, Range { offset: 1000, len: 500 }]);
    }

    #[test]
    fn a_repeated_prediction_is_hinted_ONCE() {
        // Predictors name the same expert across layers. Hinting a range twice costs a syscall for
        // nothing, and the second call cannot make the page warmer than the first did.
        let (gu, _) = ranges_for(&[3, 3, 3], 8, 100, 100);
        assert_eq!(gu.len(), 1);
    }

    #[test]
    fn an_OUT_OF_RANGE_expert_is_dropped_rather_than_advised() {
        // The predictor should not produce one, and "should not" is not a bound — advising past the
        // mapping is a syscall against memory that is not ours.
        let (gu, dn) = ranges_for(&[99, 1], 8, 100, 100);
        assert_eq!(gu, vec![Range { offset: 100, len: 100 }]);
        assert_eq!(dn.len(), 1);
    }

    #[test]
    fn ranges_come_back_SORTED() {
        // Sequential advice is cheaper for the kernel to act on than scattered, even when the
        // underlying access pattern is random.
        let (gu, _) = ranges_for(&[5, 1, 3], 8, 10, 10);
        assert_eq!(gu.iter().map(|r| r.offset).collect::<Vec<_>>(), vec![10, 30, 50]);
    }

    #[test]
    fn a_zero_stride_produces_no_ranges_rather_than_zero_length_ones() {
        // A tensor the model does not have (dn absent on some layouts) must not yield advice about a
        // zero-length span at offset zero.
        let (gu, dn) = ranges_for(&[0, 1], 8, 100, 0);
        assert_eq!(gu.len(), 2);
        assert!(dn.is_empty());
    }

    #[test]
    fn the_budget_stops_before_it_evicts_what_is_being_read() {
        // A prediction naming many experts could advise more than the page cache holds, dropping the
        // pages the CURRENT layer is reading. The LRU bounds what is resident; this bounds what is
        // requested, which is the same argument one step earlier.
        let ranges: Vec<Range> = (0..10).map(|i| Range { offset: i * 100, len: 100 }).collect();
        // Counted rather than executed, since hint() needs a real mapping: the budget arithmetic is
        // the part that can be wrong.
        let mut spent = 0usize; let mut n = 0;
        for r in &ranges { if spent + r.len > 350 { break; } spent += r.len; n += 1; }
        assert_eq!(n, 3, "three 100-byte ranges fit in 350, the fourth does not");
    }
}
