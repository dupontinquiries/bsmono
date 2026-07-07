//! `sim::net::world_presence` — host presence heartbeat + browser staleness pruning
//! (roadmap **item 3**, §A; WID r336-coordinator).
//!
//! Discovery has two liveness halves that neither `world_advert` (the record) nor
//! `session_mode::ServerBrowser` (dedupe + freshest-first) cover:
//!   - **host side:** a world Opened to DHT must *re-announce* periodically so its advert
//!     stays fresh (the DHT record TTLs out), refreshing `WorldAdvert.last_seen_ms`.
//!   - **browser side:** a client must *evict* worlds it hasn't heard from within the TTL,
//!     so the Join list doesn't show dead hosts.
//! This module is the pure timing + freshness logic for both; the actual DHT put/get and the
//! clock are bin glue (`now_ms` is passed in). It reuses the cadence constants from
//! `sim::net_channels` (`ADVERT_REANNOUNCE_MS`, `DHT_TTL_MS`) — mirrored here as defaults so
//! the module stays self-contained.
//!
//! Pure: no IO/clock/RNG, no `unwrap`/`expect`/`panic!`, saturating time arithmetic.

/// Default host re-announce period (ms). Mirrors `net_channels::ADVERT_REANNOUNCE_MS`.
pub const REANNOUNCE_MS: u64 = 15_000;
/// Default advert time-to-live (ms): unheard past this → evicted. Mirrors `DHT_TTL_MS`.
pub const TTL_MS: u64 = 45_000;

/// Liveness of an advert given how long since it was last seen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Freshness {
    /// Seen within one re-announce period — definitely live.
    Fresh,
    /// Past one period but within TTL — shown, but greyed/at risk.
    Stale,
    /// Past TTL — evict from the browser.
    Dead,
}

/// Classify an advert's liveness. `now_ms < last_seen_ms` (clock skew) is treated as `Fresh`.
pub fn freshness(last_seen_ms: u64, now_ms: u64, reannounce_ms: u64, ttl_ms: u64) -> Freshness {
    let age = now_ms.saturating_sub(last_seen_ms);
    if age <= reannounce_ms {
        Freshness::Fresh
    } else if age <= ttl_ms {
        Freshness::Stale
    } else {
        Freshness::Dead
    }
}

/// Host-side re-announce scheduler: decides when to re-publish the advert so it never TTLs out.
#[derive(Clone, Copy, Debug)]
pub struct PresenceBeacon {
    period_ms: u64,
    last_emit_ms: u64,
    emitted: bool,
}

impl PresenceBeacon {
    pub fn new(period_ms: u64) -> PresenceBeacon {
        PresenceBeacon { period_ms, last_emit_ms: 0, emitted: false }
    }
    /// Default cadence (`REANNOUNCE_MS`).
    pub fn default_cadence() -> PresenceBeacon {
        PresenceBeacon::new(REANNOUNCE_MS)
    }

    /// True if it's time to re-announce (never emitted yet, or a full period has elapsed).
    pub fn due(&self, now_ms: u64) -> bool {
        !self.emitted || now_ms.saturating_sub(self.last_emit_ms) >= self.period_ms
    }

    /// Record that an announce happened at `now_ms`; returns the `last_seen_ms` the refreshed
    /// advert should carry (i.e. `now_ms`). Call only when [`due`] (caller publishes).
    pub fn emit(&mut self, now_ms: u64) -> u64 {
        self.last_emit_ms = now_ms;
        self.emitted = true;
        now_ms
    }

    /// Milliseconds until the next announce is due (0 if due now).
    pub fn until_due(&self, now_ms: u64) -> u64 {
        if self.due(now_ms) {
            0
        } else {
            self.period_ms.saturating_sub(now_ms.saturating_sub(self.last_emit_ms))
        }
    }
}

/// Browser-side eviction: retain only adverts still within TTL, by their `last_seen_ms`.
/// Generic over the advert so it doesn't hard-depend on `WorldAdvert` (the bin passes a
/// `last_seen` accessor). Returns how many were dropped.
pub fn prune_stale<T, F>(adverts: &mut Vec<T>, now_ms: u64, ttl_ms: u64, last_seen: F) -> usize
where
    F: Fn(&T) -> u64,
{
    let before = adverts.len();
    adverts.retain(|a| freshness(last_seen(a), now_ms, REANNOUNCE_MS, ttl_ms) != Freshness::Dead);
    before - adverts.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn freshness_bands() {
        // last seen at t=1000
        assert_eq!(freshness(1000, 1000, REANNOUNCE_MS, TTL_MS), Freshness::Fresh);
        assert_eq!(freshness(1000, 1000 + REANNOUNCE_MS, REANNOUNCE_MS, TTL_MS), Freshness::Fresh);
        assert_eq!(
            freshness(1000, 1000 + REANNOUNCE_MS + 1, REANNOUNCE_MS, TTL_MS),
            Freshness::Stale
        );
        assert_eq!(freshness(1000, 1000 + TTL_MS, REANNOUNCE_MS, TTL_MS), Freshness::Stale);
        assert_eq!(freshness(1000, 1000 + TTL_MS + 1, REANNOUNCE_MS, TTL_MS), Freshness::Dead);
    }

    #[test]
    fn clock_skew_is_fresh_not_dead() {
        assert_eq!(freshness(5000, 1000, REANNOUNCE_MS, TTL_MS), Freshness::Fresh);
    }

    #[test]
    fn beacon_due_initially_then_after_period() {
        let mut b = PresenceBeacon::new(15_000);
        assert!(b.due(0), "never emitted → due");
        let seen = b.emit(1_000);
        assert_eq!(seen, 1_000);
        assert!(!b.due(1_000), "just emitted");
        assert!(!b.due(15_999), "within period");
        assert!(b.due(16_000), "a full period later → due");
    }

    #[test]
    fn beacon_until_due_counts_down() {
        let mut b = PresenceBeacon::new(15_000);
        b.emit(1_000);
        assert_eq!(b.until_due(1_000), 15_000);
        assert_eq!(b.until_due(6_000), 10_000);
        assert_eq!(b.until_due(16_000), 0);
        assert_eq!(b.until_due(99_999), 0);
    }

    #[test]
    fn emit_refreshes_last_seen() {
        let mut b = PresenceBeacon::default_cadence();
        let s1 = b.emit(1_000);
        let s2 = b.emit(20_000);
        assert!(s2 > s1 && s2 == 20_000);
    }

    #[test]
    fn prune_drops_only_dead() {
        // (id, last_seen_ms)
        let mut adverts = vec![("fresh", 9_000u64), ("stale", 5_000), ("dead", 100)];
        let now = 10_000;
        let dropped = prune_stale(&mut adverts, now, TTL_MS, |a| a.1);
        // dead: age 9900 > TTL 45000? no — adjust to actually exceed TTL
        // recompute with a clearly-dead entry
        let _ = dropped;
        let mut adverts = vec![("fresh", 9_000u64), ("dead", 1_000)];
        let now = 9_000 + TTL_MS + 5_000; // fresh is dead too now; make fresh recent
        let mut adverts2 = vec![("recent", now - 1_000), ("old", now - TTL_MS - 1)];
        let dropped = prune_stale(&mut adverts2, now, TTL_MS, |a| a.1);
        assert_eq!(dropped, 1, "only the >TTL one dropped");
        assert_eq!(adverts2.len(), 1);
        assert_eq!(adverts2[0].0, "recent");
        let _ = &mut adverts;
    }

    #[test]
    fn prune_empty_is_noop() {
        let mut adverts: Vec<(&str, u64)> = Vec::new();
        assert_eq!(prune_stale(&mut adverts, 10_000, TTL_MS, |a| a.1), 0);
    }

    #[test]
    fn ttl_exceeds_reannounce() {
        // a host re-announcing on cadence stays Fresh/Stale, never Dead, between announces
        assert!(TTL_MS > REANNOUNCE_MS, "TTL must outlast the re-announce period");
    }
}
