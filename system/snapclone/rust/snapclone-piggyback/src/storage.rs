//! Storage trait for encrypted blobs while they wait to be forwarded.
//!
//! The trait is pluggable so the storage backend can be swapped without
//! touching the relay logic. Typical backends:
//!
//!   - `InMemoryStorage` (this file): for tests and ephemeral relays.
//!     Lost on process restart.
//!   - sled/sqlite backed: production. Survives restart. Use a budget
//!     to cap disk usage; see `limits::DEFAULT_STORAGE_BUDGET_BYTES`.
//!   - tmpfs-backed file: middle ground for devices where the filesystem
//!     is read-mostly but RAM is plentiful.
//!
//! Eviction policy: LRU by `accepted_at_ms`. When storage is over
//! budget, evict the oldest entries first regardless of remaining TTL.
//! Callers should also TTL-expire entries based on snap TTL.

use crate::chain::RelayChain;

/// One stored relay item: the encrypted blob plus its chain so far.
#[derive(Debug, Clone)]
pub struct StorageEntry {
    /// 32-byte SHA-256 of the encrypted blob — also the storage key.
    pub blob_hash: [u8; 32],

    /// The encrypted blob itself. Opaque; this crate never touches its
    /// contents.
    pub blob: Vec<u8>,

    /// Chain-of-hops up to and including the current holder.
    pub chain: RelayChain,

    /// When this hop accepted the blob (unix millis). Used for LRU
    /// eviction.
    pub accepted_at_ms: i64,

    /// When the blob expires (unix millis). Storage MAY drop entries
    /// past this point even if budget allows.
    pub expires_at_ms: i64,
}

/// Errors specific to the storage backend.
#[derive(Debug, Clone, thiserror::Error)]
pub enum StorageError {
    /// The store is at its byte budget and the new entry can't fit.
    /// Callers should react by either evicting OR refusing the relay.
    #[error("storage budget exceeded")]
    BudgetExceeded,

    /// Backend-specific IO error.
    #[error("backend IO: {0}")]
    Io(String),

    /// Requested entry doesn't exist.
    #[error("not found")]
    NotFound,
}

/// Pluggable storage backend for in-transit relay blobs.
///
/// Implementations must be thread-safe (Send + Sync) because the relay
/// orchestrator may receive blobs on one task and forward them from
/// another.
pub trait Storage: Send + Sync {
    /// Insert an entry. Returns Err(BudgetExceeded) if no room and
    /// eviction policy doesn't free enough.
    fn put(&self, entry: StorageEntry) -> Result<(), StorageError>;

    /// Look up an entry by blob hash. Returns Err(NotFound) if absent.
    fn get(&self, blob_hash: &[u8; 32]) -> Result<StorageEntry, StorageError>;

    /// Remove an entry, e.g. after successful delivery to the recipient
    /// or expiry. Idempotent — removing a missing entry returns Ok.
    fn remove(&self, blob_hash: &[u8; 32]) -> Result<(), StorageError>;

    /// All currently-stored entry hashes. Used by the orchestrator's
    /// forward-attempt loop. Order is implementation-defined but the
    /// orchestrator typically wants oldest-first for fairness.
    fn list_hashes(&self) -> Vec<[u8; 32]>;

    /// Total bytes currently stored (blob + chain serialization
    /// overhead). Used to enforce the budget.
    fn total_bytes(&self) -> usize;
}

/// In-memory storage backend. For tests and ephemeral / low-memory
/// scenarios where persistence isn't needed. Uses a Mutex<HashMap>
/// internally — simple, correct, fine for the modest concurrency a
/// single device sees.
#[derive(Default)]
pub struct InMemoryStorage {
    inner: std::sync::Mutex<std::collections::HashMap<[u8; 32], StorageEntry>>,
    /// Max bytes — 0 means unlimited (test/debug use).
    budget_bytes: usize,
}

impl InMemoryStorage {
    /// New store with the given byte budget. Pass 0 for unlimited.
    pub fn new(budget_bytes: usize) -> Self {
        Self {
            inner: Default::default(),
            budget_bytes,
        }
    }

    /// Evict the oldest entries (by accepted_at_ms) until total_bytes
    /// is at or below `target`. Returns the number of entries removed.
    /// Used internally when an insert would overflow the budget.
    fn evict_oldest_until(&self, target: usize) -> usize {
        let mut inner = self.inner.lock().unwrap();
        let mut current: usize = inner.values()
            .map(|e| e.blob.len())
            .sum();
        if current <= target { return 0; }
        // Collect (hash, accepted_at_ms) and sort oldest first.
        let mut ordered: Vec<([u8; 32], i64, usize)> = inner.iter()
            .map(|(h, e)| (*h, e.accepted_at_ms, e.blob.len()))
            .collect();
        ordered.sort_by_key(|(_, ts, _)| *ts);
        let mut removed = 0;
        for (h, _ts, len) in ordered {
            if current <= target { break; }
            inner.remove(&h);
            current = current.saturating_sub(len);
            removed += 1;
        }
        removed
    }
}

impl Storage for InMemoryStorage {
    fn put(&self, entry: StorageEntry) -> Result<(), StorageError> {
        let new_len = entry.blob.len();
        // Evict if needed.
        if self.budget_bytes > 0 {
            let current = self.total_bytes();
            if current + new_len > self.budget_bytes {
                let target = self.budget_bytes.saturating_sub(new_len);
                self.evict_oldest_until(target);
                if self.total_bytes() + new_len > self.budget_bytes {
                    // Even after eviction we can't fit — refuse rather
                    // than partially evict + accept (would let big
                    // entries permanently push everything else out).
                    return Err(StorageError::BudgetExceeded);
                }
            }
        }
        self.inner.lock().unwrap().insert(entry.blob_hash, entry);
        Ok(())
    }

    fn get(&self, blob_hash: &[u8; 32]) -> Result<StorageEntry, StorageError> {
        self.inner.lock().unwrap()
            .get(blob_hash)
            .cloned()
            .ok_or(StorageError::NotFound)
    }

    fn remove(&self, blob_hash: &[u8; 32]) -> Result<(), StorageError> {
        self.inner.lock().unwrap().remove(blob_hash);
        Ok(())  // Idempotent — missing keys aren't an error.
    }

    fn list_hashes(&self) -> Vec<[u8; 32]> {
        let inner = self.inner.lock().unwrap();
        let mut hashes: Vec<([u8; 32], i64)> = inner.iter()
            .map(|(h, e)| (*h, e.accepted_at_ms))
            .collect();
        hashes.sort_by_key(|(_, ts)| *ts);
        hashes.into_iter().map(|(h, _)| h).collect()
    }

    fn total_bytes(&self) -> usize {
        self.inner.lock().unwrap().values().map(|e| e.blob.len()).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chain::RelayChain;

    fn entry(hash: u8, size: usize, ts: i64) -> StorageEntry {
        StorageEntry {
            blob_hash: [hash; 32],
            blob: vec![0; size],
            chain: RelayChain::new(),
            accepted_at_ms: ts,
            expires_at_ms: ts + 60_000,
        }
    }

    #[test]
    fn put_get_remove_roundtrip() {
        let store = InMemoryStorage::new(0);
        store.put(entry(7, 100, 1000)).unwrap();
        let got = store.get(&[7; 32]).unwrap();
        assert_eq!(got.blob.len(), 100);
        store.remove(&[7; 32]).unwrap();
        assert!(matches!(store.get(&[7; 32]), Err(StorageError::NotFound)));
    }

    #[test]
    fn remove_missing_is_ok() {
        let store = InMemoryStorage::new(0);
        // Removing a non-existent entry doesn't error.
        store.remove(&[99; 32]).unwrap();
    }

    #[test]
    fn budget_evicts_oldest() {
        // 250-byte budget, three 100-byte entries → oldest is evicted.
        let store = InMemoryStorage::new(250);
        store.put(entry(1, 100, 1000)).unwrap();
        store.put(entry(2, 100, 2000)).unwrap();
        store.put(entry(3, 100, 3000)).unwrap();
        // [1] should have been evicted (oldest), [2] and [3] kept.
        assert!(matches!(store.get(&[1; 32]), Err(StorageError::NotFound)));
        assert!(store.get(&[2; 32]).is_ok());
        assert!(store.get(&[3; 32]).is_ok());
    }

    #[test]
    fn budget_refuses_when_eviction_insufficient() {
        // 100-byte budget, single 200-byte entry can never fit.
        let store = InMemoryStorage::new(100);
        let err = store.put(entry(1, 200, 1000)).unwrap_err();
        assert!(matches!(err, StorageError::BudgetExceeded));
    }
}
