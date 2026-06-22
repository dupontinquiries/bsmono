//! store.rs — the TTL-expiring key → values map at the heart of the micro-DHT.
//!
//! Mirrors the semantics the SnapClone app relies on from OpenDHT:
//!   * A DHT *key* is an sc-ID (e.g. "sc-7aa911ac"). The app publishes its
//!     encrypted endpoint blob under its own sc-ID, and looks a peer up by the
//!     peer's sc-ID.
//!   * Multiple values can live under one key (multi-device / re-announce
//!     churn), so this is key -> Vec<value>, deduped by value bytes.
//!   * Values EXPIRE. OpenDHT values live ~1 hour and the app re-announces
//!     every ~55 min (see kReannounceIntervalMs). We honor a per-value TTL and
//!     sweep expired entries, so a lookup after the publisher goes away returns
//!     nothing — exactly like the real DHT, which is what makes "did my peer
//!     drop off?" testable.
//!
//! Deliberately dependency-free: a Mutex<HashMap> guarded store, value bytes
//! kept as Vec<u8> (the app's values are opaque encrypted blobs to the DHT).

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// One stored value plus its expiry deadline.
#[derive(Clone, Debug)]
struct Entry {
    value: Vec<u8>,
    expires_at: Instant,
}

/// The micro-DHT's value store. `key` is the sc-ID string (we keep it as the
/// raw bytes the protocol carries so we never assume a particular text form),
/// mapping to a list of unexpired values.
#[derive(Default)]
pub struct Store {
    map: HashMap<Vec<u8>, Vec<Entry>>,
    /// Stats for the status line / `STATS` query — handy when eyeballing a
    /// beta-test session.
    pub total_puts: u64,
    pub total_gets: u64,
    pub total_expired: u64,
    pub total_capped: u64,
}

/// Max distinct values retained under a single key. A rendezvous record is a
/// peer's address set + queued CONNREQs — a handful of small blobs, never dozens.
/// Capping bounds memory so one client can't fill the node by PUTting unlimited
/// distinct values under one key (Stage-0 abuse resistance). When exceeded we
/// evict the entry that expires soonest (the least-useful — it's about to go
/// anyway), keeping the freshest announcements.
pub const MAX_VALUES_PER_KEY: usize = 8;

impl Store {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert (or refresh) a value under `key` with the given TTL. If the exact
    /// value bytes already exist under this key, its expiry is bumped rather
    /// than stored twice — this is the re-announce path (same blob, new TTL),
    /// and keeping it dedup'd means a lookup doesn't return N stale copies.
    pub fn put(&mut self, key: &[u8], value: Vec<u8>, ttl: Duration) {
        self.total_puts += 1;
        let now = Instant::now();
        let expires_at = now + ttl;
        let entries = self.map.entry(key.to_vec()).or_default();
        if let Some(existing) = entries.iter_mut().find(|e| e.value == value) {
            existing.expires_at = expires_at; // re-announce: refresh TTL
            return;
        }
        entries.push(Entry { value, expires_at });
        // Stage-0 cap: bound distinct values per key. Drop expired first (free),
        // then if still over cap evict the soonest-to-expire.
        if entries.len() > MAX_VALUES_PER_KEY {
            entries.retain(|e| e.expires_at > now);
            while entries.len() > MAX_VALUES_PER_KEY {
                // index of the soonest-expiring entry
                let mut victim = 0usize;
                for (i, e) in entries.iter().enumerate() {
                    if e.expires_at < entries[victim].expires_at {
                        victim = i;
                    }
                }
                entries.remove(victim);
                self.total_capped += 1;
            }
        }
    }

    /// Return all currently-unexpired values under `key`. Sweeps expired
    /// entries lazily on read so a GET never returns stale data even if the
    /// background sweeper hasn't run yet.
    pub fn get(&mut self, key: &[u8]) -> Vec<Vec<u8>> {
        self.total_gets += 1;
        let now = Instant::now();
        let Some(entries) = self.map.get_mut(key) else {
            return Vec::new();
        };
        let before = entries.len();
        entries.retain(|e| e.expires_at > now);
        self.total_expired += (before - entries.len()) as u64;
        let out: Vec<Vec<u8>> = entries.iter().map(|e| e.value.clone()).collect();
        if entries.is_empty() {
            self.map.remove(key);
        }
        out
    }

    /// Background sweep: drop every expired value across all keys, and remove
    /// keys that end up empty. Returns how many values were reaped (for logging).
    pub fn sweep(&mut self) -> usize {
        let now = Instant::now();
        let mut reaped = 0;
        self.map.retain(|_, entries| {
            let before = entries.len();
            entries.retain(|e| e.expires_at > now);
            reaped += before - entries.len();
            !entries.is_empty()
        });
        self.total_expired += reaped as u64;
        reaped
    }

    /// Number of distinct keys currently holding at least one live value.
    pub fn key_count(&self) -> usize {
        self.map.len()
    }

    /// Total live values across all keys.
    pub fn value_count(&self) -> usize {
        self.map.values().map(|v| v.len()).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn put_get_roundtrip() {
        let mut s = Store::new();
        s.put(b"sc-deadbeef", b"endpoint-blob".to_vec(), Duration::from_secs(60));
        let got = s.get(b"sc-deadbeef");
        assert_eq!(got, vec![b"endpoint-blob".to_vec()]);
    }

    #[test]
    fn expiry_removes_value() {
        let mut s = Store::new();
        s.put(b"sc-1", b"v".to_vec(), Duration::from_millis(1));
        std::thread::sleep(Duration::from_millis(5));
        assert!(s.get(b"sc-1").is_empty());
        assert_eq!(s.key_count(), 0);
    }

    #[test]
    fn reannounce_dedups_and_refreshes() {
        let mut s = Store::new();
        s.put(b"sc-2", b"same".to_vec(), Duration::from_millis(5));
        s.put(b"sc-2", b"same".to_vec(), Duration::from_secs(60)); // re-announce
        assert_eq!(s.get(b"sc-2").len(), 1); // not duplicated
        std::thread::sleep(Duration::from_millis(10));
        assert_eq!(s.get(b"sc-2").len(), 1); // refreshed TTL kept it alive
    }

    #[test]
    fn multiple_values_per_key() {
        let mut s = Store::new();
        s.put(b"sc-3", b"a".to_vec(), Duration::from_secs(60));
        s.put(b"sc-3", b"b".to_vec(), Duration::from_secs(60));
        let mut got = s.get(b"sc-3");
        got.sort();
        assert_eq!(got, vec![b"a".to_vec(), b"b".to_vec()]);
    }

    #[test]
    fn sweep_reaps_expired() {
        let mut s = Store::new();
        s.put(b"sc-4", b"x".to_vec(), Duration::from_millis(1));
        s.put(b"sc-5", b"y".to_vec(), Duration::from_secs(60));
        std::thread::sleep(Duration::from_millis(5));
        assert_eq!(s.sweep(), 1);
        assert_eq!(s.key_count(), 1);
    }

    #[test]
    fn per_key_value_cap_is_enforced() {
        let mut s = Store::new();
        let key = b"sc-abc";
        // PUT more distinct values than the cap allows
        for i in 0..(MAX_VALUES_PER_KEY + 5) {
            s.put(key, vec![i as u8], Duration::from_secs(60));
        }
        // never exceeds the cap
        assert!(s.get(key).len() <= MAX_VALUES_PER_KEY,
                "key held {} values, cap is {}", s.get(key).len(), MAX_VALUES_PER_KEY);
        assert!(s.total_capped >= 5, "should have evicted the overflow");
    }

    #[test]
    fn cap_keeps_the_longest_lived_values() {
        let mut s = Store::new();
        let key = b"sc-xyz";
        // fill to cap with long TTLs
        for i in 0..MAX_VALUES_PER_KEY {
            s.put(key, vec![i as u8], Duration::from_secs(3600));
        }
        // one more with a SHORT ttl — it should be the one evicted (soonest expiry)
        s.put(key, vec![254u8], Duration::from_secs(1));
        let vals = s.get(key);
        assert!(vals.len() <= MAX_VALUES_PER_KEY);
        assert!(!vals.iter().any(|v| v == &vec![254u8]),
                "the short-TTL late value should have been evicted as soonest-to-expire");
    }

    #[test]
    fn reannounce_does_not_count_against_cap() {
        let mut s = Store::new();
        let key = b"sc-re";
        // re-PUT the SAME value many times — it's a re-announce, not growth
        for _ in 0..(MAX_VALUES_PER_KEY + 10) {
            s.put(key, vec![42u8], Duration::from_secs(60));
        }
        assert_eq!(s.get(key).len(), 1, "re-announce must dedup, not accumulate");
        assert_eq!(s.total_capped, 0, "dedup'd re-announce should not trigger eviction");
    }
}
