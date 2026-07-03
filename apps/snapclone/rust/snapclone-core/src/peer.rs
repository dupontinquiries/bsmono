//! Peer / network presence — the connection-layer view of a contact.
//!
//! Separate from Contact because a Contact is a persistent social relationship
//! while a Peer is an ephemeral network session. You can have contacts who are
//! offline (no Peer) and inbound connections from unknown IDs (Peer but no Contact yet).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum PeerStatus {
    #[default]
    Offline,
    Connecting,  // DHT lookup or TCP handshake in progress
    Online,      // SHAK complete, session active
    Relayed,     // connected via relay (direct path failed)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Peer {
    /// Same sc-ID as the corresponding Contact, or "inbound:ip:port" for strangers
    pub id:      String,
    pub status:  PeerStatus,
    /// IP:port of the current transport (may change across reconnects)
    pub address: Option<String>,
    /// True if we initiated the connection (determines approval flow)
    pub we_initiated: bool,
}
