//! Gossip peer discovery — Rust port of GossipDiscovery.cpp.
//! Status: scaffold — see GossipDiscovery.cpp for the full protocol.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::SocketAddr;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerRecord {
    pub sc_id:   String,
    pub address: SocketAddr,
    pub seen_at: chrono::DateTime<chrono::Utc>,
}

/// In-memory view of known peers discovered via gossip.
#[derive(Debug, Default)]
pub struct GossipView {
    peers: HashMap<String, PeerRecord>,
}

impl GossipView {
    pub fn insert(&mut self, record: PeerRecord) {
        self.peers.insert(record.sc_id.clone(), record);
    }

    pub fn get(&self, sc_id: &str) -> Option<&PeerRecord> {
        self.peers.get(sc_id)
    }

    pub fn all(&self) -> impl Iterator<Item = &PeerRecord> {
        self.peers.values()
    }

    /// Purge records older than `max_age`.
    pub fn purge_stale(&mut self, max_age: std::time::Duration) {
        let cutoff = chrono::Utc::now() - chrono::Duration::from_std(max_age).unwrap();
        self.peers.retain(|_, r| r.seen_at > cutoff);
    }
}
