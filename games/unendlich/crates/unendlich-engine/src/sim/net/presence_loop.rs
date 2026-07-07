//! `sim::net::presence_loop` — tie the two discovery-liveness halves together.
//!
//! [`world_presence`](crate::sim::net::world_presence) supplies the raw timing —
//! a re-announce scheduler ([`PresenceBeacon`]), a freshness classifier, and a
//! staleness prune — but nothing composes them around an actual
//! [`WorldAdvert`](crate::sim::session_mode::WorldAdvert). This does:
//!
//! - **Host** ([`HostAnnouncer`]): on cadence, refresh the advert's `last_seen_ms`
//!   and hand it back to publish, so the record never TTLs out of the DHT / a
//!   browser's list.
//! - **Browser** ([`classify`] / [`evict_dead`]): tag each discovered advert
//!   Fresh / Stale / Dead (so the Join UI can grey at-risk worlds) and drop the
//!   Dead ones.
//!
//! Pure and wire-agnostic: it operates on `WorldAdvert` (which carries
//! `world_seed`), leaving the byte encoding to the caller's chosen codec
//! (`session_wire` for LAN, the DHT advert codec for WAN). The clock is passed in.

use crate::sim::net::world_presence::{freshness, prune_stale, Freshness, PresenceBeacon, TTL_MS};
use crate::sim::session_mode::WorldAdvert;

/// Host-side re-announce driver around a [`PresenceBeacon`].
#[derive(Clone, Copy, Debug)]
pub struct HostAnnouncer {
    beacon: PresenceBeacon,
}

impl HostAnnouncer {
    /// Default re-announce cadence.
    pub fn new() -> HostAnnouncer {
        HostAnnouncer { beacon: PresenceBeacon::default_cadence() }
    }

    /// Explicit cadence (ms between re-announces).
    pub fn with_period(period_ms: u64) -> HostAnnouncer {
        HostAnnouncer { beacon: PresenceBeacon::new(period_ms) }
    }

    /// If a re-announce is due (or none has happened yet), stamp the advert with
    /// `now_ms` and return the refreshed copy to publish; otherwise `None`.
    pub fn poll(&mut self, advert: &WorldAdvert, now_ms: u64) -> Option<WorldAdvert> {
        if self.beacon.due(now_ms) {
            let last_seen = self.beacon.emit(now_ms);
            let mut refreshed = advert.clone();
            refreshed.last_seen_ms = last_seen;
            Some(refreshed)
        } else {
            None
        }
    }

    /// Milliseconds until the next announce is due (0 if due now) — for sleeping
    /// the announce timer.
    pub fn until_due(&self, now_ms: u64) -> u64 {
        self.beacon.until_due(now_ms)
    }
}

impl Default for HostAnnouncer {
    fn default() -> Self {
        HostAnnouncer::new()
    }
}

/// Browser-side: classify each advert's liveness (for greying at-risk worlds in
/// the Join list before they're evicted), in the same order as `adverts`.
pub fn classify(adverts: &[WorldAdvert], now_ms: u64, reannounce_ms: u64, ttl_ms: u64) -> Vec<Freshness> {
    adverts
        .iter()
        .map(|a| freshness(a.last_seen_ms, now_ms, reannounce_ms, ttl_ms))
        .collect()
}

/// Browser-side: drop adverts past TTL (Dead), returning how many were evicted.
/// Uses `world_presence::prune_stale`, which classifies via the module's
/// `REANNOUNCE_MS`; pass a `ttl_ms >= REANNOUNCE_MS` (the real `TTL_MS` is) so
/// "Dead" means "past TTL" rather than being masked by the Fresh window.
pub fn evict_dead(adverts: &mut Vec<WorldAdvert>, now_ms: u64, ttl_ms: u64) -> usize {
    prune_stale(adverts, now_ms, ttl_ms, |a| a.last_seen_ms)
}

/// Convenience eviction at the default TTL.
pub fn evict_dead_default(adverts: &mut Vec<WorldAdvert>, now_ms: u64) -> usize {
    evict_dead(adverts, now_ms, TTL_MS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn advert(last_seen: u64) -> WorldAdvert {
        WorldAdvert {
            host_username: "h".into(),
            world_name: "w".into(),
            players: 1,
            max_players: 4,
            last_seen_ms: last_seen,
            candidates: Vec::new(),
            world_seed: 0x5EED,
        }
    }

    #[test]
    fn host_announces_then_waits_a_period_then_announces_again() {
        let mut h = HostAnnouncer::with_period(1000);
        let a = advert(0);
        // first poll: never announced → due, stamps now
        let first = h.poll(&a, 5_000).expect("first announce is due");
        assert_eq!(first.last_seen_ms, 5_000);
        assert_eq!(first.world_seed, a.world_seed, "seed preserved (wire-agnostic)");
        // within the period: not due
        assert!(h.poll(&a, 5_500).is_none());
        // after a full period: due again, restamped
        let again = h.poll(&a, 6_000).expect("re-announce due");
        assert_eq!(again.last_seen_ms, 6_000);
    }

    #[test]
    fn classify_fresh_stale_dead() {
        // reannounce 1000, ttl 3000; now = 10_000
        let adverts = vec![
            advert(9_500), // age 500 → Fresh
            advert(8_500), // age 1500 → Stale
            advert(6_000), // age 4000 → Dead
        ];
        let f = classify(&adverts, 10_000, 1_000, 3_000);
        assert_eq!(f, vec![Freshness::Fresh, Freshness::Stale, Freshness::Dead]);
    }

    #[test]
    fn evict_drops_only_dead() {
        // ttl must be >= REANNOUNCE_MS (15s) for eviction to mean "past ttl";
        // use the realistic 45s TTL. Ages below: 10s (Fresh), 20s (Stale), 50s (Dead).
        let mut adverts = vec![advert(40_000), advert(30_000), advert(0)];
        let dropped = evict_dead(&mut adverts, 50_000, 45_000);
        assert_eq!(dropped, 1, "only the 50s-old advert is past the 45s TTL");
        assert_eq!(adverts.len(), 2);
    }

    #[test]
    fn full_loop_host_keeps_browser_entry_alive() {
        // Realistic cadence: re-announce every 15s, evict past 45s.
        let mut host = HostAnnouncer::with_period(15_000);
        let base = advert(0);
        let mut browser: Vec<WorldAdvert> = Vec::new();

        // t=15s: host announces; browser observes.
        if let Some(a) = host.poll(&base, 15_000) {
            browser.push(a);
        }
        // t=40s: age 25s < 45s TTL → still alive.
        assert_eq!(evict_dead(&mut browser, 40_000, 45_000), 0, "not dead yet");
        // host re-announces at t=45s (a full period since the last), refreshing last_seen.
        if let Some(a) = host.poll(&base, 45_000) {
            browser[0] = a;
        }
        // t=70s: age since the refresh is 25s < 45s → kept alive by the re-announce.
        assert_eq!(evict_dead(&mut browser, 70_000, 45_000), 0, "kept alive by re-announce");
        assert_eq!(browser.len(), 1);
        // if the host then goes silent, the entry dies past TTL (age 50s since refresh).
        assert_eq!(evict_dead(&mut browser, 95_000, 45_000), 1, "silent host evicted");
    }
}
