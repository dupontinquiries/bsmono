//! `render::copy_coalesce` — merge adjacent buffer-copy regions before recording.
//!
//! The mesh upload path (`mesh_buffer_pool::flush_pending_copies`) accumulates one
//! `vk::BufferCopy` region per chunk and hands the whole array to a single
//! `vkCmdCopyBuffer`. The call is already batched, but the *region count* still
//! costs the driver per-region setup, and after many chunks land in adjacent slots
//! the regions are frequently contiguous. Coalescing merges any run that is
//! contiguous in **both** the source and destination into one larger region, so the
//! transfer is byte-for-byte identical but the driver processes far fewer regions.
//!
//! Pure & deterministic over a plain `CopyRegion` (bytes), so it verifies in
//! isolation; the caller converts `vk::BufferCopy` ↔ `CopyRegion`.

/// A pending buffer copy in bytes: `size` bytes from `src` to `dst`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CopyRegion {
    pub src: u64,
    pub dst: u64,
    pub size: u64,
}

impl CopyRegion {
    pub fn new(src: u64, dst: u64, size: u64) -> CopyRegion {
        CopyRegion { src, dst, size }
    }
}

/// Coalesce copy regions into the smallest equivalent set. Two regions merge only
/// when the first ends exactly where the second begins in **both** address spaces
/// (`src+size == next.src` and `dst+size == next.dst`), which makes the merge
/// provably byte-identical to issuing them separately. Regions that are contiguous
/// in only one space are left alone (they are genuinely different transfers).
///
/// Output is sorted by `(dst, src)` and deterministic. Total bytes are conserved.
pub fn coalesced(regions: &[CopyRegion]) -> Vec<CopyRegion> {
    if regions.len() <= 1 {
        return regions.to_vec();
    }
    let mut sorted = regions.to_vec();
    sorted.sort_by_key(|r| (r.dst, r.src));

    let mut out: Vec<CopyRegion> = Vec::with_capacity(sorted.len());
    for r in sorted {
        if r.size == 0 {
            continue; // empty transfers carry no bytes; drop them
        }
        if let Some(last) = out.last_mut() {
            if last.dst.wrapping_add(last.size) == r.dst && last.src.wrapping_add(last.size) == r.src {
                last.size += r.size;
                continue;
            }
        }
        out.push(r);
    }
    out
}

/// Coalesce in place, returning how many regions were eliminated (for logging the
/// batching win). `regions` is replaced with the coalesced set.
pub fn coalesce_in_place(regions: &mut Vec<CopyRegion>) -> usize {
    let before = regions.len();
    let merged = coalesced(regions);
    let removed = before - merged.len();
    *regions = merged;
    removed
}

/// Total bytes across a region set — invariant across coalescing.
pub fn total_bytes(regions: &[CopyRegion]) -> u64 {
    regions.iter().map(|r| r.size).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contiguous_run_merges_to_one() {
        let regions = [
            CopyRegion::new(0, 0, 64),
            CopyRegion::new(64, 64, 64),
            CopyRegion::new(128, 128, 64),
        ];
        let out = coalesced(&regions);
        assert_eq!(out, vec![CopyRegion::new(0, 0, 192)]);
    }

    #[test]
    fn discontiguous_src_does_not_merge() {
        // dst is contiguous but src jumps — these are different transfers.
        let regions = [
            CopyRegion::new(0, 0, 64),
            CopyRegion::new(4096, 64, 64),
        ];
        let out = coalesced(&regions);
        assert_eq!(out.len(), 2, "must not merge across a src gap");
    }

    #[test]
    fn dst_gap_does_not_merge() {
        let regions = [
            CopyRegion::new(0, 0, 64),
            CopyRegion::new(64, 128, 64), // dst gap of 64
        ];
        assert_eq!(coalesced(&regions).len(), 2);
    }

    #[test]
    fn bytes_are_conserved() {
        let regions = [
            CopyRegion::new(0, 0, 64),
            CopyRegion::new(64, 64, 64),
            CopyRegion::new(500, 500, 32),
            CopyRegion::new(532, 532, 8),
        ];
        let before = total_bytes(&regions);
        let out = coalesced(&regions);
        assert_eq!(total_bytes(&out), before, "coalescing conserves bytes");
        // Two runs: [0..128] and [500..540].
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn out_of_order_input_still_coalesces() {
        // Same run presented shuffled — sort makes it contiguous again.
        let regions = [
            CopyRegion::new(128, 128, 64),
            CopyRegion::new(0, 0, 64),
            CopyRegion::new(64, 64, 64),
        ];
        assert_eq!(coalesced(&regions), vec![CopyRegion::new(0, 0, 192)]);
    }

    #[test]
    fn in_place_reports_removed_count() {
        let mut regions = vec![
            CopyRegion::new(0, 0, 10),
            CopyRegion::new(10, 10, 10),
            CopyRegion::new(20, 20, 10),
        ];
        let removed = coalesce_in_place(&mut regions);
        assert_eq!(removed, 2); // 3 → 1
        assert_eq!(regions, vec![CopyRegion::new(0, 0, 30)]);
    }

    #[test]
    fn empty_and_single_are_stable() {
        assert!(coalesced(&[]).is_empty());
        let one = [CopyRegion::new(5, 9, 3)];
        assert_eq!(coalesced(&one), one.to_vec());
    }

    #[test]
    fn zero_size_regions_are_dropped() {
        let regions = [CopyRegion::new(0, 0, 0), CopyRegion::new(0, 0, 64)];
        let out = coalesced(&regions);
        assert_eq!(out, vec![CopyRegion::new(0, 0, 64)]);
    }
}
