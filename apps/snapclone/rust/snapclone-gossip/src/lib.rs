//! snapclone-gossip — UDP gossip peer discovery protocol.
//!
//! Rust port of GossipDiscovery.cpp (395 lines → structured modules).
//!
//! # Protocol
//!
//! SnapClone uses a simple push-based gossip protocol over UDP multicast
//! (224.0.0.251:9877 on LAN). Each peer periodically broadcasts a JSON
//! message containing its own address and a random subset of peers it knows.
//! Recipients merge the incoming peer list into their local view.
//!
//! # Modules
//!
//! - `message` — JSON gossip message encoding/decoding (replaces raw QJsonObject)
//! - `view`    — peer view (map of sc-ID → PeerRecord) with TTL pruning
//! - `codec`   — multicast address constants and socket helpers
//!
//! # Porting status
//!
//! | C++ method                      | Rust location          | Status |
//! |---------------------------------|------------------------|--------|
//! | GossipDiscovery::start()        | codec::bind_multicast()| scaffold |
//! | GossipDiscovery::onDatagramReady| message::decode()      | DONE   |
//! | GossipDiscovery::processDatagram| message::decode()      | DONE   |
//! | GossipDiscovery::gossipToPeer() | message::encode()      | DONE   |
//! | GossipDiscovery::onGossipTick() | view::GossipView::sample()| DONE  |
//! | GossipDiscovery::pruneDeadPeers | view::GossipView::prune() | DONE  |
//! | GossipDiscovery::announcePresence | message::encode()    | DONE   |

pub mod message;
pub mod view;
pub mod codec;

pub use message::{GossipMessage, GossipPeer, encode_gossip, decode_gossip};
pub use view::{GossipView, PeerRecord};
pub use codec::{GOSSIP_MULTICAST_ADDR, GOSSIP_PORT};
