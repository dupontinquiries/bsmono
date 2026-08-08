//! routing.rs — the Kademlia routing table (the "map").
//!
//! Textbook k-buckets + XOR metric, hardened with the two refinements OpenDHT's
//! `routing_table.h` shows are necessary in practice:
//!   * a per-bucket **replacement cache** (OpenDHT's `cached` candidate), so a
//!     full bucket doesn't drop newly-learned good nodes on the floor, and
//!   * node **liveness** (good / questionable / bad), so we evict dead entries
//!     in favour of waiting replacements rather than keeping stale ones.
//!
//! Deliberately NOT here (they layer on top): the cluster/affinity overlay and
//! the ID-windowed maintenance scheduler. This file is the invariant core.
//!
//! All time is a caller-supplied logical `u64` tick (monotonic; milliseconds in
//! production), so every path is deterministic and unit-testable.

use std::net::SocketAddr;

/// 160-bit ids, matching OpenDHT's InfoHash width.
pub const ID_LEN: usize = 20;
/// Kademlia bucket capacity `k`.
pub const K: usize = 20;
/// One bucket per bit of shared prefix.
pub const N_BUCKETS: usize = ID_LEN * 8;

/// A node identifier in the 160-bit Kademlia keyspace.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct NodeId(pub [u8; ID_LEN]);

impl NodeId {
    /// XOR distance to `other` (the Kademlia metric). Returned big-endian, so the
    /// array's natural `Ord` *is* numeric-distance order and sorts directly.
    pub fn distance(&self, other: &NodeId) -> [u8; ID_LEN] {
        let mut d = [0u8; ID_LEN];
        for i in 0..ID_LEN {
            d[i] = self.0[i] ^ other.0[i];
        }
        d
    }

    /// Which bucket `other` belongs in relative to `self`: the count of leading
    /// zero bits in the XOR distance. `0` = differs in the top bit (farthest);
    /// `N_BUCKETS - 1` = closest. `None` iff the ids are equal.
    pub fn bucket_index(&self, other: &NodeId) -> Option<usize> {
        let d = self.distance(other);
        for (i, byte) in d.iter().enumerate() {
            if *byte != 0 {
                return Some(i * 8 + byte.leading_zeros() as usize);
            }
        }
        None
    }
}

/// Kademlia liveness classification of a known contact.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Liveness {
    Good,
    Questionable,
    Bad,
}

/// A known node: its id, address, when we last had positive contact, and how many
/// probes have failed in a row since.
#[derive(Clone, Debug)]
pub struct Contact {
    pub id: NodeId,
    pub addr: SocketAddr,
    pub last_seen: u64,
    pub failed: u32,
}

impl Contact {
    pub fn new(id: NodeId, addr: SocketAddr, now: u64) -> Self {
        Self { id, addr, last_seen: now, failed: 0 }
    }

    pub fn liveness(&self, now: u64, p: &Params) -> Liveness {
        if self.failed >= p.bad_after_failures {
            Liveness::Bad
        } else if now.saturating_sub(self.last_seen) >= p.questionable_after {
            Liveness::Questionable
        } else {
            Liveness::Good
        }
    }
}

/// Routing-table tunables. `questionable_after` is in the same logical ticks as
/// the `now` passed to the table.
#[derive(Clone, Copy, Debug)]
pub struct Params {
    pub k: usize,
    pub questionable_after: u64,
    pub bad_after_failures: u32,
}

impl Default for Params {
    fn default() -> Self {
        // 15 min in ms, 3 strikes — sane real-world defaults; tests override.
        Self { k: K, questionable_after: 15 * 60_000, bad_after_failures: 3 }
    }
}

/// A single k-bucket: up to `k` live contacts (front = least-recently-seen, the
/// eviction candidate) plus a bounded replacement cache.
#[derive(Clone, Debug, Default)]
pub struct Bucket {
    contacts: Vec<Contact>,
    replacements: Vec<Contact>,
}

impl Bucket {
    fn position(&self, id: &NodeId) -> Option<usize> {
        self.contacts.iter().position(|c| &c.id == id)
    }

    /// Record a positive interaction with `contact`. The Kademlia update rule,
    /// with a replacement cache:
    ///  - already present -> refresh (last_seen, clear failures, update addr),
    ///    move to the back (most-recently-seen);
    ///  - room -> append;
    ///  - full -> if the front (LRU) contact is `Bad`, evict it and append;
    ///    otherwise stash the newcomer in the bounded replacement cache.
    pub fn saw(&mut self, contact: Contact, now: u64, p: &Params) {
        if let Some(i) = self.position(&contact.id) {
            let mut c = self.contacts.remove(i);
            c.last_seen = now;
            c.failed = 0;
            c.addr = contact.addr;
            self.contacts.push(c);
            return;
        }
        if self.contacts.len() < p.k {
            self.contacts.push(contact);
            return;
        }
        let front_bad = self
            .contacts
            .first()
            .map(|c| c.liveness(now, p) == Liveness::Bad)
            .unwrap_or(false);
        if front_bad {
            self.contacts.remove(0);
            self.contacts.push(contact);
        } else if !self.replacements.iter().any(|c| c.id == contact.id) {
            if self.replacements.len() >= p.k {
                self.replacements.remove(0);
            }
            self.replacements.push(contact);
        }
    }

    /// A probe to `id` failed. Bump its strike count; if that tips it to `Bad`
    /// and a replacement is waiting, evict it and promote the replacement.
    pub fn failed(&mut self, id: &NodeId, now: u64, p: &Params) {
        if let Some(i) = self.position(id) {
            self.contacts[i].failed += 1;
            if self.contacts[i].liveness(now, p) == Liveness::Bad {
                if let Some(repl) = self.replacements.pop() {
                    self.contacts.remove(i);
                    self.contacts.push(repl);
                }
            }
        }
    }

    pub fn live_contacts(&self) -> &[Contact] {
        &self.contacts
    }

    pub fn replacement_len(&self) -> usize {
        self.replacements.len()
    }
}

/// The routing table: the local id plus one bucket per bit of shared prefix.
#[derive(Clone, Debug)]
pub struct RoutingTable {
    local: NodeId,
    buckets: Vec<Bucket>,
    params: Params,
}

impl RoutingTable {
    pub fn new(local: NodeId) -> Self {
        Self::with_params(local, Params::default())
    }

    pub fn with_params(local: NodeId, params: Params) -> Self {
        Self { local, buckets: vec![Bucket::default(); N_BUCKETS], params }
    }

    pub fn local(&self) -> &NodeId {
        &self.local
    }

    pub fn params(&self) -> &Params {
        &self.params
    }

    /// Record a positive interaction with a node. Our own id is ignored.
    pub fn saw(&mut self, id: NodeId, addr: SocketAddr, now: u64) {
        if let Some(b) = self.local.bucket_index(&id) {
            self.buckets[b].saw(Contact::new(id, addr, now), now, &self.params);
        }
    }

    /// Record a failed probe to a node.
    pub fn failed(&mut self, id: &NodeId, now: u64) {
        if let Some(b) = self.local.bucket_index(id) {
            self.buckets[b].failed(id, now, &self.params);
        }
    }

    /// The `count` known contacts closest to `target` by XOR distance — the core
    /// query for iterative lookup and for answering an inbound FIND_NODE.
    pub fn closest(&self, target: &NodeId, count: usize) -> Vec<Contact> {
        let mut all: Vec<Contact> = self
            .buckets
            .iter()
            .flat_map(|b| b.live_contacts().iter().cloned())
            .collect();
        all.sort_by(|a, c| a.id.distance(target).cmp(&c.id.distance(target)));
        all.truncate(count);
        all
    }

    /// Bucket index `id` would occupy, exposed for the scheduler (ID-windowed
    /// maintenance) and the cluster overlay.
    pub fn bucket_of(&self, id: &NodeId) -> Option<usize> {
        self.local.bucket_index(id)
    }

    pub fn len(&self) -> usize {
        self.buckets.iter().map(|b| b.live_contacts().len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(bytes: [u8; ID_LEN]) -> NodeId {
        NodeId(bytes)
    }

    /// id that is all zeros except a single set bit at absolute bit position `pos`
    /// (0 = most-significant bit of byte 0).
    fn id_bit(pos: usize) -> NodeId {
        let mut b = [0u8; ID_LEN];
        b[pos / 8] = 0x80 >> (pos % 8);
        NodeId(b)
    }

    fn addr(n: u16) -> SocketAddr {
        format!("127.0.0.1:{n}").parse().unwrap()
    }

    fn fast_params() -> Params {
        // questionable after 100 ticks, bad after 2 strikes — easy to drive.
        Params { k: 4, questionable_after: 100, bad_after_failures: 2 }
    }

    #[test]
    fn distance_is_zero_to_self_and_symmetric() {
        let a = id([0x12; ID_LEN]);
        let b = id([0xAB; ID_LEN]);
        assert_eq!(a.distance(&a), [0u8; ID_LEN]);
        assert_eq!(a.distance(&b), b.distance(&a));
    }

    #[test]
    fn bucket_index_tracks_highest_differing_bit() {
        let zero = id([0u8; ID_LEN]);
        assert_eq!(zero.bucket_index(&zero), None); // identical
        assert_eq!(zero.bucket_index(&id_bit(0)), Some(0)); // top bit differs
        assert_eq!(zero.bucket_index(&id_bit(159)), Some(159)); // last bit differs
        assert_eq!(zero.bucket_index(&id_bit(8)), Some(8)); // top bit of byte 1
    }

    #[test]
    fn closest_returns_k_nearest_in_distance_order() {
        let local = id([0u8; ID_LEN]);
        let mut rt = RoutingTable::with_params(local, fast_params());
        // Insert ids at increasing distance from `local` (single bit set, lower
        // bit position = closer because the leading bits stay zero).
        for pos in [3usize, 10, 20, 40, 80, 150] {
            rt.saw(id_bit(pos), addr(pos as u16 + 1), 0);
        }
        let target = id([0u8; ID_LEN]); // closest to local-space origin
        let got = rt.closest(&target, 3);
        assert_eq!(got.len(), 3);
        // Must be sorted by XOR distance to target.
        let mut prev = [0u8; ID_LEN];
        for (i, c) in got.iter().enumerate() {
            let d = c.id.distance(&target);
            if i > 0 {
                assert!(prev <= d, "closest not sorted by distance");
            }
            prev = d;
        }
        // The nearest should be the lowest-distance bit we inserted (pos 150 ->
        // smallest XOR to all-zero target since only its lowest bit is set... but
        // any single-bit id's distance to zero is the bit's value; pos 150 sets a
        // low bit => smallest). Confirm the top result is bit 150.
        assert_eq!(got[0].id, id_bit(150));
    }

    #[test]
    fn closest_returns_fewer_than_count_when_sparse() {
        let mut rt = RoutingTable::with_params(id([0u8; ID_LEN]), fast_params());
        rt.saw(id_bit(5), addr(1), 0);
        rt.saw(id_bit(6), addr(2), 0);
        assert_eq!(rt.closest(&id([0u8; ID_LEN]), 8).len(), 2);
    }

    #[test]
    fn saw_dedups_refreshes_and_moves_to_back() {
        let mut bucket = Bucket::default();
        let p = fast_params();
        let a = id_bit(10);
        let b = id_bit(11);
        bucket.saw(Contact::new(a, addr(1), 0), 0, &p);
        bucket.saw(Contact::new(b, addr(2), 0), 0, &p);
        // Re-see `a` later with a new addr: should refresh, not duplicate, and
        // move it to the back (most-recently-seen).
        bucket.saw(Contact::new(a, addr(99), 50), 50, &p);
        let live = bucket.live_contacts();
        assert_eq!(live.len(), 2, "no duplicate");
        assert_eq!(live.last().unwrap().id, a, "refreshed contact moved to back");
        assert_eq!(live.last().unwrap().addr, addr(99), "addr updated");
        assert_eq!(live.last().unwrap().last_seen, 50, "last_seen refreshed");
    }

    #[test]
    fn full_bucket_with_good_front_caches_replacement() {
        let mut bucket = Bucket::default();
        let p = fast_params(); // k = 4
        for i in 0..4u8 {
            bucket.saw(Contact::new(id_bit(10 + i as usize), addr(i as u16), 10), 10, &p);
        }
        assert_eq!(bucket.live_contacts().len(), 4);
        // Front contact is Good (seen at t=10, now t=20 < questionable_after=100):
        // newcomer goes to the replacement cache, live set unchanged.
        bucket.saw(Contact::new(id_bit(50), addr(50), 20), 20, &p);
        assert_eq!(bucket.live_contacts().len(), 4, "live set unchanged");
        assert_eq!(bucket.replacement_len(), 1, "newcomer cached");
    }

    #[test]
    fn full_bucket_with_bad_front_evicts_and_admits() {
        let mut bucket = Bucket::default();
        let p = fast_params(); // k = 4, questionable_after = 100
        // Front contact seen long ago so it's stale; others fresh.
        bucket.saw(Contact::new(id_bit(10), addr(0), 0), 0, &p);
        for i in 1..4u8 {
            bucket.saw(Contact::new(id_bit(10 + i as usize), addr(i as u16), 500), 500, &p);
        }
        // Drive the front contact to Bad via failures (bad_after_failures = 2).
        bucket.failed(&id_bit(10), 500, &p);
        bucket.failed(&id_bit(10), 500, &p); // no replacement yet -> stays, now Bad
        // Now a newcomer arrives with the front Bad: it should be evicted+admitted.
        let newcomer = id_bit(60);
        bucket.saw(Contact::new(newcomer, addr(60), 500), 500, &p);
        let ids: Vec<NodeId> = bucket.live_contacts().iter().map(|c| c.id).collect();
        assert!(!ids.contains(&id_bit(10)), "bad front evicted");
        assert!(ids.contains(&newcomer), "newcomer admitted");
        assert_eq!(bucket.live_contacts().len(), 4);
    }

    #[test]
    fn failed_promotes_replacement_when_contact_goes_bad() {
        let mut bucket = Bucket::default();
        let p = fast_params(); // k = 4
        for i in 0..4u8 {
            bucket.saw(Contact::new(id_bit(10 + i as usize), addr(i as u16), 100), 100, &p);
        }
        // Cache a replacement (front is Good, so newcomer is cached).
        let repl = id_bit(70);
        bucket.saw(Contact::new(repl, addr(70), 110), 110, &p);
        assert_eq!(bucket.replacement_len(), 1);
        // Fail an existing contact until Bad; replacement should be promoted.
        let victim = id_bit(11);
        bucket.failed(&victim, 110, &p);
        bucket.failed(&victim, 110, &p); // 2 strikes -> Bad -> promote
        let ids: Vec<NodeId> = bucket.live_contacts().iter().map(|c| c.id).collect();
        assert!(!ids.contains(&victim), "bad contact evicted");
        assert!(ids.contains(&repl), "replacement promoted");
        assert_eq!(bucket.replacement_len(), 0, "replacement consumed");
    }

    #[test]
    fn table_ignores_self_insert() {
        let local = id([0x55; ID_LEN]);
        let mut rt = RoutingTable::new(local);
        rt.saw(local, addr(1), 0); // inserting our own id is a no-op
        assert!(rt.is_empty());
    }
}
