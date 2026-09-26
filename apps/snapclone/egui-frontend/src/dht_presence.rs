//! dht_presence — when to publish, and who to look up.
//!
//! The DHT node spawns, bootstraps and polls. Before this module it
//! published nothing and resolved nobody: `NodeHost::publish` and
//! `NodeHost::resolve` had no caller in the tree, so the node maintained a
//! routing table on behalf of the network and got nothing back for it, and
//! every peer the app found came from LAN gossip. This decides when to do
//! either.
//!
//! This module is the decision half, kept pure so the cadence can be tested
//! without a socket. `tick_dht` performs what it decides.
//!
//! ## Why a scheduler and not "resolve everyone each tick"
//!
//! A lookup is an iterative walk of the DHT — several round trips to several
//! nodes. Issuing one per contact per tick, for an address book of thirty on
//! a phone, is a burst of traffic every tick that mostly re-asks questions
//! already in flight. Worse, it is indistinguishable from a scraper at the
//! other end.
//!
//! So: **one lookup in flight at a time**, round-robin over the candidates,
//! and a per-contact cooldown so nobody is re-asked about until the answer
//! could plausibly have changed.
//!
//! ## Who is a candidate
//!
//! Contacts who are NOT currently visible on the LAN. Gossip is better
//! evidence (it is signed, and it means they are on this subnet right now),
//! so asking the DHT about someone two metres away is spending network to
//! learn something worse. This is also why the results land as
//! `Reach::Unverified` — see `peer_source`.
//!
//! ## Publishing is a privacy decision, not a plumbing one
//!
//! Publishing writes "this sc-ID is at this address" into a public DHT,
//! where anyone who knows the id can read it. That is the entire point —
//! and it is also a location beacon. It is therefore gated on the discovery
//! setting the user chose, republished on a slow cadence rather than
//! continuously, and [`Plan::publish`] carries the address so the caller
//! cannot accidentally publish one the user did not agree to.

/// How long before a contact may be looked up again. Ten minutes: long
/// enough that a phone is not walking its address book, short enough that
/// someone who came online is found within a coffee break.
pub const LOOKUP_COOLDOWN_MS: u64 = 10 * 60 * 1000;

/// How often to republish our own presence: every 8 minutes
/// (`node_host::REPUBLISH_WITHIN`).
///
/// This was 55 minutes, copied from the Qt build's `kDhtReannounceMs` on the
/// belief that a DHT value lives about an hour. It does not: OpenDHT expires a
/// value 10 minutes after it was stored (`DEFAULT_VALUE_EXPIRATION`,
/// vendor/opendht/include/opendht/value.h). The Qt build gets away with 55
/// because its presence put is `permanent=true`, and a permanent put is
/// refreshed by the local OpenDHT node itself before it expires; the 55-minute
/// timer only re-announces on top of that. This node has no such machinery, so
/// the record was findable for 10 minutes out of every 55.
///
/// 8 minutes is what the Qt build uses for its own non-permanent records
/// (the connection-request mailbox) against the same 10-minute expiry.
pub const PUBLISH_EVERY_MS: u64 = snapclone_presence::node_host::REPUBLISH_WITHIN * 1000;

/// A lookup that has not answered in this long is presumed lost, so the
/// scheduler is not wedged forever on one unlucky id. The DHT layer has no
/// timeout of its own that reaches back here.
pub const LOOKUP_TIMEOUT_MS: u64 = 60 * 1000;

/// How many empty lookups before a node calls itself silent. Three: one is
/// an offline contact, two is a quiet afternoon, three with nothing at all
/// is a node that is not reaching the network.
pub const SILENT_AFTER: u32 = 3;

/// What the caller should do on this tick.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Plan {
    /// Publish our presence now. `None` means not yet.
    pub publish: bool,
    /// Look this sc-ID up now. `None` means nothing due, or one is already
    /// in flight.
    pub resolve: Option<String>,
}

impl Plan {
    pub fn is_empty(&self) -> bool {
        !self.publish && self.resolve.is_none()
    }
}

/// The scheduler's memory.
#[derive(Debug, Clone, Default)]
pub struct Schedule {
    last_publish_ms: Option<u64>,
    /// (sc_id, when we last asked).
    asked: Vec<(String, u64)>,
    /// The lookup currently in flight, and when it started.
    in_flight: Option<(String, u64)>,
    /// Lookups that came back, and how many of those found anything.
    answered: u32,
    fruitful: u32,
    /// Said once. A silent DHT is the failure mode this counts for.
    reported_silent: bool,
}

impl Schedule {
    pub fn new() -> Schedule {
        Schedule::default()
    }

    pub fn in_flight(&self) -> Option<&str> {
        self.in_flight.as_ref().map(|(id, _)| id.as_str())
    }

    /// Decide what to do now.
    ///
    /// `candidates` is the caller's already-filtered list — contacts not
    /// currently visible on the LAN, in a stable order. Order matters: this
    /// walks it round-robin by "least recently asked", so a stable input
    /// gives a fair rotation and a shuffling one does not starve anybody
    /// either.
    pub fn tick(&mut self, now_ms: u64, candidates: &[String], may_publish: bool) -> Plan {
        let mut plan = Plan::default();

        if may_publish
            && self.last_publish_ms.is_none_or(|t| now_ms.saturating_sub(t) >= PUBLISH_EVERY_MS)
        {
            self.last_publish_ms = Some(now_ms);
            plan.publish = true;
        }

        // A lookup that never answered must not wedge the rotation.
        if let Some((_, started)) = &self.in_flight {
            if now_ms.saturating_sub(*started) >= LOOKUP_TIMEOUT_MS {
                self.in_flight = None;
            } else {
                return plan;
            }
        }

        // Least recently asked first; never-asked counts as longest ago.
        let mut best: Option<(&String, u64)> = None;
        for id in candidates {
            let last = self.asked.iter().find(|(a, _)| a == id).map(|(_, t)| *t);
            if let Some(t) = last {
                if now_ms.saturating_sub(t) < LOOKUP_COOLDOWN_MS {
                    continue;
                }
            }
            // 0 sorts first, which is what "never asked" should do.
            let key = last.map(|t| t + 1).unwrap_or(0);
            if best.is_none_or(|(_, b)| key < b) {
                best = Some((id, key));
            }
        }
        if let Some((id, _)) = best {
            let id = id.clone();
            self.note_asked(&id, now_ms);
            self.in_flight = Some((id.clone(), now_ms));
            plan.resolve = Some(id);
        }
        plan
    }

    /// Publish on the next tick instead of waiting out the interval. For a
    /// publish that reached nobody (the node had no usable peers yet): once the
    /// table recovers, waiting eight more minutes to be findable is the same as
    /// having failed.
    pub fn republish_soon(&mut self) {
        self.last_publish_ms = None;
    }

    /// Has this node asked enough times, and heard nothing at all, that
    /// something is wrong rather than unlucky?
    ///
    /// A DHT that finds nobody looks exactly like a DHT whose contacts are
    /// all offline, and the difference matters enormously: one is a quiet
    /// afternoon, the other is a node that cannot speak to the network it
    /// bootstraps against. Nothing in the DHT reports the second, so it
    /// has to be inferred, and the inference is this: several completed
    /// lookups, none of which returned a single record.
    ///
    /// Reports ONCE. The condition stays true forever once it is met, and
    /// a note re-set every lookup would sit on the page permanently.
    pub fn silent(&mut self) -> bool {
        if self.reported_silent || self.fruitful > 0 || self.answered < SILENT_AFTER {
            return false;
        }
        self.reported_silent = true;
        true
    }

    /// A lookup came back — answered or empty, both end it.
    ///
    /// An empty result is NOT retried sooner than a found one. "Not in the
    /// DHT" is the common case for a contact who is simply offline, and
    /// retrying it faster would spend the most traffic on the peers least
    /// likely to appear.
    pub fn finished(&mut self, sc_id: &str, found_any: bool) {
        if self.in_flight.as_ref().is_some_and(|(id, _)| id == sc_id) {
            self.in_flight = None;
        }
        self.answered = self.answered.saturating_add(1);
        if found_any {
            self.fruitful = self.fruitful.saturating_add(1);
        }
    }

    /// Forget everything. Used when the node goes away, so a restart does
    /// not sit through a cooldown that belonged to a dead socket.
    pub fn reset(&mut self) {
        *self = Schedule::default();
    }

    fn note_asked(&mut self, id: &str, now_ms: u64) {
        match self.asked.iter_mut().find(|(a, _)| a == id) {
            Some(e) => e.1 = now_ms,
            None => self.asked.push((id.to_string(), now_ms)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("sc-{i:08x}")).collect()
    }

    #[test]
    fn only_one_lookup_is_in_flight_at_a_time() {
        // The alternative — a lookup per contact per tick — is a burst of
        // iterative DHT walks every tick, mostly re-asking questions already
        // outstanding, and indistinguishable from a scraper at the far end.
        let mut s = Schedule::new();
        let c = ids(5);
        let first = s.tick(1_000, &c, false).resolve.expect("nothing was scheduled");
        assert_eq!(s.tick(1_100, &c, false).resolve, None, "a second went out concurrently");
        s.finished(&first, false);
        let second = s.tick(1_200, &c, false).resolve.expect("the rotation stalled");
        assert_ne!(second, first, "the same contact was asked twice in a row");
    }

    #[test]
    fn the_rotation_reaches_everyone_before_repeating() {
        let mut s = Schedule::new();
        let c = ids(4);
        let mut seen = Vec::new();
        let mut t = 0;
        for _ in 0..4 {
            t += 100;
            let id = s.tick(t, &c, false).resolve.expect("the rotation stopped early");
            s.finished(&id, false);
            seen.push(id);
        }
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), 4, "somebody was asked twice while another waited");
        // A fifth turn finds everyone inside the cooldown.
        assert_eq!(s.tick(t + 100, &c, false).resolve, None,
                   "the cooldown was ignored and the walk started over");
    }

    #[test]
    fn a_lookup_that_never_answers_does_not_wedge_the_rotation() {
        // The DHT layer has no timeout that reaches back here, so without
        // this the scheduler waits forever on one unlucky id and the rest of
        // the address book is never looked up again.
        let mut s = Schedule::new();
        let c = ids(3);
        let stuck = s.tick(0, &c, false).resolve.expect("nothing scheduled");
        assert_eq!(s.tick(LOOKUP_TIMEOUT_MS - 1, &c, false).resolve, None);
        let next = s.tick(LOOKUP_TIMEOUT_MS, &c, false).resolve
            .expect("still wedged after the timeout");
        assert_ne!(next, stuck);
    }

    /// The record must be republished before OpenDHT expires it (10 minutes).
    #[test]
    fn the_republish_interval_beats_the_value_lifetime() {
        assert!(PUBLISH_EVERY_MS < 10 * 60 * 1000, "republishing every {PUBLISH_EVERY_MS} ms outlives the record");
        assert!(PUBLISH_EVERY_MS >= 5 * 60 * 1000, "republishing every {PUBLISH_EVERY_MS} ms is radio for nothing");
        let mut s = Schedule::new();
        assert!(s.tick(0, &[], true).publish);
        assert!(!s.tick(60_000, &[], true).publish);
        s.republish_soon();
        assert!(s.tick(61_000, &[], true).publish, "republish_soon did not publish on the next tick");
    }

    #[test]
    fn publishing_is_gated_and_paced() {
        // It writes "this sc-ID is at this address" into a public DHT. Off
        // means off, and on does not mean continuously.
        let mut s = Schedule::new();
        assert!(!s.tick(0, &[], false).publish, "published while discovery was off");
        assert!(s.tick(1_000, &[], true).publish, "the first tick did not publish");
        assert!(!s.tick(1_001, &[], true).publish, "republished immediately");
        assert!(s.tick(1_000 + PUBLISH_EVERY_MS, &[], true).publish,
                "never republished, so our record goes stale and we vanish");
    }

    #[test]
    fn an_empty_answer_is_not_retried_sooner_than_a_found_one() {
        // "Not in the DHT" is the ordinary state of an offline contact.
        // Retrying it faster spends the most traffic on the peers least
        // likely to appear.
        let mut s = Schedule::new();
        let c = vec!["sc-00000001".to_string()];
        let id = s.tick(0, &c, false).resolve.unwrap();
        s.finished(&id, false);   // empty result: same call as a found one
        assert_eq!(s.tick(LOOKUP_COOLDOWN_MS - 1, &c, false).resolve, None);
        assert_eq!(s.tick(LOOKUP_COOLDOWN_MS, &c, false).resolve.as_deref(),
                   Some("sc-00000001"));
    }

    #[test]
    fn a_node_that_finds_nothing_at_all_eventually_says_so() {
        // A DHT that finds nobody looks exactly like a DHT whose contacts
        // are all offline. One is a quiet afternoon; the other is a node
        // that cannot speak to the network it bootstraps against, and
        // nothing in the DHT reports the difference.
        let mut s = Schedule::new();
        let c = ids(8);
        let mut t = 0;
        for _ in 0..SILENT_AFTER {
            t += LOOKUP_COOLDOWN_MS;
            let id = s.tick(t, &c, false).resolve.expect("rotation stalled");
            assert!(!s.silent(), "called silent before enough evidence");
            s.finished(&id, false);
        }
        assert!(s.silent(), "three empty lookups and still no word");
        assert!(!s.silent(), "the warning repeats every lookup thereafter");
    }

    #[test]
    fn one_successful_lookup_means_the_node_is_not_silent() {
        // The point of the counter is to distinguish "nobody is online"
        // from "we are not on the network". Finding anybody settles it.
        let mut s = Schedule::new();
        let c = ids(8);
        let mut t = 0;
        for i in 0..SILENT_AFTER {
            t += LOOKUP_COOLDOWN_MS;
            let id = s.tick(t, &c, false).resolve.expect("rotation stalled");
            s.finished(&id, i == 0);
        }
        assert!(!s.silent(), "a node that found somebody reported itself unreachable");
    }

    #[test]
    fn a_reset_does_not_leave_a_dead_lookup_holding_the_slot() {
        // The node going away takes its in-flight lookup with it. Without
        // the reset the next node sits out a timeout for a socket that no
        // longer exists.
        let mut s = Schedule::new();
        let c = ids(2);
        s.tick(0, &c, false);
        assert!(s.in_flight().is_some());
        s.reset();
        assert!(s.in_flight().is_none());
        assert!(s.tick(1, &c, false).resolve.is_some());
    }
}
