//! store.rs — the value layer beneath routing, lifting the proven semantics of
//! `snapclone-microdht`: a key -> multi-value map with per-value TTL, a
//! re-announce path that refreshes in place (dedups by bytes, never grows), and
//! a per-key cap.
//!
//! Keys live in the same 160-bit space as node ids — in Kademlia, keys and nodes
//! share the keyspace, so STORE and FIND_VALUE target the nodes closest to the
//! key. Mapping a logical name ("sc-…", "blob:<hash>", "group-feed:…") to a key
//! is a hash done one layer up; here a `Key` is already a point in the keyspace.

use crate::routing::NodeId;
use std::collections::HashMap;

/// A storage key: a point in the node-id keyspace.
pub type Key = NodeId;

#[derive(Clone, Debug)]
struct Stored {
    bytes: Vec<u8>,
    expires_at: u64,
}

#[derive(Clone, Copy, Debug)]
pub struct StoreParams {
    pub max_values_per_key: usize,
    /// Total distinct KEYS retained. The per-key cap bounds how much one key can
    /// hold; nothing bounded how many keys existed, so a client could exhaust a
    /// node's memory by PUTting one value under each of a million keys — the
    /// per-key limit never fires because every key holds exactly one value.
    ///
    /// This is the "storage cap" from the microdht lift list, and it is the
    /// half that was missing.
    pub max_keys: usize,
    /// Total bytes of value data retained. A node with 4 KB of RAM per key and
    /// a million keys is dead whatever the key count says, and a cap on COUNT
    /// alone is defeated by making each value larger.
    pub max_bytes: usize,
}

impl Default for StoreParams {
    fn default() -> Self {
        // Sized for a phone participating opportunistically rather than a
        // dedicated node: a few thousand rendezvous records is a busy mesh, and
        // 4 MB is small enough not to matter next to an app's own working set.
        Self { max_values_per_key: 64, max_keys: 4096, max_bytes: 4 * 1024 * 1024 }
    }
}

/// TTL-expiring, multi-value, re-announce-aware key/value store.
#[derive(Clone, Debug, Default)]
pub struct Store {
    map: HashMap<Key, Vec<Stored>>,
    params: StoreParams,
}

impl Store {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_params(params: StoreParams) -> Self {
        Self { map: HashMap::new(), params }
    }

    /// Store, or re-announce, a value under `key` with `ttl` ticks of life from
    /// `now`. If the same bytes already live there, refresh the TTL in place —
    /// the re-announce path: no duplicate, no growth, doesn't count against the
    /// cap. Otherwise append, first dropping anything expired and, if still at
    /// the cap, evicting the soonest-to-expire value.
    pub fn put(&mut self, key: Key, value: Vec<u8>, ttl: u64, now: u64) {
        let expires_at = now.saturating_add(ttl);
        let entry = self.map.entry(key).or_default();
        if let Some(existing) = entry.iter_mut().find(|s| s.bytes == value) {
            existing.expires_at = existing.expires_at.max(expires_at);
            return;
        }
        entry.retain(|s| s.expires_at > now);
        if entry.len() >= self.params.max_values_per_key {
            if let Some((idx, _)) =
                entry.iter().enumerate().min_by_key(|(_, s)| s.expires_at)
            {
                entry.remove(idx);
            }
        }
        entry.push(Stored { bytes: value, expires_at });
        self.enforce_caps(now);
    }

    /// Bytes of value data currently held.
    pub fn byte_count(&self) -> usize {
        self.map.values().flat_map(|v| v.iter()).map(|s| s.bytes.len()).sum()
    }

    /// Evict until both global caps hold.
    ///
    /// Expired data goes first — it is free to drop and about to go anyway. Only
    /// if that is not enough does live data get evicted, and then SOONEST-EXPIRING
    /// first, matching microdht's rule: the least-useful value is the one closest
    /// to expiry, and dropping the freshest announcement to keep a stale one
    /// inverts the whole point of announcing.
    fn enforce_caps(&mut self, now: u64) {
        if self.map.len() <= self.params.max_keys && self.byte_count() <= self.params.max_bytes {
            return;
        }
        self.gc(now);
        while self.map.len() > self.params.max_keys || self.byte_count() > self.params.max_bytes {
            // Find the soonest-expiring value anywhere.
            let victim = self.map.iter()
                .flat_map(|(k, v)| v.iter().map(move |s| (k.clone(), s.expires_at)))
                .min_by_key(|(_, e)| *e);
            let Some((key, expires_at)) = victim else { break };
            if let Some(v) = self.map.get_mut(&key) {
                if let Some(i) = v.iter().position(|s| s.expires_at == expires_at) { v.remove(i); }
                if v.is_empty() { self.map.remove(&key); }
            } else { break }
        }
    }

    /// All live values under `key` at `now`.
    pub fn get(&self, key: &Key, now: u64) -> Vec<Vec<u8>> {
        self.map
            .get(key)
            .map(|v| {
                v.iter()
                    .filter(|s| s.expires_at > now)
                    .map(|s| s.bytes.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Drop expired values (and now-empty keys).
    pub fn gc(&mut self, now: u64) {
        for v in self.map.values_mut() {
            v.retain(|s| s.expires_at > now);
        }
        self.map.retain(|_, v| !v.is_empty());
    }

    pub fn key_count(&self) -> usize {
        self.map.len()
    }

    pub fn value_count(&self, key: &Key) -> usize {
        self.map.get(key).map(|v| v.len()).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(b: u8) -> Key {
        let mut a = [0u8; crate::routing::ID_LEN];
        a[0] = b;
        NodeId(a)
    }

    #[test]
    fn put_then_get_returns_live_value() {
        let mut s = Store::new();
        s.put(key(1), b"v1".to_vec(), 100, 0);
        assert_eq!(s.get(&key(1), 0), vec![b"v1".to_vec()]);
    }

    #[test]
    fn value_expires_after_ttl() {
        let mut s = Store::new();
        s.put(key(1), b"v1".to_vec(), 100, 0);
        assert!(s.get(&key(1), 99).len() == 1);
        assert!(s.get(&key(1), 100).is_empty(), "expired at now == expires_at");
        assert!(s.get(&key(1), 200).is_empty());
    }

    #[test]
    fn reannounce_refreshes_ttl_without_duplicating() {
        let mut s = Store::new();
        s.put(key(1), b"v1".to_vec(), 100, 0); // expires at 100
        s.put(key(1), b"v1".to_vec(), 100, 50); // re-announce -> expires at 150
        assert_eq!(s.value_count(&key(1)), 1, "no duplicate value");
        assert_eq!(s.get(&key(1), 120).len(), 1, "still live past original TTL");
    }

    #[test]
    fn multiple_distinct_values_coexist_under_one_key() {
        let mut s = Store::new();
        s.put(key(1), b"a".to_vec(), 100, 0);
        s.put(key(1), b"b".to_vec(), 100, 0);
        let mut got = s.get(&key(1), 0);
        got.sort();
        assert_eq!(got, vec![b"a".to_vec(), b"b".to_vec()]);
    }

    #[test]
    fn per_key_cap_evicts_soonest_expiring() {
        let mut s = Store::with_params(StoreParams { max_values_per_key: 2, ..Default::default() });
        s.put(key(1), b"short".to_vec(), 10, 0); // expires at 10
        s.put(key(1), b"long".to_vec(), 1000, 0); // expires at 1000
        s.put(key(1), b"mid".to_vec(), 500, 0); // at cap -> evict "short"
        let got = s.get(&key(1), 20); // "short" gone (evicted, also expired)
        assert_eq!(got.len(), 2);
        assert!(got.contains(&b"long".to_vec()));
        assert!(got.contains(&b"mid".to_vec()));
        assert!(!got.contains(&b"short".to_vec()));
    }

    #[test]
    fn gc_drops_expired_and_empty_keys() {
        let mut s = Store::new();
        s.put(key(1), b"v".to_vec(), 10, 0);
        s.gc(100);
        assert_eq!(s.key_count(), 0);
    }

#[cfg(test)]
mod cap_tests {
    use super::*;

    fn key(n: u64) -> Key {
        let mut b = [0u8; crate::routing::ID_LEN];
        b[..8].copy_from_slice(&n.to_be_bytes());
        crate::routing::NodeId(b)
    }

    fn capped(keys: usize, bytes: usize) -> Store {
        Store::with_params(StoreParams { max_values_per_key: 64, max_keys: keys, max_bytes: bytes })
    }

    #[test]
    fn one_value_under_a_million_keys_cannot_exhaust_the_node() {
        // The hole the per-key cap never closed: it only fires when a single key
        // holds too much, and every key here holds exactly one value.
        let mut s = capped(10, 1 << 20);
        for i in 0..100u64 { s.put(key(i), vec![1, 2, 3], 1000, 0); }
        assert!(s.key_count() <= 10, "held {} keys", s.key_count());
    }

    #[test]
    fn a_byte_cap_survives_values_getting_larger() {
        // A cap on COUNT alone is defeated by making each value bigger.
        let mut s = capped(1000, 4096);
        for i in 0..100u64 { s.put(key(i), vec![0u8; 512], 1000, 0); }
        assert!(s.byte_count() <= 4096, "held {} bytes", s.byte_count());
    }

    #[test]
    fn expired_data_is_dropped_before_live_data() {
        // It is free to evict and about to go anyway; evicting a live value
        // while dead ones sit in the map is pure loss.
        let mut s = capped(3, 1 << 20);
        for i in 0..3u64 { s.put(key(i), vec![9], 10, 0); }     // expire at 10
        s.put(key(100), vec![9], 10_000, 100);                   // long-lived, later
        assert_eq!(s.key_count(), 1, "the three expired ones went first");
        assert_eq!(s.get(&key(100), 100).len(), 1, "and the live one stayed");
    }

    #[test]
    fn the_soonest_expiring_live_value_is_evicted_first() {
        // microdht's rule: the least-useful value is the one closest to expiry,
        // and dropping the freshest announcement to keep a stale one inverts the
        // point of announcing.
        let mut s = capped(2, 1 << 20);
        s.put(key(1), vec![1], 100, 0);      // expires at 100
        s.put(key(2), vec![2], 900, 0);      // expires at 900
        s.put(key(3), vec![3], 800, 0);      // expires at 800 — forces an eviction
        assert!(s.get(&key(1), 0).is_empty(), "the soonest-expiring went");
        assert_eq!(s.get(&key(2), 0).len(), 1, "the freshest stayed");
    }

    #[test]
    fn a_re_announce_still_refreshes_rather_than_counting_twice() {
        // The dedup path must not be defeated by the cap: re-announcing the same
        // value under the same key extends it, it does not consume budget.
        let mut s = capped(10, 1 << 20);
        for _ in 0..50 { s.put(key(1), vec![7], 500, 0); }
        assert_eq!(s.key_count(), 1);
        assert_eq!(s.get(&key(1), 0).len(), 1, "still one value");
    }

    #[test]
    fn a_store_under_its_caps_evicts_nothing() {
        // The common case must not pay for the pathological one.
        let mut s = capped(100, 1 << 20);
        for i in 0..20u64 { s.put(key(i), vec![1, 2, 3, 4], 1000, 0); }
        assert_eq!(s.key_count(), 20);
        for i in 0..20u64 { assert_eq!(s.get(&key(i), 0).len(), 1, "key {i}"); }
    }
}
}
