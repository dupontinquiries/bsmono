//! schedule — stage 3: batch maintenance into ID-derived time windows.
//!
//! Kademlia maintenance is two periodic jobs: refresh buckets that have gone
//! quiet, and re-announce values before their TTL expires. Run naively, each
//! fires on its own timer and every one of them wakes the radio.
//!
//! **The radio has tail energy.** After a transmission the modem stays in a
//! high-power state for seconds waiting for more traffic, so ten wake-ups spread
//! across a minute cost far more than ten transmissions batched into one. That
//! is what this module exists to fix, and it is why the unit here is a *window*
//! rather than a deadline.
//!
//! ## Windows are derived from the node id, not from the clock
//!
//! If every node refreshed at the top of the minute, a network would synchronise
//! into a thundering herd — the classic failure of naive periodic maintenance.
//! Deriving the offset from the node's own id spreads the fleet deterministically
//! across the period, with no coordination and no randomness to persist.
//!
//! ## Ephemeral data never earns a wake-up
//!
//! The `Lifetime` flag trickles app → share-kit → snapcore → scheduler, and this
//! is where it lands. Ephemeral values expire in minutes and are not replicated,
//! so re-announcing one is spending battery to keep alive something that is
//! meant to disappear. Durable data earns a window; ephemeral data rides along
//! in a window that was happening anyway, or waits.

use crate::routing::{NodeId, N_BUCKETS};

/// How the device is powered and connected. The regime flips what we are
/// optimising: on battery the radio dominates, on wifi/plugged the node is
/// awake anyway and CPU is the cost worth minimising.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Regime {
    /// Battery or cellular: minimise radio wake-ups.
    Constrained,
    /// Wifi or plugged: the node is on anyway, minimise CPU instead.
    Relaxed,
}

/// What a scheduled window is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Job {
    /// Refresh a bucket that has gone quiet.
    BucketRefresh(usize),
    /// Re-announce a stored value before its TTL lapses.
    ReAnnounce,
}

/// A maintenance window: everything due in it goes out together.
#[derive(Debug, Clone, PartialEq)]
pub struct Window {
    /// Tick the window opens at.
    pub at: u64,
    pub jobs: Vec<Job>,
}

impl Window {
    /// A window with nothing in it must never be emitted — waking the radio to
    /// do nothing is the exact cost this module exists to avoid.
    pub fn is_empty(&self) -> bool { self.jobs.is_empty() }
}

/// Period between maintenance windows, in ticks. Kademlia's conventional bucket
/// refresh interval is an hour; a tick here is a second.
pub const PERIOD: u64 = 3600;

/// How long a bucket may go unqueried before it needs refreshing.
pub const BUCKET_STALE: u64 = 3600;

/// Re-announce this long before a value's TTL lapses, so a missed window does
/// not drop the value. Half the remaining life, floored — announcing at the
/// last moment means one lost packet loses the data.
pub fn reannounce_lead(ttl: u64) -> u64 { (ttl / 2).max(1) }

/// The tick this node's window opens on, within each period.
///
/// Derived from the id so the fleet spreads deterministically: no coordination,
/// no stored randomness, and the same node always lands in the same slot, which
/// makes the schedule reproducible in tests and in the field.
pub fn window_offset(id: &NodeId, period: u64) -> u64 {
    // Fold the whole id rather than taking a prefix: ids that share a prefix are
    // exactly the ones that are close in the keyspace and therefore most likely
    // to be maintaining the same buckets at the same time.
    let mut acc: u64 = 0;
    for b in id.0.iter() { acc = acc.wrapping_mul(31).wrapping_add(*b as u64); }
    if period == 0 { 0 } else { acc % period }
}

/// The next window opening at or after `now`.
pub fn next_window_at(id: &NodeId, now: u64, period: u64) -> u64 {
    if period == 0 { return now; }
    let off = window_offset(id, period);
    let base = (now / period) * period + off;
    if base >= now { base } else { base + period }
}

/// Plan the next window.
///
/// `last_query` is per-bucket; `values` are `(ttl_expiry, durable)` pairs.
/// Returns `None` when there is nothing to do — the caller must not wake for an
/// empty window.
pub fn plan(id: &NodeId, now: u64, regime: Regime,
            last_query: &[u64; N_BUCKETS], values: &[(u64, bool)]) -> Option<Window> {
    let at = next_window_at(id, now, PERIOD);
    let mut jobs = Vec::new();

    for (i, last) in last_query.iter().enumerate() {
        // A bucket never queried is not automatically stale: a fresh node has
        // 160 empty buckets and refreshing all of them on first boot is the
        // worst possible opening move for a battery.
        if *last == 0 { continue; }
        if at.saturating_sub(*last) >= BUCKET_STALE { jobs.push(Job::BucketRefresh(i)); }
    }

    for (expiry, durable) in values {
        // Ephemeral values are not re-announced: they are meant to disappear,
        // and spending a radio wake-up to keep one alive is spending battery to
        // defeat the user's own choice.
        if !*durable { continue; }
        let ttl_left = expiry.saturating_sub(at);
        if ttl_left <= reannounce_lead(*expiry - now.min(*expiry)) || ttl_left == 0 {
            jobs.push(Job::ReAnnounce);
            break;      // one re-announce pass covers every value due
        }
    }

    if jobs.is_empty() { return None; }

    // In the relaxed regime the node is awake anyway, so a window may open
    // immediately rather than waiting for the id-derived slot: the slot exists
    // to spread RADIO wake-ups, and there is no wake-up to spread.
    let at = if regime == Regime::Relaxed { now } else { at };
    Some(Window { at, jobs })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(seed: u8) -> NodeId {
        let mut b = [0u8; crate::routing::ID_LEN];
        for (i, x) in b.iter_mut().enumerate() { *x = seed.wrapping_add(i as u8); }
        NodeId(b)
    }

    fn quiet() -> [u64; N_BUCKETS] { [0; N_BUCKETS] }

    #[test]
    fn nodes_spread_across_the_period_rather_than_synchronising() {
        // If every node refreshed at the top of the hour the network would
        // thunder. The offset is id-derived, so the spread costs no coordination
        // and nothing has to be stored.
        let offs: Vec<u64> = (0..40u8).map(|s| window_offset(&id(s), PERIOD)).collect();
        let unique: std::collections::BTreeSet<u64> = offs.iter().copied().collect();
        assert!(unique.len() > 30, "only {} distinct slots of 40", unique.len());
        assert!(offs.iter().all(|o| *o < PERIOD));
    }

    #[test]
    fn the_same_node_always_lands_in_the_same_slot() {
        // Reproducible in tests and in the field; a node that moved slot every
        // boot would be indistinguishable from one that is thundering.
        assert_eq!(window_offset(&id(7), PERIOD), window_offset(&id(7), PERIOD));
        assert_ne!(window_offset(&id(7), PERIOD), window_offset(&id(8), PERIOD));
    }

    #[test]
    fn the_whole_id_is_folded_so_close_nodes_do_not_collide() {
        // Ids sharing a prefix are exactly the ones closest in the keyspace, and
        // therefore the ones most likely to maintain the same buckets at once.
        // Taking a prefix would put them in the same slot.
        let mut a = [0u8; crate::routing::ID_LEN];
        let mut b = a;
        a[crate::routing::ID_LEN - 1] = 1;
        b[crate::routing::ID_LEN - 1] = 2;
        assert_ne!(window_offset(&NodeId(a), PERIOD), window_offset(&NodeId(b), PERIOD));
    }

    #[test]
    fn a_fresh_node_does_not_refresh_all_hundred_and_sixty_buckets_on_boot() {
        // The worst possible opening move for a battery. A bucket never queried
        // is not stale, it is empty.
        assert!(plan(&id(1), 10_000, Regime::Constrained, &quiet(), &[]).is_none());
    }

    #[test]
    fn a_quiet_bucket_earns_a_refresh() {
        let mut lq = quiet();
        lq[5] = 1;                       // queried once, long ago
        let w = plan(&id(1), 100_000, Regime::Constrained, &lq, &[]).expect("a window");
        assert!(w.jobs.contains(&Job::BucketRefresh(5)));
        assert!(!w.is_empty());
    }

    #[test]
    fn ephemeral_values_never_wake_the_radio() {
        // They are meant to disappear; re-announcing one spends battery to
        // defeat the user's own choice. This is where the Lifetime flag that
        // trickles app -> share-kit -> snapcore finally lands.
        let due = vec![(100_100u64, false)];      // expiring, but ephemeral
        assert!(plan(&id(1), 100_000, Regime::Constrained, &quiet(), &due).is_none());
        let durable = vec![(100_100u64, true)];
        assert!(plan(&id(1), 100_000, Regime::Constrained, &quiet(), &durable).is_some());
    }

    #[test]
    fn a_value_is_reannounced_with_room_to_spare() {
        // Announcing at the last moment means one lost packet loses the data.
        assert_eq!(reannounce_lead(3600), 1800);
        assert_eq!(reannounce_lead(1), 1, "never zero, or nothing is ever early");
        assert_eq!(reannounce_lead(0), 1);
    }

    #[test]
    fn one_reannounce_pass_covers_every_due_value() {
        // Ten values due in the same window is one radio wake-up, not ten.
        let due: Vec<(u64, bool)> = (0..10).map(|i| (100_050 + i, true)).collect();
        let w = plan(&id(1), 100_000, Regime::Constrained, &quiet(), &due).expect("window");
        assert_eq!(w.jobs.iter().filter(|j| **j == Job::ReAnnounce).count(), 1);
    }

    #[test]
    fn the_relaxed_regime_does_not_wait_for_a_radio_slot() {
        // The slot exists to spread RADIO wake-ups. On wifi or plugged the node
        // is awake anyway, so there is no wake-up to spread and waiting only
        // adds latency.
        let mut lq = quiet();
        lq[3] = 1;
        let now = 100_000;
        let c = plan(&id(1), now, Regime::Constrained, &lq, &[]).unwrap();
        let r = plan(&id(1), now, Regime::Relaxed, &lq, &[]).unwrap();
        assert_eq!(r.at, now, "immediately");
        assert!(c.at >= now, "and the constrained one waits for its slot");
        assert_eq!(c.jobs, r.jobs, "the same work either way");
    }

    #[test]
    fn a_window_is_never_emitted_empty() {
        // Waking the radio to do nothing is the exact cost this module exists
        // to avoid.
        assert!(plan(&id(2), 500, Regime::Constrained, &quiet(), &[]).is_none());
        assert!(plan(&id(2), 500, Regime::Relaxed, &quiet(), &[]).is_none());
    }

    #[test]
    fn the_next_window_is_always_in_the_future() {
        for now in [0u64, 1, 3599, 3600, 3601, 999_999] {
            let at = next_window_at(&id(9), now, PERIOD);
            assert!(at >= now, "now={now} at={at}");
            assert!(at < now + PERIOD, "and within one period");
        }
    }
}
