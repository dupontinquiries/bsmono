//! Network constants and async socket helpers for gossip.

/// Gossip multicast group address (matches C++ GossipDiscovery.cpp).
pub const GOSSIP_MULTICAST_ADDR: &str = "224.0.0.251";
/// Gossip UDP listen port (matches C++ kGossipPort).
pub const GOSSIP_PORT: u16 = 9877;

/// Build an announce JSON blob for ourself.
/// Called at start() and periodically by the tick timer.
pub fn build_announce(
    sc_id:  &str,
    host:   &str,
    port:   u16,
    peers:  Vec<crate::message::GossipPeer>,
) -> anyhow::Result<Vec<u8>> {
    let msg = crate::message::GossipMessage {
        id:    sc_id.to_owned(),
        host:  host.to_owned(),
        port,
        ts:    chrono::Utc::now().timestamp_millis(),
        peers: peers.into_iter().take(crate::message::MAX_GOSSIP_PEERS).collect(),
    };
    crate::message::encode_gossip(&msg)
}
