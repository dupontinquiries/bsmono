//! Gossip JSON message codec — ported from GossipDiscovery.cpp processDatagram().
//!
//! Wire format (JSON, max 4096 bytes):
//! ```json
//! {
//!   "id":   "sc-32a51661",
//!   "host": "192.168.1.5",
//!   "port": 9876,
//!   "ts":   1700000000000,
//!   "peers": [
//!     { "id": "sc-aabbccdd", "host": "192.168.1.6", "port": 9876, "ts": ... },
//!     ...
//!   ]
//! }
//! ```
//! The `peers` array is a random gossip sample (up to kMaxGossipPeers = 8).
//! Receiving peers merge this into their view with TTL validation.

use anyhow::{bail, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Max gossip datagram size — matches kMaxGossipDatagramBytes in GossipDiscovery.cpp
pub const MAX_GOSSIP_BYTES: usize = 4096;

/// Max peers to include in a single gossip message.
pub const MAX_GOSSIP_PEERS: usize = 8;

/// A peer entry within a gossip message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GossipPeer {
    pub id:   String,
    pub host: String,
    pub port: u16,
    pub ts:   i64, // ms since epoch
}

impl GossipPeer {
    pub fn last_seen(&self) -> Option<DateTime<Utc>> {
        DateTime::from_timestamp_millis(self.ts)
    }
}

/// A complete gossip message — sender info + peer sample.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GossipMessage {
    /// Sender's stable sc-ID.
    pub id:   String,
    /// Sender's reachable IP address (may differ from UDP source if behind NAT).
    pub host: String,
    /// Sender's WebSocket listen port.
    pub port: u16,
    /// Sender's current timestamp (used to detect stale gossip).
    pub ts:   i64,
    /// Gossip sample: a subset of peers the sender knows about.
    #[serde(default)]
    pub peers: Vec<GossipPeer>,
}

/// Encode a gossip message to JSON bytes.
/// Returns `Err` if the JSON exceeds MAX_GOSSIP_BYTES.
pub fn encode_gossip(msg: &GossipMessage) -> Result<Vec<u8>> {
    let json = serde_json::to_vec(msg)?;
    if json.len() > MAX_GOSSIP_BYTES {
        bail!("Gossip message too large: {} bytes (max {})", json.len(), MAX_GOSSIP_BYTES);
    }
    Ok(json)
}

/// Decode a gossip message from JSON bytes.
/// Validates message structure and rejects obviously bad data.
pub fn decode_gossip(data: &[u8]) -> Result<GossipMessage> {
    if data.len() > MAX_GOSSIP_BYTES {
        bail!("Datagram too large: {} bytes", data.len());
    }
    let msg: GossipMessage = serde_json::from_slice(data)?;

    // Validate sc-ID format — matches C++ kScIdRx
    if !is_valid_sc_id(&msg.id) {
        bail!("Invalid sc-ID in gossip message: {:?}", msg.id);
    }
    // Validate port range
    if msg.port == 0 {
        bail!("Invalid port 0 in gossip message");
    }
    // Validate peer count (cap to prevent memory abuse)
    if msg.peers.len() > MAX_GOSSIP_PEERS * 2 {
        bail!("Too many peers in gossip message: {}", msg.peers.len());
    }
    // Filter invalid peer entries silently (C++ also does this)
    Ok(GossipMessage {
        peers: msg.peers.into_iter()
            .filter(|p| is_valid_sc_id(&p.id) && p.port > 0 && !p.host.is_empty())
            .take(MAX_GOSSIP_PEERS)
            .collect(),
        ..msg
    })
}

fn is_valid_sc_id(s: &str) -> bool {
    s.len() == 11 && s.starts_with("sc-") && s[3..].chars().all(|c| c.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample_msg() -> GossipMessage {
        GossipMessage {
            id: "sc-32a51661".to_owned(), host: "192.168.1.5".to_owned(),
            port: 9876, ts: 1700000000000,
            peers: vec![GossipPeer { id: "sc-aabbccdd".to_owned(), host: "192.168.1.6".to_owned(), port: 9876, ts: 1700000000000 }],
        }
    }
    #[test] fn roundtrip()         { let m = sample_msg(); let d = encode_gossip(&m).unwrap(); let m2 = decode_gossip(&d).unwrap(); assert_eq!(m2.id, m.id); }
    #[test] fn rejects_bad_id()    { let mut m = sample_msg(); m.id = "bad-id".to_owned(); assert!(encode_gossip(&m).and_then(|d| decode_gossip(&d)).is_err()); }
    #[test] fn rejects_port_zero() { let mut m = sample_msg(); m.port = 0; assert!(encode_gossip(&m).and_then(|d| decode_gossip(&d)).is_err()); }
    #[test] fn filters_bad_peers() {
        let mut m = sample_msg();
        m.peers.push(GossipPeer { id: "invalid".to_owned(), host: "1.2.3.4".to_owned(), port: 9876, ts: 0 });
        let d  = encode_gossip(&m).unwrap();
        let m2 = decode_gossip(&d).unwrap();
        assert_eq!(m2.peers.len(), 1); // bad peer filtered out
    }
}
