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
}

impl Default for StoreParams {
    fn default() -> Self {
        Self { max_values_per_key: 64 }
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
        let mut s = Store::with_params(StoreParams { max_values_per_key: 2 });
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
}
