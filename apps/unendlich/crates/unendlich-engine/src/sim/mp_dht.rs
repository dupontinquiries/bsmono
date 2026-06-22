//! `sim::mp_dht` — multiplayer routing model: Kademlia-style XOR routing, bounded
//! mailboxes, and the same-seed region-overlap link rule (gated by a
//! multiplayer-search toggle).
//!
//! Pure model only — the bin maps `NodeId`→socket and ferries `Mail` over the
//! wire. No sockets, no threads, no clock (logical time `t` is supplied).
//! Deterministic, panic-free.

use std::collections::HashMap;
use std::collections::VecDeque;

pub type NodeId = u64;

/// Kademlia XOR metric.
pub fn xor_distance(a: NodeId, b: NodeId) -> u64 {
    a ^ b
}

pub const K: usize = 8;

/// Bucket index for a distance: the position of the most-significant differing
/// bit (0..63). Distance 0 has no bucket.
fn bucket_of(dist: u64) -> Option<usize> {
    if dist == 0 {
        None
    } else {
        Some(63 - dist.leading_zeros() as usize)
    }
}

pub struct RoutingTable {
    local: NodeId,
    buckets: Vec<Vec<NodeId>>, // 64 buckets, LRU: oldest at front
}

impl RoutingTable {
    pub fn new(local: NodeId) -> Self {
        RoutingTable { local, buckets: vec![Vec::new(); 64] }
    }

    /// Insert/refresh a peer. Drops self, dedups, keeps ≤ K per bucket (LRU evicts
    /// the oldest when full).
    pub fn observe(&mut self, peer: NodeId) {
        if peer == self.local {
            return;
        }
        let b = match bucket_of(xor_distance(self.local, peer)) {
            Some(b) => b,
            None => return,
        };
        let bucket = &mut self.buckets[b];
        if let Some(pos) = bucket.iter().position(|&n| n == peer) {
            // refresh: move to back (most recent)
            bucket.remove(pos);
            bucket.push(peer);
        } else {
            if bucket.len() >= K {
                bucket.remove(0); // evict oldest
            }
            bucket.push(peer);
        }
    }

    /// The `n` known peers nearest `target` by XOR distance (ascending, ties
    /// broken by id for determinism).
    pub fn closest(&self, target: NodeId, n: usize) -> Vec<NodeId> {
        let mut all: Vec<NodeId> = self.buckets.iter().flatten().copied().collect();
        all.sort_by(|&a, &b| {
            xor_distance(a, target)
                .cmp(&xor_distance(b, target))
                .then(a.cmp(&b))
        });
        all.truncate(n);
        all
    }

    pub fn len(&self) -> usize {
        self.buckets.iter().map(|b| b.len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mail {
    pub to: NodeId,
    pub from: NodeId,
    pub topic: u16,
    pub body: Vec<u8>,
    pub t: u64,
}

/// Per-recipient FIFO mailboxes, each bounded. A full box rejects new mail
/// (`put` returns false) so senders back off until the recipient fetches.
pub struct Mailboxes {
    max_per_box: usize,
    boxes: HashMap<NodeId, VecDeque<Mail>>,
}

impl Mailboxes {
    pub fn new(max_per_box: usize) -> Self {
        Mailboxes { max_per_box: max_per_box.max(1), boxes: HashMap::new() }
    }

    /// Store mail in the recipient's box. Returns false if the box is full.
    pub fn put(&mut self, m: Mail) -> bool {
        let q = self.boxes.entry(m.to).or_default();
        if q.len() >= self.max_per_box {
            return false;
        }
        q.push_back(m);
        true
    }

    /// Drain a recipient's box in arrival order.
    pub fn fetch(&mut self, who: NodeId) -> Vec<Mail> {
        match self.boxes.get_mut(&who) {
            Some(q) => q.drain(..).collect(),
            None => Vec::new(),
        }
    }

    pub fn pending(&self, who: NodeId) -> usize {
        self.boxes.get(&who).map(|q| q.len()).unwrap_or(0)
    }
}

/// An open world section: a chunk-space AABB on a given world seed.
#[derive(Clone, Copy, Debug)]
pub struct Region {
    pub seed: u64,
    pub min: [i32; 3],
    pub max: [i32; 3],
}

/// True iff the two regions are on the SAME seed and their AABBs overlap.
pub fn regions_overlap(a: &Region, b: &Region) -> bool {
    if a.seed != b.seed {
        return false;
    }
    for k in 0..3 {
        if a.min[k] > b.max[k] || a.max[k] < b.min[k] {
            return false;
        }
    }
    true
}

/// Two peers should be linked iff multiplayer search is on and their regions
/// overlap on the same seed.
pub fn should_link(a: &Region, b: &Region, search_enabled: bool) -> bool {
    search_enabled && regions_overlap(a, b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xor_symmetric_self_zero() {
        assert_eq!(xor_distance(5, 5), 0);
        assert_eq!(xor_distance(3, 9), xor_distance(9, 3));
        assert!(xor_distance(1, 2) > 0);
    }

    #[test]
    fn observe_drops_self_dedups_caps_lru() {
        let mut rt = RoutingTable::new(0);
        rt.observe(0); // self -> ignored
        assert_eq!(rt.len(), 0);
        // many peers in the same high bucket (all share top bit pattern vs 0):
        for p in 1u64..=20 {
            rt.observe(p);
        }
        // dedup
        rt.observe(1);
        // each bucket capped at K
        for b in &rt.buckets {
            assert!(b.len() <= K, "bucket over K");
        }
        // observe a fresh peer into a full bucket evicts the oldest in that bucket
        let mut rt2 = RoutingTable::new(0);
        // ids 1<<40 .. share bucket 40
        let base = 1u64 << 40;
        for i in 0..K as u64 {
            rt2.observe(base + i);
        }
        let oldest = base; // first inserted
        rt2.observe(base + 100); // overflow -> evict oldest
        let in_table: Vec<NodeId> = rt2.buckets.iter().flatten().copied().collect();
        assert!(!in_table.contains(&oldest), "oldest evicted");
        assert!(in_table.contains(&(base + 100)), "newest kept");
    }

    #[test]
    fn closest_matches_naive() {
        let mut rt = RoutingTable::new(0xFFFF);
        let peers = [1u64, 2, 7, 100, 12345, 9999, 42, 256];
        for &p in &peers {
            rt.observe(p);
        }
        let target = 64u64;
        let got = rt.closest(target, 3);
        let mut naive: Vec<NodeId> = peers.to_vec();
        naive.sort_by(|&a, &b| xor_distance(a, target).cmp(&xor_distance(b, target)).then(a.cmp(&b)));
        assert_eq!(got, naive[..3].to_vec());
    }

    #[test]
    fn mailbox_fifo_bound_and_drain() {
        let mut mb = Mailboxes::new(2);
        let m = |i: u8| Mail { to: 9, from: 1, topic: 0, body: vec![i], t: i as u64 };
        assert!(mb.put(m(1)));
        assert!(mb.put(m(2)));
        assert!(!mb.put(m(3)), "full box rejects");
        assert_eq!(mb.pending(9), 2);
        let got = mb.fetch(9);
        assert_eq!(got.iter().map(|m| m.body[0]).collect::<Vec<_>>(), vec![1, 2], "FIFO");
        assert_eq!(mb.pending(9), 0);
        assert!(mb.fetch(123).is_empty(), "empty box ok");
    }

    #[test]
    fn region_overlap_truth() {
        let a = Region { seed: 1, min: [0, 0, 0], max: [4, 4, 4] };
        let b = Region { seed: 1, min: [3, 3, 3], max: [8, 8, 8] };
        let c = Region { seed: 1, min: [10, 10, 10], max: [12, 12, 12] };
        let d = Region { seed: 2, min: [0, 0, 0], max: [4, 4, 4] };
        assert!(regions_overlap(&a, &b), "overlapping");
        assert!(!regions_overlap(&a, &c), "disjoint");
        assert!(!regions_overlap(&a, &d), "different seed never overlaps");
        // touching at a face counts as overlap
        let t = Region { seed: 1, min: [4, 0, 0], max: [6, 4, 4] };
        assert!(regions_overlap(&a, &t));
    }

    #[test]
    fn should_link_truth_table() {
        let a = Region { seed: 1, min: [0, 0, 0], max: [4, 4, 4] };
        let b = Region { seed: 1, min: [3, 3, 3], max: [8, 8, 8] };
        let diff = Region { seed: 2, min: [0, 0, 0], max: [4, 4, 4] };
        assert!(!should_link(&a, &b, false), "toggle off => never");
        assert!(should_link(&a, &b, true), "on + overlap + same seed => yes");
        assert!(!should_link(&a, &diff, true), "different seed => never");
    }

    #[test]
    fn deterministic() {
        let mut r1 = RoutingTable::new(7);
        let mut r2 = RoutingTable::new(7);
        for p in [3u64, 100, 42, 9] {
            r1.observe(p);
            r2.observe(p);
        }
        assert_eq!(r1.closest(50, 4), r2.closest(50, 4));
    }
}
