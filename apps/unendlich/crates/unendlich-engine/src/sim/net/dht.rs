//! `dht` — Kademlia-style routing table + channel definitions for peer
//! discovery/routing (r500). The current LAN path needs a known host IP; a DHT
//! lets peers find each other by `NodeId` XOR-distance with no fixed host. This
//! module is the pure, tested core: NodeId distance, k-buckets, and closest-node
//! lookup. The UDP RPC transport (ping / find_node / bootstrap) and the bin
//! integration layer on top of this — they're the on-device step.
//!
//! IDs are `u64` (64-bit keyspace) — plenty for a game's peer set and far easier
//! to reason about/test than 160-bit. Distance is XOR; bucket `i` holds contacts
//! whose distance's highest set bit is bit `i`.

/// A node identifier in the DHT keyspace.
pub type NodeId = u64;

/// Max contacts kept per k-bucket (Kademlia's `k`).
pub const K: usize = 8;

/// XOR distance between two ids (the Kademlia metric: symmetric, unidirectional).
#[inline]
pub fn distance(a: NodeId, b: NodeId) -> u64 {
    a ^ b
}

/// A routable peer: its DHT id plus how to reach it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Contact {
    pub id: NodeId,
    pub addr_v4: [u8; 4],
    pub port: u16,
}

impl Contact {
    pub fn new(id: NodeId, addr_v4: [u8; 4], port: u16) -> Self {
        Contact { id, addr_v4, port }
    }
}

/// A Kademlia routing table: 64 k-buckets keyed by XOR-distance bit, each
/// holding up to [`K`] contacts in least- to most-recently-seen order.
pub struct RoutingTable {
    self_id: NodeId,
    buckets: Vec<Vec<Contact>>, // 64 buckets
}

impl RoutingTable {
    pub fn new(self_id: NodeId) -> Self {
        RoutingTable { self_id, buckets: (0..64).map(|_| Vec::new()).collect() }
    }

    pub fn self_id(&self) -> NodeId {
        self.self_id
    }

    /// Bucket index for `id` = position of the highest set bit of the distance.
    /// `None` for our own id (distance 0, not stored).
    fn bucket_index(&self, id: NodeId) -> Option<usize> {
        let d = distance(self.self_id, id);
        if d == 0 {
            return None;
        }
        Some(63 - d.leading_zeros() as usize)
    }

    /// Insert or refresh a contact. A seen contact moves to most-recently-seen
    /// (bucket tail). A new contact is stored if the bucket has room; if full it
    /// is dropped (full Kademlia would ping the least-recently-seen and evict it
    /// only if dead — kept simple here, a transport-layer refinement). Returns
    /// whether the contact is now present.
    pub fn insert(&mut self, c: Contact) -> bool {
        let Some(bi) = self.bucket_index(c.id) else {
            return false;
        };
        let b = &mut self.buckets[bi];
        if let Some(pos) = b.iter().position(|x| x.id == c.id) {
            let existing = b.remove(pos);
            b.push(existing); // most-recently-seen at the tail
            return true;
        }
        if b.len() < K {
            b.push(c);
            true
        } else {
            false
        }
    }

    /// Remove a contact (e.g. on a failed send).
    pub fn remove(&mut self, id: NodeId) {
        if let Some(bi) = self.bucket_index(id) {
            self.buckets[bi].retain(|c| c.id != id);
        }
    }

    /// The up-to-`n` contacts closest (XOR) to `target`. This is the core lookup
    /// step: route a message toward the peers nearest its key.
    pub fn closest(&self, target: NodeId, n: usize) -> Vec<Contact> {
        let mut all: Vec<Contact> = self.buckets.iter().flatten().copied().collect();
        all.sort_by_key(|c| distance(c.id, target));
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

/// Wire channels (formerly bare `const CH_* : u8` in the bin). Adds a `Dht`
/// channel for routing-table RPCs alongside the existing game channels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Channel {
    Handshake = 1,
    Entity = 2,
    WorldCmd = 3,
    Avatar = 4,
    Dht = 5,
}

impl Channel {
    pub fn as_u8(self) -> u8 {
        self as u8
    }
    pub fn from_u8(b: u8) -> Option<Channel> {
        Some(match b {
            1 => Channel::Handshake,
            2 => Channel::Entity,
            3 => Channel::WorldCmd,
            4 => Channel::Avatar,
            5 => Channel::Dht,
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(id: NodeId) -> Contact {
        Contact::new(id, [127, 0, 0, 1], 4000)
    }

    #[test]
    fn distance_is_symmetric_and_zero_to_self() {
        assert_eq!(distance(5, 9), distance(9, 5));
        assert_eq!(distance(42, 42), 0);
    }

    #[test]
    fn insert_dedups_and_refreshes() {
        let mut t = RoutingTable::new(0);
        assert!(t.insert(c(0b1010)));
        assert!(t.insert(c(0b1010))); // same id → refresh, still 1
        assert_eq!(t.len(), 1);
        assert!(!t.insert(c(0))); // our own id is not stored
        assert_eq!(t.len(), 1);
    }

    #[test]
    fn bucket_caps_at_k() {
        let mut t = RoutingTable::new(0);
        // ids 1<<63 .. all share the top bit → same bucket (index 63)
        for i in 0..(K as u64 + 4) {
            t.insert(c((1u64 << 63) | i));
        }
        // bucket 63 holds at most K
        assert!(t.len() <= K, "bucket capped at K, got {}", t.len());
    }

    #[test]
    fn closest_returns_nearest_by_xor() {
        let mut t = RoutingTable::new(0);
        for id in [0b0001u64, 0b0010, 0b0100, 0b1000, 0b1100, 0b1111] {
            t.insert(c(id));
        }
        let target = 0b0011u64;
        let near = t.closest(target, 3);
        assert_eq!(near.len(), 3);
        // nearest to 0b0011 should include 0b0001 (d=2) and 0b0010 (d=1)
        let ids: Vec<NodeId> = near.iter().map(|x| x.id).collect();
        assert!(ids.contains(&0b0010), "0b0010 is distance 1 from target");
        assert!(ids.contains(&0b0001), "0b0001 is distance 2 from target");
        // results are sorted by distance ascending
        for w in near.windows(2) {
            assert!(distance(w[0].id, target) <= distance(w[1].id, target));
        }
    }

    #[test]
    fn channel_round_trips() {
        for ch in [
            Channel::Handshake, Channel::Entity, Channel::WorldCmd,
            Channel::Avatar, Channel::Dht,
        ] {
            assert_eq!(Channel::from_u8(ch.as_u8()), Some(ch));
        }
        assert_eq!(Channel::from_u8(99), None);
    }

    #[test]
    fn remove_drops_contact() {
        let mut t = RoutingTable::new(0);
        t.insert(c(0b101));
        assert_eq!(t.len(), 1);
        t.remove(0b101);
        assert_eq!(t.len(), 0);
    }
}
