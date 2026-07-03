//! Sync stagger logic — ported from handleV3Sync in SnapRouter.cpp.
//!
//! When a peer reconnects, we send them all undelivered snaps. To avoid
//! clobbering the UDP channel (which can only track one in-flight chunked
//! transfer per peer), we stagger sends by kStaggerMs per snap.
//!
//! The C++ uses QTimer::singleShot(i * 60, ...). This module provides the
//! same semantics as an async iterator over (snap_id, delay) pairs.

use std::time::Duration;

/// Stagger interval between consecutive snap sends (matches C++ kSnapSendStaggerMs).
pub const STAGGER_MS: u64 = 60;

/// Maximum snaps to sync in one burst (matches C++ kMaxSyncBurst).
pub const MAX_BURST: usize = 50;

/// Compute the send schedule for a batch of undelivered snap IDs.
/// Returns `(snap_id, delay)` pairs in send order.
///
/// ANNOUNCE-only snaps (peer already has the blob) get delay=0.
/// DATA snaps are staggered to prevent UDP channel overwrite.
pub fn stagger_schedule<'a>(
    snap_ids: &'a [String],
    peer_has_blob: impl Fn(&str) -> bool,
) -> impl Iterator<Item = (&'a str, Duration)> {
    let mut data_count: u64 = 0;
    snap_ids.iter().take(MAX_BURST).map(move |id| {
        if peer_has_blob(id.as_str()) {
            (id.as_str(), Duration::ZERO)
        } else {
            let delay = Duration::from_millis(data_count * STAGGER_MS);
            data_count += 1;
            (id.as_str(), delay)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn announce_only_no_delay() {
        let ids: Vec<String> = (0..3).map(|i| format!("snap-{}", i)).collect();
        let has_blob: HashSet<&str> = ids.iter().map(|s| s.as_str()).collect();
        let schedule: Vec<_> = stagger_schedule(&ids, |id| has_blob.contains(id)).collect();
        for (_, delay) in &schedule {
            assert_eq!(*delay, Duration::ZERO);
        }
    }

    #[test]
    fn data_snaps_staggered() {
        let ids: Vec<String> = (0..3).map(|i| format!("snap-{}", i)).collect();
        let schedule: Vec<_> = stagger_schedule(&ids, |_| false).collect();
        assert_eq!(schedule[0].1, Duration::ZERO);
        assert_eq!(schedule[1].1, Duration::from_millis(STAGGER_MS));
        assert_eq!(schedule[2].1, Duration::from_millis(STAGGER_MS * 2));
    }

    #[test]
    fn respects_max_burst() {
        let ids: Vec<String> = (0..100).map(|i| format!("snap-{}", i)).collect();
        let schedule: Vec<_> = stagger_schedule(&ids, |_| false).collect();
        assert_eq!(schedule.len(), MAX_BURST);
    }
}
