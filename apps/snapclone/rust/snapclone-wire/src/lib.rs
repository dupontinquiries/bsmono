//! snapclone-wire — binary wire frame codecs for peer discovery.
//!
//! Ported from the wire format documentation in DirectDiscovery.cpp.
//! These codecs are used by both LAN discovery (UDP broadcast/multicast)
//! and gossip exchange (over WebSocket sessions).
//!
//! # Frame types
//!
//! | Magic      | Name  | Purpose                                   |
//! |------------|-------|-------------------------------------------|
//! | 0x53434150 | SCAP  | Announce presence (broadcast/multicast)   |
//! | 0x5054424C | PTBL  | Share peer table (over established session)|
//!
//! # Why this is a separate crate
//!
//! The codec is used by:
//! - `snapclone-gossip` (UDP multicast discovery)
//! - `snapclone-dht` (DHT peer table exchange)
//! - Any future discovery protocol that wants LAN peer sharing
//! Having one canonical byte-for-byte implementation prevents drift.

pub mod scap;
pub mod ptbl;

pub use scap::{ScapFrame, encode_scap, decode_scap};
pub use ptbl::{PtblFrame, PeerEntry, encode_ptbl, decode_ptbl};
