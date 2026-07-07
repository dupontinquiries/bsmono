//! microdht — a Kademlia-lite DHT for the fallback path: when OpenDHT is unreachable, profiles are
//! still discoverable on the LAN. This module is the **pure routing core** — 256-bit ids, XOR
//! distance, k-buckets, closest-node selection, and a local key→value store. The iterative network
//! lookup rides on top (I/O), but the math that decides *who is closest* lives here and is tested.

/// A 256-bit node/key id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId(pub [u8; 32]);

impl NodeId {
    /// Derive an id from an arbitrary key (a player id, a game session) with a dependency-free
    /// avalanche hash (FNV-1a mixed across 32 bytes). Good enough for LAN routing; the global path
    /// uses OpenDHT's own hashing.
    pub fn from_key(key: &str) -> NodeId {
        let mut out = [0u8; 32];
        let bytes = key.as_bytes();
        for (i, slot) in out.iter_mut().enumerate() {
            let mut h: u64 = 0xcbf29ce484222325 ^ (i as u64).wrapping_mul(0x100000001b3);
            for &b in bytes {
                h ^= b as u64;
                h = h.wrapping_mul(0x100000001b3);
                h = h.rotate_left(5);
            }
            *slot = (h ^ (h >> 32)) as u8;
        }
        NodeId(out)
    }

    /// XOR distance to another id — the Kademlia metric (symmetric; d(a,a)=0).
    pub fn distance(&self, other: &NodeId) -> [u8; 32] {
        let mut d = [0u8; 32];
        for i in 0..32 {
            d[i] = self.0[i] ^ other.0[i];
        }
        d
    }

    /// Which k-bucket a peer falls in: the standard Kademlia index = the position of the highest
    /// set bit of the XOR distance (from the LSB). A peer differing only in the lowest bit is the
    /// closest → bucket 0; differing in the top bit is farthest → bucket 255.
    pub fn bucket_index(&self, other: &NodeId) -> Option<usize> {
        let d = self.distance(other);
        for (i, byte) in d.iter().enumerate() {
            if *byte != 0 {
                let bit_in_byte = byte.leading_zeros() as usize; // 0..7
                let leading = i * 8 + bit_in_byte;
                return Some(255 - leading);
            }
        }
        None // same id
    }
}

/// Compare two distances (big-endian): is `a` closer than `b` to the target?
fn dist_lt(a: &[u8; 32], b: &[u8; 32]) -> bool {
    for i in 0..32 {
        if a[i] != b[i] {
            return a[i] < b[i];
        }
    }
    false
}

/// A k-bucket routing table. `K` peers per bucket (Kademlia replication parameter).
pub struct RoutingTable {
    pub me: NodeId,
    k: usize,
    buckets: Vec<Vec<NodeId>>, // 256 buckets
}

impl RoutingTable {
    pub fn new(me: NodeId, k: usize) -> RoutingTable {
        RoutingTable { me, k, buckets: (0..256).map(|_| Vec::new()).collect() }
    }

    /// Learn about a peer. Bounded per bucket (keeps the freshest `k`); ignores our own id.
    pub fn add(&mut self, peer: NodeId) {
        let Some(idx) = self.me.bucket_index(&peer) else {
            return;
        };
        let b = &mut self.buckets[idx];
        if let Some(pos) = b.iter().position(|p| *p == peer) {
            b.remove(pos); // move-to-end = most-recently-seen
        }
        b.push(peer);
        if b.len() > self.k {
            b.remove(0); // evict least-recently-seen
        }
    }

    pub fn len(&self) -> usize {
        self.buckets.iter().map(|b| b.len()).sum()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The `count` known peers closest (by XOR) to `target`.
    pub fn closest(&self, target: &NodeId, count: usize) -> Vec<NodeId> {
        let mut all: Vec<NodeId> = self.buckets.iter().flatten().copied().collect();
        all.sort_by(|a, b| {
            let (da, db) = (a.distance(target), b.distance(target));
            if dist_lt(&da, &db) {
                std::cmp::Ordering::Less
            } else if dist_lt(&db, &da) {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        });
        all.truncate(count);
        all
    }
}

/// A micro-DHT node: routing table + a local key→value store.
pub struct MicroDht {
    pub table: RoutingTable,
    store: std::collections::HashMap<NodeId, Vec<u8>>,
}

impl MicroDht {
    pub fn new(me: NodeId, k: usize) -> MicroDht {
        MicroDht { table: RoutingTable::new(me, k), store: std::collections::HashMap::new() }
    }
    pub fn put(&mut self, key: &str, value: Vec<u8>) {
        self.store.insert(NodeId::from_key(key), value);
    }
    pub fn get(&self, key: &str) -> Option<&Vec<u8>> {
        self.store.get(&NodeId::from_key(key))
    }
    /// Peers to ask next for a key we don't hold (the lookup frontier).
    pub fn closest_to_key(&self, key: &str, count: usize) -> Vec<NodeId> {
        self.table.closest(&NodeId::from_key(key), count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distance_is_symmetric_and_zero_to_self() {
        let a = NodeId::from_key("alice");
        let b = NodeId::from_key("bob");
        assert_eq!(a.distance(&b), b.distance(&a));
        assert_eq!(a.distance(&a), [0u8; 32]);
        assert!(a != b, "distinct keys → distinct ids");
    }

    #[test]
    fn closest_orders_by_xor_distance() {
        let me = NodeId::from_key("me");
        let mut t = RoutingTable::new(me, 20);
        let peers: Vec<NodeId> = (0..50).map(|i| NodeId::from_key(&format!("peer{i}"))).collect();
        for p in &peers {
            t.add(*p);
        }
        let target = NodeId::from_key("peer7");
        let closest = t.closest(&target, 5);
        assert_eq!(closest.len(), 5);
        // peer7 itself is distance 0 → must be first
        assert_eq!(closest[0], target);
        // returned list is monotonically non-decreasing in distance
        for w in closest.windows(2) {
            assert!(!dist_lt(&w[1].distance(&target), &w[0].distance(&target)), "sorted by distance");
        }
    }

    #[test]
    fn bucket_index_shrinks_with_distance() {
        let me = NodeId([0u8; 32]);
        let mut near = [0u8; 32];
        near[31] = 1; // differs only in the lowest bit → smallest distance → bucket 0
        let mut far = [0u8; 32];
        far[0] = 0x80; // differs in the top bit → largest distance → bucket 255
        assert_eq!(me.bucket_index(&NodeId(near)), Some(0));
        assert_eq!(me.bucket_index(&NodeId(far)), Some(255));
        assert_eq!(me.bucket_index(&me), None);
    }

    #[test]
    fn buckets_bounded_by_k_and_store_round_trips() {
        let me = NodeId::from_key("me");
        let mut d = MicroDht::new(me, 4);
        // hammer one region; bucket stays bounded at k
        for i in 0..100 {
            d.table.add(NodeId::from_key(&format!("n{i}")));
        }
        assert!(d.table.buckets.iter().all(|b| b.len() <= 4));
        d.put("kesslerd@a1", b"profile-bytes".to_vec());
        assert_eq!(d.get("kesslerd@a1").map(|v| v.as_slice()), Some(&b"profile-bytes"[..]));
        assert!(d.get("nobody").is_none());
    }
}
