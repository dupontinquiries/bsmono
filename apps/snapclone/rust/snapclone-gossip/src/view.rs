//! Peer view — the gossip overlay's in-memory knowledge of reachable peers.
//!
//! Ported from GossipDiscovery's m_view map + pruneDeadPeers() + onGossipTick().

use crate::message::{GossipMessage, GossipPeer};
use chrono::{DateTime, Duration, Utc};
use std::collections::HashMap;
use tracing::debug;

/// How long a peer record is considered fresh (matches C++ kViewTtlMs / 1000)
pub const PEER_TTL_SECS: i64 = 120;
/// Maximum peers to keep in the local view (matches C++ kMaxViewSize)
pub const MAX_VIEW_SIZE: usize = 64;

#[derive(Debug, Clone)]
pub struct PeerRecord {
    pub sc_id:    String,
    pub host:     String,
    pub port:     u16,
    pub last_seen: DateTime<Utc>,
}

/// In-memory peer view with TTL-based pruning.
#[derive(Debug, Default)]
pub struct GossipView {
    peers: HashMap<String, PeerRecord>,
}

impl GossipView {
    pub fn new() -> Self { Self::default() }

    /// Merge an incoming gossip message into the view.
    /// Returns the list of newly discovered peer sc-IDs.
    pub fn merge(&mut self, msg: &GossipMessage, sender_host: &str) -> Vec<String> {
        let mut new_peers = Vec::new();

        // First: insert the sender itself
        let effective_host = if msg.host.is_empty() || msg.host == "0.0.0.0" {
            sender_host.to_owned()
        } else {
            msg.host.clone()
        };
        if !self.peers.contains_key(&msg.id) && self.peers.len() < MAX_VIEW_SIZE {
            new_peers.push(msg.id.clone());
        }
        self.peers.insert(msg.id.clone(), PeerRecord {
            sc_id: msg.id.clone(), host: effective_host,
            port: msg.port, last_seen: Utc::now(),
        });

        // Then: merge gossip sample
        for peer in &msg.peers {
            if self.peers.contains_key(&peer.id) { continue; } // already known
            if self.peers.len() >= MAX_VIEW_SIZE  { break; }   // cap view size
            // Reject stale entries (older than 2× TTL)
            let age_ms = Utc::now().timestamp_millis() - peer.ts;
            if age_ms > PEER_TTL_SECS * 2 * 1000 {
                debug!("Gossip: rejecting stale peer {} (age {}ms)", peer.id, age_ms);
                continue;
            }
            new_peers.push(peer.id.clone());
            self.peers.insert(peer.id.clone(), PeerRecord {
                sc_id:     peer.id.clone(),
                host:      peer.host.clone(),
                port:      peer.port,
                last_seen: peer.last_seen().unwrap_or_else(Utc::now),
            });
        }
        new_peers
    }

    /// Remove stale entries older than PEER_TTL_SECS.
    pub fn prune(&mut self) {
        let cutoff = Utc::now() - Duration::seconds(PEER_TTL_SECS);
        let before = self.peers.len();
        self.peers.retain(|_, r| r.last_seen > cutoff);
        let pruned = before - self.peers.len();
        if pruned > 0 {
            debug!("Gossip: pruned {} stale peers", pruned);
        }
    }

    /// Select a random gossip sample of up to `n` peers to include in a message.
    pub fn sample(&self, n: usize, exclude_id: &str) -> Vec<GossipPeer> {
        use rand::seq::IteratorRandom;
        let mut rng = rand::thread_rng();
        self.peers.values()
            .filter(|r| r.sc_id != exclude_id)
            .choose_multiple(&mut rng, n)
            .into_iter()
            .map(|r| GossipPeer {
                id:   r.sc_id.clone(),
                host: r.host.clone(),
                port: r.port,
                ts:   r.last_seen.timestamp_millis(),
            })
            .collect()
    }

    pub fn get(&self, sc_id: &str)           -> Option<&PeerRecord> { self.peers.get(sc_id) }
    pub fn contains(&self, sc_id: &str)       -> bool               { self.peers.contains_key(sc_id) }
    pub fn len(&self)                          -> usize              { self.peers.len() }
    pub fn is_empty(&self)                     -> bool              { self.peers.is_empty() }
    pub fn all(&self) -> impl Iterator<Item = &PeerRecord>          { self.peers.values() }
    pub fn remove(&mut self, sc_id: &str)      -> Option<PeerRecord>{ self.peers.remove(sc_id) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::GossipMessage;

    fn msg(id: &str, host: &str) -> GossipMessage {
        GossipMessage { id: id.to_owned(), host: host.to_owned(), port: 9876,
                        ts: Utc::now().timestamp_millis(), peers: vec![] }
    }

    #[test]
    fn merge_adds_sender() {
        let mut view = GossipView::new();
        let new = view.merge(&msg("sc-aaaaaaaa", "192.168.1.1"), "192.168.1.1");
        assert_eq!(view.len(), 1);
        assert_eq!(new, vec!["sc-aaaaaaaa"]);
    }

    #[test]
    fn prune_removes_stale() {
        let mut view = GossipView::new();
        view.peers.insert("sc-aaaaaaaa".to_owned(), PeerRecord {
            sc_id: "sc-aaaaaaaa".to_owned(), host: "1.2.3.4".to_owned(), port: 9876,
            last_seen: Utc::now() - Duration::seconds(PEER_TTL_SECS + 10),
        });
        view.prune();
        assert!(view.is_empty());
    }

    #[test]
    fn sample_excludes_self() {
        let mut view = GossipView::new();
        view.merge(&msg("sc-aaaaaaaa", "1.1.1.1"), "1.1.1.1");
        view.merge(&msg("sc-bbbbbbbb", "2.2.2.2"), "2.2.2.2");
        let sample = view.sample(8, "sc-aaaaaaaa");
        assert!(!sample.iter().any(|p| p.id == "sc-aaaaaaaa"));
    }
}
