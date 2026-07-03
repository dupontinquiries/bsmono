//! snapclone-relay — Magic Wormhole relay transport.
//!
//! Rust port of MagicWormholeRelayTransport.cpp.
//! Manages a WebSocket connection to a relay server for NAT traversal
//! when direct peer connections are impossible (CGNAT, strict firewalls).
//!
//! # Protocol
//! 1. Connect to `wss://relay-host/v1`
//! 2. Send `{"type":"bind","id":"sc-XXXXXXXX","peer":"sc-YYYYYYYY"}`
//! 3. Relay server pairs us with the peer when they bind with reversed IDs
//! 4. Subsequent messages are forwarded transparently
//! 5. On disconnect: exponential backoff reconnect (2s→4s→8s, 3 attempts)

pub mod transport;
pub mod protocol;
pub mod backoff;

pub use transport::{RelayTransport, RelayEvent};
pub use protocol::{RelayMessage, encode_relay_msg, decode_relay_msg};
pub use backoff::ExponentialBackoff;
