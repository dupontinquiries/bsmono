//! Traversal strategy hints derived from the reachability database.
//!
//! NETWORK_ARCHITECTURE.md §3: "Weighted graph / probabilistic routing hints,
//! follow warm paths first strategy."

use crate::{bins::NetworkBin, memory::ReachabilityDb, scoring::Protocol};
use serde::{Deserialize, Serialize};

/// Ordered list of strategies to try, best-first.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraversalHints {
    pub strategies: Vec<TraversalStrategy>,
}

/// A single traversal approach.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TraversalStrategy {
    /// Direct UDP hole-punch to the peer's reflexive address.
    UdpPunch { confidence: f32 },
    /// Direct TCP WebSocket to the peer's address.
    TcpDirect { confidence: f32 },
    /// Ask an assistant node to help coordinate simultaneous punch.
    /// (NETWORK_ARCHITECTURE.md §4: 3-device model)
    AssistPunch { assistant_id: String, confidence: f32 },
    /// Relay via a known relay server.
    Relay { relay_url: String },
    /// DHT mailbox rendezvous (NETWORK_ARCHITECTURE.md §9 Layer 3).
    MailboxRendezvous,
}

/// Generate traversal hints for a peer based on their address and our history.
pub fn hints_for(
    db:       &ReachabilityDb,
    peer_id:  &str,
    peer_ip:  std::net::IpAddr,
    is_cgnat: bool,
) -> TraversalHints {
    let bin = NetworkBin::of(peer_ip);
    let mut strategies = Vec::new();

    if is_cgnat {
        // CGNAT: skip UDP punch, go straight to relay
        // (NETWORK_ARCHITECTURE.md §9: escalation layers)
        strategies.push(TraversalStrategy::MailboxRendezvous);
        strategies.push(TraversalStrategy::Relay { relay_url: String::new() });
        return TraversalHints { strategies };
    }

    // Check for warm UDP path first
    if !db.is_blocked(peer_id, &bin, Protocol::Udp) {
        let conf = db.best_protocol(peer_id, &bin)
            .filter(|&p| p == Protocol::Udp)
            .map(|_| 0.9f32)
            .unwrap_or(0.5);
        strategies.push(TraversalStrategy::UdpPunch { confidence: conf });
    }

    // TCP direct
    if !db.is_blocked(peer_id, &bin, Protocol::TcpDirect) {
        strategies.push(TraversalStrategy::TcpDirect { confidence: 0.6 });
    }

    // Relay as final fallback
    strategies.push(TraversalStrategy::MailboxRendezvous);
    strategies.push(TraversalStrategy::Relay { relay_url: String::new() });

    TraversalHints { strategies }
}
