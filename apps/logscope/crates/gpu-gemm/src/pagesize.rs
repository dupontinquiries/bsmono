//! pagesize.rs — page size is a variable, and the code must not pick one.
//!
//! ARM64 supports 4 KiB, 16 KiB and 64 KiB pages, and all three are in the field right now:
//!
//! * **Asahi is 16 KiB** — Apple's hardware requires it, so it will not change on these machines
//! * **Fedora and RHEL on aarch64 have shipped 64 KiB** kernels, and a future Asahi build could
//! * **Android is moving to 16 KiB**, so phones will land there and stay
//! * **x86-64 is 4 KiB**, which is what most code was written against
//!
//! ## Why this is a correctness question and not a tuning one
//!
//! A zero-copy mmap needs the mapped offset to be page-aligned. Code that assumes 4 KiB **works on
//! x86, silently falls back to a copy on Asahi, and is simply wrong on a 64 KiB kernel** — and the
//! failure mode differs at each size, which is why testing on one machine proves nothing about the
//! others.
//!
//! ## The rule
//!
//! **Align to the largest page size you might meet, not the one you are on.** 64 KiB alignment
//! satisfies 16 and 4 as well; 4 KiB alignment satisfies only itself. Over-aligning wastes at most
//! 60 KiB per allocation and removes a whole class of machine-specific bug.

/// The page sizes that exist on hardware this runs on.
pub const PAGE_4K: usize = 4 * 1024;
pub const PAGE_16K: usize = 16 * 1024;
pub const PAGE_64K: usize = 64 * 1024;

/// Align to this and the mapping works everywhere.
///
/// **Not the running kernel's page size.** A buffer aligned to what this machine happens to use is
/// a buffer that stops working when the file, the cache or the code moves to a machine that uses
/// more — and page size is exactly the kind of thing that changes under you between distro releases.
pub const PORTABLE_ALIGN: usize = PAGE_64K;

/// The running kernel's page size, or `PAGE_4K` if it cannot be determined.
///
/// Falling back to the SMALLEST is deliberate: it makes `is_page_aligned` optimistic and
/// `align_up` conservative in the same direction as being wrong about the machine — a check that
/// passes when it should not is caught by the mapping failing, while an allocation sized too small
/// corrupts memory.
pub fn running_page_size() -> usize {
    #[cfg(unix)]
    {
        // SAFETY: sysconf with a valid name; returns -1 on failure, which the check below handles.
        let v = unsafe { libc_sysconf_pagesize() };
        if v >= PAGE_4K && v.is_power_of_two() { return v }
    }
    PAGE_4K
}

#[cfg(unix)]
unsafe fn libc_sysconf_pagesize() -> usize {
    extern "C" { fn sysconf(name: i32) -> isize; }
    // _SC_PAGESIZE is 30 on Linux and 29 on macOS/BSD. Trying both and taking the plausible answer
    // avoids a target_os cascade for a two-value difference.
    for name in [30i32, 29] {
        let v = unsafe { sysconf(name) };
        if v > 0 && (v as usize).is_power_of_two() && (v as usize) >= PAGE_4K {
            return v as usize;
        }
    }
    0
}

/// Round up to a page boundary.
pub fn align_up(n: usize, page: usize) -> usize {
    if page == 0 || !page.is_power_of_two() { return n }
    (n + page - 1) & !(page - 1)
}

/// Can this offset be mapped directly, without a copy?
pub fn can_map_directly(offset: u64, page: usize) -> bool {
    page != 0 && offset % page as u64 == 0
}

/// Why a mapping would fall back to a copy, phrased so the reason is actionable.
pub fn map_fallback_reason(offset: u64, page: usize) -> Option<String> {
    if can_map_directly(offset, page) { return None }
    Some(format!(
        "offset {offset} is not a multiple of the {} KiB page size, so this mapping copies rather \
than aliases. It would be {} on a 4 KiB kernel — the same file is zero-copy on x86 and a copy here, \
which is why testing the mmap path on one machine proves nothing about the others",
        page / 1024,
        if offset % PAGE_4K as u64 == 0 { "direct" } else { "a copy there too" }))
}

/// Bytes wasted per allocation by aligning portably instead of to the running page size.
///
/// Stated so the tradeoff is a number rather than a feeling: **at most 60 KiB per allocation**, and
/// zero for anything already a multiple of 64 KiB — which most tensor buffers are.
pub fn portable_align_overhead(n: usize) -> usize {
    align_up(n, PORTABLE_ALIGN) - align_up(n, running_page_size())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ALL_THREE_page_sizes_are_in_the_field_and_alignment_differs_at_each() {
        // 16 KiB alignment is not 64 KiB alignment, and a buffer that maps on Asahi does not
        // necessarily map on a 64 KiB Fedora kernel.
        let off = 16 * 1024u64;
        assert!(can_map_directly(off, PAGE_4K));
        assert!(can_map_directly(off, PAGE_16K));
        assert!(!can_map_directly(off, PAGE_64K), "16 KiB aligned is NOT 64 KiB aligned");
    }

    #[test]
    fn PORTABLE_ALIGN_satisfies_every_size_and_the_smallest_satisfies_only_itself() {
        // The rule, in one assertion: align to the largest you might meet, not the one you are on.
        for p in [PAGE_4K, PAGE_16K, PAGE_64K] {
            assert!(can_map_directly(PORTABLE_ALIGN as u64, p), "64K-aligned should map at {p}");
        }
        assert!(!can_map_directly(PAGE_4K as u64, PAGE_64K));
    }

    #[test]
    fn the_fallback_reason_says_it_would_have_WORKED_on_x86() {
        // The actionable part: the same file is zero-copy on a 4 KiB kernel and a copy here, so a
        // developer testing on x86 sees a path that never runs on the target.
        let r = map_fallback_reason(4096, PAGE_16K).unwrap();
        assert!(r.contains("would be direct on a 4 KiB kernel"));
        assert!(r.contains("proves nothing about the others"));
    }

    #[test]
    fn an_offset_misaligned_EVERYWHERE_says_so_rather_than_blaming_the_page_size() {
        // Otherwise the message implies a portability problem where there is only a bad offset.
        let r = map_fallback_reason(1234, PAGE_16K).unwrap();
        assert!(r.contains("a copy there too"));
    }

    #[test]
    fn align_up_is_correct_at_every_size_and_a_noop_when_already_aligned() {
        assert_eq!(align_up(1, PAGE_4K), PAGE_4K);
        assert_eq!(align_up(PAGE_16K, PAGE_16K), PAGE_16K, "already aligned must not grow");
        assert_eq!(align_up(PAGE_16K + 1, PAGE_64K), PAGE_64K);
    }

    #[test]
    fn a_NON_POWER_OF_TWO_page_is_refused_rather_than_masked() {
        // The mask trick is only valid for powers of two, and silently returning a wrong alignment
        // would corrupt a mapping rather than fail it.
        assert_eq!(align_up(100, 3), 100);
        assert!(!can_map_directly(0, 0));
    }

    #[test]
    fn the_detected_size_MATCHES_getconf_where_it_can_be_read() {
        // A detector that silently falls back to 4 KiB would pass every other test in this module
        // while reporting the wrong number on the one machine that matters. Compare against the
        // system's own answer rather than trusting the fallback.
        let detected = running_page_size();
        if let Ok(out) = std::process::Command::new("getconf").arg("PAGESIZE").output() {
            if let Ok(txt) = String::from_utf8(out.stdout) {
                if let Ok(sys) = txt.trim().parse::<usize>() {
                    assert_eq!(detected, sys,
                        "sysconf disagrees with getconf — the fallback is masking a detection bug");
                    return;
                }
            }
        }
        // No getconf (a minimal container, or not unix): the plausibility check is all that is left,
        // and saying so is better than asserting nothing.
        assert!(detected.is_power_of_two() && detected >= PAGE_4K);
    }

    #[test]
    fn the_running_page_size_is_PLAUSIBLE_whatever_the_host() {
        // It must not panic or return nonsense on any machine this builds for, including one where
        // sysconf fails.
        let p = running_page_size();
        assert!(p.is_power_of_two() && p >= PAGE_4K, "got {p}");
    }

    #[test]
    fn the_overhead_of_aligning_portably_is_stated_as_a_NUMBER() {
        // At most 60 KiB per allocation, and zero for anything already a multiple of 64 KiB — which
        // most tensor buffers are.
        assert_eq!(portable_align_overhead(PAGE_64K), 0);
        assert!(portable_align_overhead(1) < PAGE_64K);
    }
}
