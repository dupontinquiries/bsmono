//! pagesize — the host page size, and the two places it was assumed to be 4 KiB.
//!
//! Asahi Fedora ships a **16 KiB page kernel** (`7.1.6-400.asahi.fc43.aarch64+16k`),
//! and Apple silicon uses 16 KiB natively. Nothing in this crate asked the OS —
//! two places assumed 4096, and both fail *quietly* rather than loudly.
//!
//! ## 1. `madvise` needs a page-aligned ADDRESS
//!
//! `prefetch_mapped` calls `advise_range(WillNeed, offset, len)`, and memmap2
//! passes `ptr + offset` straight to `madvise`, which returns `EINVAL` when the
//! address is not page-aligned. The result is `.is_ok()`-ed into a `false` that
//! nobody checks.
//!
//! So on a 4 KiB kernel the prefetch works and on a 16 KiB kernel it silently
//! does nothing for any offset that is 4K-aligned but not 16K-aligned — three
//! times out of four. **The symptom is a load that pages in on demand and looks
//! slow, on one kernel and not another**, which is about the worst shape a
//! performance bug can have: it does not reproduce where it is being debugged.
//!
//! ## 2. A size class threshold that means "a page"
//!
//! `pool_class::EXACT_BELOW` was written as 4096 with the comment "a page".
//! On this kernel it is a quarter of one, so allocations between 4 KiB and
//! 16 KiB were being size-classed when the reasoning for classing them —
//! "bigger than a page, so rounding is cheap relative to the mapping" — did not
//! apply yet.
//!
//! ## Why this is queried, not assumed
//!
//! aarch64 Linux supports 4, 16 and 64 KiB pages, and the same binary runs on
//! all three. A constant that is right on the developer's machine and wrong on
//! the deployment target is the shape of every bug in this file's history.

/// The host page size in bytes, queried once.
///
/// Falls back to 16 KiB rather than 4 KiB if the query fails: over-aligning is
/// always safe for `madvise` (it just advises a little extra), while
/// under-aligning is the `EINVAL` this module exists to prevent. The failure
/// mode picks the direction.
pub fn page_size() -> usize {
    use std::sync::OnceLock;
    static PS: OnceLock<usize> = OnceLock::new();
    *PS.get_or_init(|| {
        #[cfg(unix)]
        {
            // SAFETY: sysconf with a valid name and no pointer arguments.
            let v = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
            if v > 0 { return v as usize; }
        }
        16 * 1024
    })
}

/// Round `offset` DOWN and `len` UP so the range is page-aligned and still
/// covers the original request.
///
/// Down for the offset, because `madvise` wants an aligned address and
/// advising a little before the target is harmless. Up for the length, because
/// stopping short would leave the tail of the requested range unadvised — which
/// is the part a sequential read reaches last and needs most.
pub fn align_advice(offset: usize, len: usize, max: usize) -> Option<(usize, usize)> {
    if len == 0 || offset >= max { return None; }
    let ps = page_size();
    let start = offset - (offset % ps);
    let end = (offset + len.min(max - offset)).min(max);
    // Round the end up to a page boundary, then clamp: the final page of a
    // mapping may be partial, and madvise accepts a length running to the end
    // of the mapping even when it is not a whole page.
    let end = end.next_multiple_of(ps).min(max);
    if end <= start { return None; }
    Some((start, end - start))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_page_size_is_queried_and_sane() {
        let ps = page_size();
        assert!(ps.is_power_of_two(), "{ps}");
        assert!(ps >= 4096 && ps <= 64 * 1024, "{ps}");
    }

    #[test]
    fn an_aligned_range_is_returned_aligned() {
        let ps = page_size();
        let (off, len) = align_advice(ps * 3, ps * 2, ps * 100).unwrap();
        assert_eq!(off % ps, 0);
        assert_eq!(off, ps * 3);
        assert_eq!(len, ps * 2);
    }

    #[test]
    fn a_misaligned_offset_rounds_down_rather_than_failing() {
        // The bug: madvise returns EINVAL on an unaligned address, and the
        // caller .is_ok()s it into a false nobody checks. On a 16K kernel that
        // is three offsets in four.
        let ps = page_size();
        let want_off = ps * 3 + 4096;
        let (off, len) = align_advice(want_off, 8192, ps * 100).unwrap();
        assert_eq!(off % ps, 0, "aligned: {off}");
        assert!(off <= want_off, "rounded DOWN so the target is inside");
        assert!(off + len >= want_off + 8192, "and the whole request is still covered");
    }

    #[test]
    fn the_aligned_range_always_covers_the_request() {
        // The property that matters: advising a little extra is free, advising
        // less than asked leaves the tail cold — and the tail is what a
        // sequential read reaches last.
        let max = page_size() * 64;
        for off in (0..max - 8192).step_by(997) {
            let len = 5000;
            let (a, l) = align_advice(off, len, max).unwrap();
            assert_eq!(a % page_size(), 0, "offset {off}");
            assert!(a <= off, "offset {off}");
            assert!(a + l >= (off + len).min(max), "offset {off}: {a}+{l} < {}", off + len);
            assert!(a + l <= max, "offset {off}: past the mapping");
        }
    }

    #[test]
    fn a_range_running_past_the_mapping_is_clamped() {
        // The last page of a mapping is often partial; madvise accepts a length
        // to the end of the mapping, but not past it.
        let max = page_size() * 4 + 100;
        let (off, len) = align_advice(max - 50, 10_000, max).unwrap();
        assert!(off + len <= max);
        assert!(off <= max - 50);
    }

    #[test]
    fn a_zero_length_or_out_of_range_offset_advises_nothing() {
        let max = page_size() * 4;
        assert_eq!(align_advice(0, 0, max), None);
        assert_eq!(align_advice(max, 100, max), None);
        assert_eq!(align_advice(max + 1, 100, max), None);
    }

    #[test]
    fn an_offset_in_the_first_page_starts_at_zero() {
        let (off, len) = align_advice(10, 20, page_size() * 4).unwrap();
        assert_eq!(off, 0);
        assert!(len >= 30);
    }

    #[test]
    fn the_fallback_over_aligns_rather_than_under_aligning() {
        // Over-aligning is always safe for madvise — it advises a little extra.
        // Under-aligning is the EINVAL this module exists to prevent, so the
        // fallback picks the direction that cannot be wrong.
        assert!(page_size() >= 4096);
    }
}
